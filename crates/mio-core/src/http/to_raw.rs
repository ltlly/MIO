use super::*;

fn write_message(
    mut raw: Vec<u8>,
    fields: &[Header],
    body: &[u8],
    trailers: &[Header],
    frame_chunked_without_trailers: bool,
) -> Vec<u8> {
    let chunked = !trailers.is_empty() || (frame_chunked_without_trailers && is_chunked(fields));
    for field in fields {
        if chunked
            && (field.name.eq_ignore_ascii_case("content-length")
                || field.name.eq_ignore_ascii_case("transfer-encoding"))
        {
            continue;
        }
        raw.extend_from_slice(field.name.as_bytes());
        raw.extend_from_slice(b": ");
        raw.extend_from_slice(&field.value);
        raw.extend_from_slice(b"\r\n");
    }
    if chunked {
        raw.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
    }
    raw.extend_from_slice(b"\r\n");
    if chunked {
        if !body.is_empty() {
            raw.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
            raw.extend_from_slice(body);
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(b"0\r\n");
        for field in trailers {
            raw.extend_from_slice(field.name.as_bytes());
            raw.extend_from_slice(b": ");
            raw.extend_from_slice(&field.value);
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(b"\r\n");
    } else {
        raw.extend_from_slice(body);
    }
    raw
}

pub fn request_to_raw(request: &HttpRequest) -> Vec<u8> {
    let target = Url::parse(&request.url)
        .ok()
        .and_then(|_| absolute_request_target(&request.url))
        .unwrap_or_else(|| request.url.clone());
    write_message(
        format!(
            "{} {} {}\r\n",
            request.method,
            target,
            request.version.as_str()
        )
        .into_bytes(),
        &request.headers,
        &request.body,
        &request.trailers,
        true,
    )
}

pub fn response_to_raw(response: &HttpResponse) -> Vec<u8> {
    let mut start_line = format!("{} {}", response.version.as_str(), response.status).into_bytes();
    if let Some(reason) = response.reason_phrase.as_deref().filter(|r| !r.is_empty()) {
        start_line.push(b' ');
        start_line.extend_from_slice(reason);
    }
    start_line.extend_from_slice(b"\r\n");
    write_message(
        start_line,
        &response.headers,
        &response.body,
        &response.trailers,
        false,
    )
}
