use super::*;

pub(crate) async fn send(
    proxy: SocketAddr,
    target: SocketAddr,
    path: &str,
    body: &[u8],
) -> TcpStream {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    stream.write_all(format!(
                "POST http://{target}{path} HTTP/1.1\r\nHost: {target}\r\nX-Duplicate: first\r\nX-Duplicate: second\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ).as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    stream
}

pub(crate) async fn respond(listener: &TcpListener, path: &str, body: &[u8]) {
    let (mut stream, peer) = listener.accept().await.unwrap();
    assert!(peer.ip().is_loopback());
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 8192);
        head.push(stream.read_u8().await.unwrap());
    }
    assert!(head.starts_with(format!("POST {path} HTTP/1.1\r\n").as_bytes()));
    let text = std::str::from_utf8(&head).unwrap().to_ascii_lowercase();
    assert!(text.contains("x-duplicate: first\r\n"));
    assert!(text.contains("x-duplicate: second\r\n"));
    let mut bytes = vec![0; body.len()];
    stream.read_exact(&mut bytes).await.unwrap();
    assert_eq!(bytes, body);
    stream.write_all(b"HTTP/1.1 200 OK\r\nX-Duplicate: one\r\nX-Duplicate: two\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\0\xffR").await.unwrap();
    assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
}

pub(crate) async fn response(mut stream: TcpStream) {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    assert!(bytes.starts_with(b"HTTP/1.1 200"));
    let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    assert!(text.contains("x-duplicate: one\r\n"));
    assert!(text.contains("x-duplicate: two\r\n"));
    assert!(bytes.ends_with(&[0, 255, b'R']));
}

pub(crate) async fn acknowledged(client: &mut Client, project: ProjectKey, workspace: &Workspace) {
    let Event::WorkspaceChanged { workspace: saved, .. } = event(client, |event| matches!(event,
                Event::WorkspaceChanged { project: owner, workspace: saved } if *owner == project && saved.revision == workspace.revision
            )).await else { unreachable!() };
    assert_eq!(saved, *workspace);
}

pub(crate) fn listener_pending(listener: &TcpListener) -> bool {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    listener.poll_accept(&mut context).is_pending()
}

pub(crate) const REQUEST_COOKIES: [&[u8]; 2] = [
    br#"dup=one; encoded=%41; quoted="part; tail=done"; flag"#,
    b"dup=two; request-only=client; opaque=\xff",
];
pub(crate) const RESPONSE_COOKIES: [&[u8]; 2] = [
    b"response-only=server; Path=/request-only; HttpOnly",
    b"dup=reply; Path=/; SameSite=Lax",
];
pub(crate) const REQUEST_PLAIN: &[u8] =
    b"\0\xffrequest-visible\0\xffrequest-visible\0\xffrequest-visible";
pub(crate) const RESPONSE_PLAIN: &[u8] =
    b"\0\xffresponse-visible\0\xffresponse-visible\0\xffresponse-visible";

pub(crate) fn compressed_fixture(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

pub(crate) fn body_pattern(bytes: &[u8]) -> String {
    format!(
        "^{}$",
        bytes
            .iter()
            .map(|byte| format!(r"\x{byte:02x}"))
            .collect::<String>()
    )
}

pub(crate) fn cookie_body_rule(
    match_type: ProxyInterceptType,
    relationship: ProxyInterceptRelation,
    condition: &str,
) -> ProxyInterceptRule {
    ProxyInterceptRule {
        id: Uuid::new_v4(),
        enabled: true,
        operator: ProxyInterceptOperator::And,
        match_type,
        relationship,
        condition: condition.into(),
    }
}

pub(crate) fn wire_header_values<'a>(head: &'a [u8], name: &[u8]) -> Vec<&'a [u8]> {
    head.split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let colon = line.iter().position(|byte| *byte == b':')?;
            line[..colon]
                .eq_ignore_ascii_case(name)
                .then(|| line[colon + 1..].trim_ascii())
        })
        .collect()
}

pub(crate) async fn send_cookie_gzip(
    proxy: SocketAddr,
    target: SocketAddr,
    path: &str,
    body: &[u8],
) -> TcpStream {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    let mut wire = format!(
        "POST http://{target}{path} HTTP/1.1\r\nHost: {target}\r\nContent-Type: application/octet-stream\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .into_bytes();
    for cookie in REQUEST_COOKIES {
        wire.extend_from_slice(b"Cookie: ");
        wire.extend_from_slice(cookie);
        wire.extend_from_slice(b"\r\n");
    }
    wire.extend_from_slice(b"\r\n");
    wire.extend_from_slice(body);
    stream.write_all(&wire).await.unwrap();
    stream
}

pub(crate) async fn respond_cookie_gzip(
    listener: &TcpListener,
    path: &str,
    request_body: &[u8],
    response_body: &[u8],
    request_gzip: bool,
) -> TcpStream {
    let (mut stream, peer) = listener.accept().await.unwrap();
    assert!(peer.ip().is_loopback());
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 8192);
        head.push(stream.read_u8().await.unwrap());
    }
    assert!(head.starts_with(format!("POST {path} HTTP/1.1\r\n").as_bytes()));
    assert_eq!(wire_header_values(&head, b"cookie"), REQUEST_COOKIES);
    let expected_encoding = if request_gzip {
        vec![b"gzip".as_slice()]
    } else {
        Vec::new()
    };
    assert_eq!(
        wire_header_values(&head, b"content-encoding"),
        expected_encoding
    );
    assert_eq!(
        wire_header_values(&head, b"content-length"),
        [request_body.len().to_string().as_bytes()]
    );
    let mut observed = vec![0; request_body.len()];
    stream.read_exact(&mut observed).await.unwrap();
    assert_eq!(observed, request_body);
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n",
        response_body.len()
    )
    .into_bytes();
    for cookie in RESPONSE_COOKIES {
        response.extend_from_slice(b"Set-Cookie: ");
        response.extend_from_slice(cookie);
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(response_body);
    stream.write_all(&response).await.unwrap();
    stream
}

pub(crate) async fn cookie_body_response(mut stream: TcpStream, expected_body: &[u8], gzip: bool) {
    let mut wire = Vec::new();
    stream.read_to_end(&mut wire).await.unwrap();
    assert!(wire.starts_with(b"HTTP/1.1 200"));
    let body_start = wire
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap()
        + 4;
    let head = &wire[..body_start];
    assert_eq!(wire_header_values(head, b"set-cookie"), RESPONSE_COOKIES);
    let expected_encoding = if gzip {
        vec![b"gzip".as_slice()]
    } else {
        Vec::new()
    };
    assert_eq!(
        wire_header_values(head, b"content-encoding"),
        expected_encoding
    );
    assert_eq!(
        wire_header_values(head, b"content-length"),
        [expected_body.len().to_string().as_bytes()]
    );
    assert_eq!(&wire[body_start..], expected_body);
}

pub(crate) fn assert_cookie_response(record: &HttpResponse, expected_body: &[u8], gzip: bool) {
    assert_eq!(record.status, 200);
    assert_eq!(record.version, HttpVersion::Http11);
    assert_eq!(record.body, expected_body);
    assert!(record.trailers.is_empty());
    assert_eq!(record.headers.len(), if gzip { 6 } else { 5 });
    let values = |name: &str| {
        record
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_slice())
            .collect::<Vec<_>>()
    };
    assert_eq!(values("set-cookie"), RESPONSE_COOKIES);
    assert_eq!(values("content-type"), [b"application/octet-stream"]);
    assert_eq!(values("connection"), [b"close"]);
    let expected_encoding = if gzip {
        vec![b"gzip".as_slice()]
    } else {
        Vec::new()
    };
    assert_eq!(values("content-encoding"), expected_encoding);
    assert_eq!(
        values("content-length"),
        [expected_body.len().to_string().as_bytes()]
    );
}
