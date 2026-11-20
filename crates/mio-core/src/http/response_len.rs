use super::*;

#[cfg(test)]
pub fn response_raw_len(
    version: HttpVersion,
    status: u16,
    headers: &[Header],
    trailers: &[Header],
    body_bytes: u64,
) -> u64 {
    let mut len = (version.as_str().len() + 1 + status.to_string().len() + b"\r\n".len()) as u64;
    let chunked = !trailers.is_empty();
    for field in headers {
        if chunked
            && (field.name.eq_ignore_ascii_case("content-length")
                || field.name.eq_ignore_ascii_case("transfer-encoding"))
        {
            continue;
        }
        len += (field.name.len() + b": ".len() + field.value.len() + b"\r\n".len()) as u64;
    }
    if chunked {
        len += b"Transfer-Encoding: chunked\r\n".len() as u64;
    }
    len += b"\r\n".len() as u64;
    if chunked {
        if body_bytes != 0 {
            len += format!("{:x}\r\n", body_bytes).len() as u64 + body_bytes + b"\r\n".len() as u64;
        }
        len += b"0\r\n".len() as u64;
        for field in trailers {
            len += (field.name.len() + b": ".len() + field.value.len() + b"\r\n".len()) as u64;
        }
        len += b"\r\n".len() as u64;
    } else {
        len += body_bytes;
    }
    len
}

fn canonical_reason(status: u16) -> &'static [u8] {
    match status {
        100 => b"Continue",
        101 => b"Switching Protocols",
        200 => b"OK",
        201 => b"Created",
        202 => b"Accepted",
        204 => b"No Content",
        206 => b"Partial Content",
        301 => b"Moved Permanently",
        302 => b"Found",
        303 => b"See Other",
        304 => b"Not Modified",
        307 => b"Temporary Redirect",
        308 => b"Permanent Redirect",
        400 => b"Bad Request",
        401 => b"Unauthorized",
        403 => b"Forbidden",
        404 => b"Not Found",
        405 => b"Method Not Allowed",
        408 => b"Request Timeout",
        409 => b"Conflict",
        410 => b"Gone",
        411 => b"Length Required",
        413 => b"Payload Too Large",
        414 => b"URI Too Long",
        415 => b"Unsupported Media Type",
        418 => b"I'm a teapot",
        422 => b"Unprocessable Entity",
        429 => b"Too Many Requests",
        500 => b"Internal Server Error",
        501 => b"Not Implemented",
        502 => b"Bad Gateway",
        503 => b"Service Unavailable",
        504 => b"Gateway Timeout",
        _ => b"",
    }
}

pub fn intruder_response_len(
    version: HttpVersion,
    status: u16,
    reason_phrase: Option<&[u8]>,
    headers: &[Header],
    body_bytes: u64,
) -> u64 {
    let status_line_len = match version {
        HttpVersion::Http10 | HttpVersion::Http11 => {
            let reason = reason_phrase.unwrap_or_else(|| canonical_reason(status));
            let reason_extra = if reason.is_empty() {
                0
            } else {
                1 + reason.len()
            };
            version.as_str().len() + 1 + status.to_string().len() + reason_extra + 2
        }
        HttpVersion::Http2 => {
            let reason_extra = reason_phrase
                .filter(|r| !r.is_empty())
                .map_or(0, |r| 1 + r.len());
            version.as_str().len() + 1 + status.to_string().len() + reason_extra + 2
        }
    };
    let mut len = status_line_len as u64;
    if is_chunked(headers) {
        for field in headers {
            if field.name.eq_ignore_ascii_case("transfer-encoding")
                || field.name.eq_ignore_ascii_case("content-length")
            {
                continue;
            }
            len += (field.name.len() + 2 + field.value.len() + 2) as u64;
        }
        len += (b"Content-Length: ".len() + body_bytes.to_string().len() + 2) as u64;
    } else {
        for field in headers {
            len += (field.name.len() + 2 + field.value.len() + 2) as u64;
        }
    }
    len + 2 + body_bytes
}
