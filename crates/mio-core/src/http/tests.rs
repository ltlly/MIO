use super::*;
pub(super) use std::time::Duration;
pub(super) use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
pub(super) use tokio_util::sync::CancellationToken;

pub(super) const PATHS: &[&str] = &[
    "/a/../b",
    "/a/%2e%2E/b",
    "/a/./b?first=%2F&x=%2e%2E&x=1",
    "/a//b///c?",
    "/a/%2F/b?next=http://other.invalid/../x",
    "/a/%5C/%5c?upper=%5C&lower=%5c",
];

pub(super) const DRAFT_ONLY_PATHS: &[&str] = &[r"/a\b", r"/a?b=\c"];

pub(super) fn interception_rule(
    id: u128,
    operator: crate::ProxyInterceptOperator,
    match_type: crate::ProxyInterceptType,
    relationship: crate::ProxyInterceptRelation,
    condition: &str,
) -> crate::ProxyInterceptRule {
    crate::ProxyInterceptRule {
        id: crate::Uuid::from_u128(id),
        enabled: true,
        operator,
        match_type,
        relationship,
        condition: condition.into(),
    }
}

pub(super) fn interception_record() -> FlowRecord {
    FlowRecord {
        summary: crate::FlowSummary {
            id: crate::Uuid::new_v4(),
            revision: 1,
            source: crate::TrafficSource::Proxy,
            method: "GET".into(),
            url: "http://example.com/fixture".into(),
            status: Some(200),
            state: crate::FlowState::Pending,
            request_bytes: 0,
            response_bytes: 0,
            started_unix_ms: 1,
            elapsed_ms: 0,
        },
        request: HttpRequest {
            method: "GET".into(),
            url: "http://example.com/fixture".into(),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: Vec::new(),
            trailers: Vec::new(),
        },
        response: Some(HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![Header {
                name: "Content-Type".into(),
                value: b"text/plain".to_vec(),
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        }),
    }
}

pub(super) fn eval_port(listener_port: u16) -> mio_proxy::InterceptEvalContext<'static> {
    mio_proxy::InterceptEvalContext {
        listener_port,
        target_ip: None,
        was_annotated: false,
    }
}

#[path = "tests/intercept_cookies.rs"]
mod intercept_cookies;
#[path = "tests/intercept_headers.rs"]
mod intercept_headers;
#[path = "tests/intercept_rules.rs"]
mod intercept_rules;
#[path = "tests/intercept_unpack.rs"]
mod intercept_unpack;
#[path = "tests/parameters_match.rs"]
mod parameters_match;
#[path = "tests/parameters_raw.rs"]
mod parameters_raw;
#[path = "tests/proxy_transforms.rs"]
mod proxy_transforms_tests;
#[path = "tests/targets_ws_extra.rs"]
mod targets_ws_extra;
#[path = "tests/transform_match.rs"]
mod transform_match;
