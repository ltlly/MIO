use crate::{ProxyError, UpstreamRoute};
use mio_plugin_api::HttpVersion;
use rama::bytes::{Bytes, BytesMut};
use rama::extensions::Extension;
use rama::http::{
    HeaderMap, Method,
    body::{Frame, SizeHint, StreamingBody},
    headers::{HeaderMapExt, ProxyAuthorization},
};
use rama::net::{uri::Uri, user::ProxyCredential};
use std::{
    convert::Infallible,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

#[derive(Clone, Debug)]
pub(crate) struct RawH1Request {
    pub bytes: Arc<[u8]>,
    pub suppressed: Arc<AtomicBool>,
}

impl Extension for RawH1Request {}

pub(super) struct RawBody {
    data: Option<Bytes>,
    suppressed: Arc<AtomicBool>,
}

impl RawBody {
    pub(super) fn new(bytes: Vec<u8>, suppressed: Arc<AtomicBool>) -> Self {
        Self {
            data: (!bytes.is_empty()).then(|| Bytes::from(bytes)),
            suppressed,
        }
    }
}

impl StreamingBody for RawBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.suppressed.load(Ordering::Acquire) {
            self.data = None;
            return Poll::Ready(None);
        }
        Poll::Ready(self.data.take().map(|bytes| Ok(Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        self.suppressed.load(Ordering::Acquire) || self.data.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pub(super) fn build_raw_h1_request(
    method: &Method,
    uri: &Uri,
    version: HttpVersion,
    headers: &HeaderMap,
    body: &[u8],
    upstream_route: &UpstreamRoute,
) -> Result<Arc<[u8]>, ProxyError> {
    let route = upstream_route.rama()?;
    let via_http_proxy = matches!(
        upstream_route,
        UpstreamRoute::Http(_) | UpstreamRoute::Https { .. }
    ) && uri.scheme_str() == Some("http");
    let mut target = BytesMut::new();
    if via_http_proxy {
        uri.write_http_absolute_form(&mut target)
    } else {
        uri.write_http_origin_form(&mut target)
    }
    .map_err(ProxyError::invalid)?;
    let mut raw = Vec::with_capacity(target.len() + body.len() + 128);
    raw.extend_from_slice(method.as_str().as_bytes());
    raw.push(b' ');
    raw.extend_from_slice(&target);
    raw.extend_from_slice(match version {
        HttpVersion::Http10 => b" HTTP/1.0\r\n",
        HttpVersion::Http11 => b" HTTP/1.1\r\n",
        HttpVersion::Http2 => return Err(ProxyError::invalid("原始 HTTP/1 发送不接受 HTTP/2")),
    });
    let mut h1_headers = headers.clone();
    if via_http_proxy
        && let Some(pa) = route.proxy_address()
        && let Some(ProxyCredential::Basic(basic)) = pa.credential.clone()
    {
        h1_headers.typed_insert(ProxyAuthorization(basic));
    }
    for (name, value) in h1_headers.ordered_iter() {
        name.write_original(&mut raw);
        if value.is_empty() {
            raw.extend_from_slice(b":\r\n");
        } else {
            raw.extend_from_slice(b": ");
            raw.extend_from_slice(value.as_bytes());
            raw.extend_from_slice(b"\r\n");
        }
    }
    raw.extend_from_slice(b"\r\n");
    raw.extend_from_slice(body);
    Ok(Arc::from(raw))
}
