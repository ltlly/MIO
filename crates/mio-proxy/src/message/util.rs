use crate::ProxyError;
use mio_plugin_api::{Header, HttpVersion};
use rama::extensions::ExtensionsRef;
use rama::http::{Body, HeaderMap, HeaderName, HeaderValue, Version, body::util::BodyExt};

pub(crate) fn version(version: HttpVersion) -> Version {
    match version {
        HttpVersion::Http10 => Version::HTTP_10,
        HttpVersion::Http11 => Version::HTTP_11,
        HttpVersion::Http2 => Version::HTTP_2,
    }
}

pub(crate) fn model_version(version: Version) -> HttpVersion {
    match version {
        Version::HTTP_10 => HttpVersion::Http10,
        Version::HTTP_2 => HttpVersion::Http2,
        _ => HttpVersion::Http11,
    }
}

pub(crate) fn reason_phrase(response: &rama::http::Response) -> Option<Vec<u8>> {
    response
        .extensions()
        .get_ref::<rama::http::proto::h1::ext::ReasonPhrase>()
        .map(|reason| reason.as_bytes())
        .filter(|bytes| !bytes.is_empty())
        .map(<[u8]>::to_vec)
}

pub(crate) fn headers(headers: &HeaderMap) -> Vec<Header> {
    headers
        .ordered_iter()
        .map(|(name, value)| Header {
            name: name.to_string(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

pub(crate) fn header_map(headers: &[Header]) -> Result<HeaderMap, ProxyError> {
    let mut map = HeaderMap::new();
    for header in headers {
        let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(ProxyError::invalid)?;
        let value = HeaderValue::from_bytes(&header.value).map_err(ProxyError::invalid)?;
        map.append(name, value);
    }
    Ok(map)
}

pub(crate) fn consume_header_budget(
    headers: &HeaderMap,
    remaining: &mut (usize, usize),
) -> Result<(), ProxyError> {
    for (name, value) in headers.ordered_iter() {
        remaining.0 = remaining
            .0
            .checked_sub(1)
            .ok_or_else(|| ProxyError::invalid("响应 header 与 trailers 合计超过 1024 个字段"))?;
        remaining.1 = remaining
            .1
            .checked_sub(name.as_str().len())
            .and_then(|bytes| bytes.checked_sub(value.as_bytes().len()))
            .and_then(|bytes| bytes.checked_sub(4))
            .ok_or_else(|| ProxyError::invalid("响应 header 与 trailers 合计超过 128 KiB"))?;
    }
    Ok(())
}

pub(crate) async fn body(body: Body, max: usize) -> Result<(Vec<u8>, Vec<Header>), ProxyError> {
    body_with_header_budget(body, max, None).await
}

pub(crate) async fn body_with_header_budget(
    mut body: Body,
    max: usize,
    mut header_budget: Option<(usize, usize)>,
) -> Result<(Vec<u8>, Vec<Header>), ProxyError> {
    let mut bytes = Vec::new();
    let mut trailers = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(ProxyError::network)?;
        match frame.into_data() {
            Ok(data) => {
                if data.len() > max.saturating_sub(bytes.len()) {
                    return Err(ProxyError::BodyLimit { limit: max });
                }
                bytes.extend_from_slice(&data);
            }
            Err(frame) => {
                if let Ok(map) = frame.into_trailers() {
                    if let Some(remaining) = &mut header_budget {
                        consume_header_budget(&map, remaining)?;
                    }
                    trailers.extend(headers(&map));
                }
            }
        }
    }
    Ok((bytes, trailers))
}
