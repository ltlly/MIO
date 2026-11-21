#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageDisplayCharset {
    Utf8,
    Gbk,
    Gb18030,
    Big5,
    ShiftJis,
    EucKr,
    Iso8859_1,
    Windows1252,
    UsAscii,
    Other(&'static encoding_rs::Encoding),
}

impl MessageDisplayCharset {
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        let lower = label.trim().to_ascii_lowercase();
        match lower.as_str() {
            "" | "utf-8" | "utf8" => Some(Self::Utf8),
            "gbk" | "gb2312" | "cp936" | "ms936" | "x-gbk" => Some(Self::Gbk),
            "gb18030" => Some(Self::Gb18030),
            "big5" | "big5-hkscs" | "cn-big5" | "xx-big5" | "cp950" => Some(Self::Big5),
            "shift_jis" | "shift-jis" | "sjis" | "ms932" | "cp932" | "windows-31j" | "x-sjis" => {
                Some(Self::ShiftJis)
            }
            "euc-kr" | "euckr" | "cp949" | "ks_c_5601-1987" | "windows-949" => Some(Self::EucKr),
            "iso-8859-1" | "iso_8859-1" | "latin1" | "latin-1" => Some(Self::Iso8859_1),
            "windows-1252" | "cp1252" => Some(Self::Windows1252),
            "us-ascii" | "ascii" => Some(Self::UsAscii),
            other => encoding_rs::Encoding::for_label(other.as_bytes()).map(Self::Other),
        }
    }

    pub(crate) fn decode(self, bytes: &[u8]) -> Option<String> {
        match self {
            Self::Utf8 => std::str::from_utf8(bytes).ok().map(str::to_owned),
            Self::Iso8859_1 => Some(decode_latin1(bytes)),
            Self::UsAscii => Some(decode_ascii(bytes)),
            Self::Gbk => Some(
                encoding_rs::GBK
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::Gb18030 => Some(
                encoding_rs::GB18030
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::Big5 => Some(
                encoding_rs::BIG5
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::ShiftJis => Some(
                encoding_rs::SHIFT_JIS
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::EucKr => Some(
                encoding_rs::EUC_KR
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::Windows1252 => Some(
                encoding_rs::WINDOWS_1252
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            ),
            Self::Other(enc) => Some(enc.decode_without_bom_handling(bytes).0.into_owned()),
        }
    }

    pub(crate) fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Utf8 | Self::UsAscii => text.as_bytes().to_vec(),
            Self::Iso8859_1 => {
                let mut out = Vec::with_capacity(text.len());
                let mut buf = [0u8; 4];
                for ch in text.chars() {
                    if (ch as u32) <= 0xFF {
                        out.push(ch as u8);
                    } else {
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                }
                out
            }
            Self::Gbk => encoding_rs::GBK.encode(text).0.into_owned(),
            Self::Gb18030 => encoding_rs::GB18030.encode(text).0.into_owned(),
            Self::Big5 => encoding_rs::BIG5.encode(text).0.into_owned(),
            Self::ShiftJis => encoding_rs::SHIFT_JIS.encode(text).0.into_owned(),
            Self::EucKr => encoding_rs::EUC_KR.encode(text).0.into_owned(),
            Self::Windows1252 => encoding_rs::WINDOWS_1252.encode(text).0.into_owned(),
            Self::Other(enc) => enc.encode(text).0.into_owned(),
        }
    }
}

fn extract_header_charset(bytes: &[u8]) -> Option<String> {
    let (_, headers) = parse_http_head_info(bytes)?;
    for header in headers {
        if !header.name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        let value = std::str::from_utf8(&header.value).ok()?;
        for part in value.split(';').skip(1) {
            let trimmed = part.trim();
            if let Some((key, raw_val)) = trimmed.split_once('=')
                && key.trim().eq_ignore_ascii_case("charset")
            {
                let cleaned = raw_val
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .trim();
                if !cleaned.is_empty() {
                    return Some(cleaned.to_ascii_lowercase());
                }
            }
        }
    }
    None
}

fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

fn decode_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if b < 0x80 { char::from(b) } else { '\u{FFFD}' })
        .collect()
}

fn decode_named_charset(bytes: &[u8], charset_lower: &str) -> Option<String> {
    match MessageDisplayCharset::from_label(charset_lower) {
        Some(charset) => charset.decode(bytes),
        None => std::str::from_utf8(bytes).ok().map(str::to_owned),
    }
}

fn active_display_charset(original_wire_bytes: &[u8]) -> MessageDisplayCharset {
    let (mode, specific) = crate::theme::message_editor_charset();
    match mode {
        0 => extract_header_charset(original_wire_bytes)
            .and_then(|label| MessageDisplayCharset::from_label(&label))
            .unwrap_or(MessageDisplayCharset::Utf8),
        1 => MessageDisplayCharset::Utf8,
        2 => MessageDisplayCharset::Iso8859_1,
        _ => MessageDisplayCharset::from_label(&specific).unwrap_or(MessageDisplayCharset::Utf8),
    }
}

pub(super) fn decode_message_bytes_with_charset(bytes: &[u8]) -> (MessageDisplayCharset, Option<String>) {
    let (mode, specific) = crate::theme::message_editor_charset();
    match mode {
        // 0: Recognize automatically based on message headers
        0 => {
            if let Some(charset) = extract_header_charset(bytes)
                && let Some(resolved) = MessageDisplayCharset::from_label(&charset)
            {
                let decoded = resolved.decode(bytes);
                return (resolved, decoded);
            }
            (
                MessageDisplayCharset::Utf8,
                std::str::from_utf8(bytes).ok().map(str::to_owned),
            )
        }
        // 1: Use platform default (UTF-8)
        1 => (
            MessageDisplayCharset::Utf8,
            std::str::from_utf8(bytes).ok().map(str::to_owned),
        ),
        // 2: Display as raw bytes (1:1 ISO-8859-1 byte view)
        2 => (MessageDisplayCharset::Iso8859_1, Some(decode_latin1(bytes))),
        // 3: Use a specific character set
        _ => {
            let resolved =
                MessageDisplayCharset::from_label(&specific).unwrap_or(MessageDisplayCharset::Utf8);
            let decoded = decode_named_charset(bytes, &specific.trim().to_ascii_lowercase());
            (resolved, decoded)
        }
    }
}

pub(super) fn decode_message_bytes(bytes: &[u8]) -> Option<String> {
    decode_message_bytes_with_charset(bytes).1
}

pub(super) fn encode_raw_text_bytes(
    text: &str,
    cached_charset: MessageDisplayCharset,
    cached_pref: &(u8, String),
    original_wire_bytes: &[u8],
) -> Vec<u8> {
    if crate::theme::message_editor_charset() == *cached_pref {
        cached_charset.encode(text)
    } else {
        active_display_charset(original_wire_bytes).encode(text)
    }
}

fn parse_http_head_with_buf<'a>(
    input: &'a [u8],
    headers: &mut [httparse::Header<'a>],
) -> Result<(usize, Vec<mio_core::Header>), httparse::Error> {
    let mut config = httparse::ParserConfig::default();
    config
        .allow_multiple_spaces_in_request_line_delimiters(true)
        .allow_multiple_spaces_in_response_status_delimiters(true)
        .allow_space_before_first_header_name(true)
        .allow_obsolete_multiline_headers_in_responses(true);
    let (status, parsed_headers) = if input.starts_with(b"HTTP/") {
        let mut resp = httparse::Response::new(headers);
        let status = config.parse_response(&mut resp, input)?;
        let list = resp
            .headers
            .iter()
            .map(|h| mio_core::Header {
                name: h.name.to_owned(),
                value: h.value.to_vec(),
            })
            .collect();
        (status, list)
    } else {
        let mut req = httparse::Request::new(headers);
        let status = config.parse_request(&mut req, input)?;
        let list = req
            .headers
            .iter()
            .map(|h| mio_core::Header {
                name: h.name.to_owned(),
                value: h.value.to_vec(),
            })
            .collect();
        (status, list)
    };
    match status {
        httparse::Status::Complete(offset) => Ok((offset, parsed_headers)),
        httparse::Status::Partial => Err(httparse::Error::HeaderValue),
    }
}

pub(super) fn parse_http_head_info(bytes: &[u8]) -> Option<(usize, Vec<mio_core::Header>)> {
    if bytes.is_empty() {
        return None;
    }
    let newline = bytes.iter().position(|&b| b == b'\n')?;
    let eol_len = if newline > 0 && bytes[newline - 1] == b'\r' {
        2
    } else {
        1
    };
    let first_line = &bytes[..newline + 1 - eol_len];
    let normalized = if let Some(rest) = first_line.strip_prefix(b"HTTP/2.0 ") {
        let mut v = b"HTTP/1.1 ".to_vec();
        v.extend_from_slice(rest);
        v.extend_from_slice(&bytes[newline + 1 - eol_len..]);
        Some(v)
    } else if let Some(rest) = first_line.strip_prefix(b"HTTP/2 ") {
        let mut v = b"HTTP/1.1 ".to_vec();
        v.extend_from_slice(rest);
        v.extend_from_slice(&bytes[newline + 1 - eol_len..]);
        Some(v)
    } else if let Some(prefix) = first_line.strip_suffix(b" HTTP/2.0") {
        let mut v = prefix.to_vec();
        v.extend_from_slice(b" HTTP/1.1");
        v.extend_from_slice(&bytes[newline + 1 - eol_len..]);
        Some(v)
    } else if let Some(prefix) = first_line.strip_suffix(b" HTTP/2") {
        let mut v = prefix.to_vec();
        v.extend_from_slice(b" HTTP/1.1");
        v.extend_from_slice(&bytes[newline + 1 - eol_len..]);
        Some(v)
    } else {
        None
    };
    let input = normalized.as_deref().unwrap_or(bytes);
    let added = input.len() - bytes.len();
    let mut small_headers = [httparse::EMPTY_HEADER; 64];
    match parse_http_head_with_buf(input, &mut small_headers) {
        Ok((offset, list)) => Some((offset - added, list)),
        Err(httparse::Error::TooManyHeaders) => {
            let mut large_headers = vec![httparse::EMPTY_HEADER; 4096];
            let (offset, list) = parse_http_head_with_buf(input, &mut large_headers).ok()?;
            Some((offset - added, list))
        }
        Err(_) => None,
    }
}

pub(super) fn serialize_pretty_edit(
    edited: &str,
    original_wire: &[u8],
    baseline: Option<&str>,
) -> Vec<u8> {
    let Some((body_offset, edited_headers)) = parse_http_head_info(edited.as_bytes()) else {
        return edited.as_bytes().to_vec();
    };
    let head_part = &edited[..body_offset];
    let body_part = &edited[body_offset..];
    let normalized_head: String = head_part
        .trim_end_matches(['\r', '\n'])
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\r\n");
    let mut out = Vec::with_capacity(normalized_head.len() + 4 + body_part.len());
    out.extend_from_slice(normalized_head.as_bytes());
    out.extend_from_slice(b"\r\n\r\n");
    let orig_info = parse_http_head_info(original_wire);
    if let Some(base) = baseline
        && let Some((base_body_offset, _)) = parse_http_head_info(base.as_bytes())
        && &base[base_body_offset..] == body_part
        && let Some((orig_body_offset, ref orig_headers)) = orig_info
        && mio_core::is_chunked_headers(&edited_headers) == mio_core::is_chunked_headers(orig_headers)
    {
        out.extend_from_slice(&original_wire[orig_body_offset..]);
        return out;
    }
    let mut body_text = body_part;
    let mut extracted_trailers: Vec<mio_core::Header> = Vec::new();
    if mio_core::is_chunked_headers(&edited_headers)
        && let Some((orig_off, ref orig_hdrs)) = orig_info
        && mio_core::is_chunked_headers(orig_hdrs)
        && let Ok((_, orig_trailers)) =
            mio_core::decode_http_body(orig_hdrs, &original_wire[orig_off..])
        && !orig_trailers.is_empty()
    {
        let mut trailer_suffix = String::new();
        for t in &orig_trailers {
            trailer_suffix.push_str(&format!(
                "{}: {}\n",
                t.name,
                String::from_utf8_lossy(&t.value)
            ));
        }
        if let Some(stripped) = body_text.strip_suffix(&trailer_suffix) {
            body_text = stripped.strip_suffix('\n').unwrap_or(stripped);
            extracted_trailers = orig_trailers;
        }
    }
    let payload_bytes = body_text.as_bytes().to_vec();
    if mio_core::is_chunked_headers(&edited_headers)
        && mio_core::decode_http_body(&edited_headers, &payload_bytes).is_err()
    {
        if !payload_bytes.is_empty() {
            out.extend_from_slice(format!("{:x}\r\n", payload_bytes.len()).as_bytes());
            out.extend_from_slice(&payload_bytes);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n");
        for t in &extracted_trailers {
            out.extend_from_slice(t.name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(&t.value);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
    } else {
        out.extend_from_slice(&payload_bytes);
    }
    out
}
