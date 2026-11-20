use std::{borrow::Cow, fmt, time::Instant};

use mio_plugin_api::{Header, HttpRequest, HttpResponse, HttpVersion};
use tokio_util::sync::CancellationToken;

use super::{MAX_HEAD, is_chunked, token};

const CHUNK: usize = 8 * 1024;
const MAX_LAYERS: usize = 16;
type UnpackedMessage = (Vec<Header>, Vec<u8>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UnpackError {
    Cancelled,
    Deadline,
    Failed(String),
}

impl fmt::Display for UnpackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("消息解包已取消"),
            Self::Deadline => f.write_str("消息解包超过截止时间"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for UnpackError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coding {
    Identity,
    Gzip,
    Deflate,
    Brotli,
}

impl Coding {
    fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Brotli => "Brotli",
        }
    }
}

pub(crate) fn unpack_response(
    response: &HttpResponse,
    request_method: &str,
    max_body_bytes: usize,
    cancel: &CancellationToken,
    deadline: std::time::Instant,
) -> std::result::Result<Option<HttpResponse>, UnpackError> {
    if request_method == "HEAD"
        || (100..200).contains(&response.status)
        || matches!(response.status, 204 | 205 | 304)
        || (request_method == "CONNECT" && (200..300).contains(&response.status))
    {
        return Ok(None);
    }
    let Some((headers, body)) = unpack_message(
        &response.headers,
        &response.body,
        response.version,
        !response.trailers.is_empty(),
        max_body_bytes,
        cancel,
        deadline,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(HttpResponse {
        status: response.status,
        version: response.version,
        reason_phrase: response.reason_phrase.clone(),
        headers,
        body,
        trailers: response.trailers.clone(),
    }))
}

pub(crate) fn unpack_request(
    request: &HttpRequest,
    max_body_bytes: usize,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<Option<HttpRequest>, UnpackError> {
    let Some((headers, body)) = unpack_message(
        &request.headers,
        &request.body,
        request.version,
        !request.trailers.is_empty(),
        max_body_bytes,
        cancel,
        deadline,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(HttpRequest {
        method: request.method.clone(),
        url: request.url.clone(),
        version: request.version,
        headers,
        body,
        trailers: request.trailers.clone(),
    }))
}

pub fn decompress_gzip_bytes(
    input: &[u8],
    max_body_bytes: usize,
) -> std::result::Result<Vec<u8>, String> {
    if input.len() < 10 || !input.starts_with(&[0x1f, 0x8b]) {
        return Err("输入不是有效的 Gzip 数据（缺少 1f 8b 魔数头）；原始字节已保留。".into());
    }
    decode_layer(Coding::Gzip, input, max_body_bytes, &|| Ok(())).map_err(|err| err.to_string())
}

pub fn compress_gzip_bytes(
    input: &[u8],
    max_input_bytes: usize,
) -> std::result::Result<Vec<u8>, String> {
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write as _;

    if input.len() > max_input_bytes {
        return Err(format!("Gzip 压缩输入超过 {max_input_bytes} 字节上限。"));
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|err| format!("Gzip 压缩失败：{err}"))?;
    encoder
        .finish()
        .map_err(|err| format!("Gzip 压缩收尾失败：{err}"))
}

pub fn compress_deflate_bytes(
    input: &[u8],
    max_input_bytes: usize,
) -> std::result::Result<Vec<u8>, String> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write as _;

    if input.len() > max_input_bytes {
        return Err(format!("Deflate 压缩输入超过 {max_input_bytes} 字节上限。"));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(input)
        .map_err(|err| format!("Deflate 压缩失败：{err}"))?;
    encoder
        .finish()
        .map_err(|err| format!("Deflate 压缩收尾失败：{err}"))
}

pub fn decompress_deflate_bytes(
    input: &[u8],
    max_body_bytes: usize,
) -> std::result::Result<Vec<u8>, String> {
    if input.is_empty() {
        return Err("Deflate 解压输入不能为空。".into());
    }
    decode_layer(Coding::Deflate, input, max_body_bytes, &|| Ok(())).map_err(|err| err.to_string())
}

pub fn decompress_brotli_bytes(
    input: &[u8],
    max_body_bytes: usize,
) -> std::result::Result<Vec<u8>, String> {
    if input.is_empty() {
        return Err("Brotli 解压输入不能为空。".into());
    }
    decode_layer(Coding::Brotli, input, max_body_bytes, &|| Ok(())).map_err(|err| err.to_string())
}

fn unpack_message(
    fields: &[Header],
    encoded: &[u8],
    version: HttpVersion,
    has_trailers: bool,
    max_body_bytes: usize,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<Option<UnpackedMessage>, UnpackError> {
    let codings = content_codings(fields)?;
    if codings.iter().all(|coding| *coding == Coding::Identity) {
        return Ok(None);
    }
    // Rama only removes chunk framing; Raw requires the normalized single chunked value.
    let mut transfer_codings = fields
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("transfer-encoding"));
    if let Some(coding) = transfer_codings.next()
        && (version != HttpVersion::Http11
            || !coding.value.eq_ignore_ascii_case(b"chunked")
            || transfer_codings.next().is_some())
    {
        return Err(UnpackError::Failed(
            "消息 Transfer-Encoding 不支持安全的 Content-Encoding 解包".into(),
        ));
    }
    let check = || check_control(cancel, deadline);
    let mut body = Cow::Borrowed(encoded);
    for (index, coding) in codings.into_iter().enumerate().rev() {
        check()?;
        if coding == Coding::Identity {
            continue;
        }
        body = Cow::Owned(decode_layer(coding, &body, max_body_bytes, &check).map_err(
            |error| match error {
                UnpackError::Failed(reason) => UnpackError::Failed(format!(
                    "消息第 {} 层 {} 解包失败：{reason}",
                    index + 1,
                    coding.name()
                )),
                other => other,
            },
        )?);
    }
    check()?;

    let chunked = is_chunked(fields) || has_trailers;
    let mut length = (!chunked).then(|| body.len().to_string().into_bytes());
    let mut headers = Vec::with_capacity(fields.len());
    for header in fields {
        if header.name.eq_ignore_ascii_case("content-encoding") {
            continue;
        }
        if header.name.eq_ignore_ascii_case("content-length") {
            if let Some(value) = length.take() {
                headers.push(Header {
                    name: header.name.clone(),
                    value,
                });
            }
        } else {
            headers.push(header.clone());
        }
    }
    if let Some(value) = length {
        headers.push(Header {
            name: "Content-Length".into(),
            value,
        });
    }
    Ok(Some((headers, body.into_owned())))
}

fn content_codings(headers: &[Header]) -> Result<Vec<Coding>, UnpackError> {
    let mut codings = Vec::new();
    let mut head_bytes = 0usize;
    for header in headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("content-encoding"))
    {
        if header.value.len() > MAX_HEAD.saturating_sub(head_bytes) {
            return Err(UnpackError::Failed(
                "消息 Content-Encoding 超过 header 限额".into(),
            ));
        }
        head_bytes += header.value.len();
        for value in header.value.split(|byte| *byte == b',') {
            if codings.len() == MAX_LAYERS {
                return Err(UnpackError::Failed(
                    "消息 Content-Encoding 超过 16 层限额".into(),
                ));
            }
            let invalid_whitespace = value
                .iter()
                .any(|byte| byte.is_ascii_whitespace() && !matches!(*byte, b' ' | b'\t'));
            let value = value.trim_ascii();
            if invalid_whitespace || !token(value) {
                return Err(UnpackError::Failed("消息 Content-Encoding 格式无效".into()));
            }
            let coding = if value.eq_ignore_ascii_case(b"identity") {
                Coding::Identity
            } else if value.eq_ignore_ascii_case(b"gzip") {
                Coding::Gzip
            } else if value.eq_ignore_ascii_case(b"deflate") {
                Coding::Deflate
            } else if value.eq_ignore_ascii_case(b"br") {
                Coding::Brotli
            } else {
                return Err(UnpackError::Failed(
                    "消息 Content-Encoding 含不支持的编码".into(),
                ));
            };
            codings.push(coding);
        }
    }
    Ok(codings)
}

fn check_control(cancel: &CancellationToken, deadline: Instant) -> Result<(), UnpackError> {
    if cancel.is_cancelled() {
        Err(UnpackError::Cancelled)
    } else if Instant::now() >= deadline {
        Err(UnpackError::Deadline)
    } else {
        Ok(())
    }
}

mod codecs;
use codecs::decode_layer;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_limits;
#[cfg(test)]
mod tests_transport;
