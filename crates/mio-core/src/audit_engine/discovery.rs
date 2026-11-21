use std::{
    collections::{HashSet, VecDeque},
    time::Instant,
};

use anyhow::{Context, Result, ensure};
use mio_proxy::{SendConfig, send};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{AuditFinding, inspect_flow_passive};
use crate::{
    FlowRecord, FlowState, FlowSummary, Header, HttpRequest, HttpResponse, HttpVersion,
    TargetScope, TrafficSource, Uuid, store::audit::merge_audit_findings,
};

mod html_json;

use html_json::{extract_html_endpoints, extract_json_endpoints};

pub const MAX_DISCOVERED_PER_PAGE: usize = 256;
pub const MAX_CRAWL_DEPTH: u8 = 4;
pub const MAX_CRAWL_REQUESTS: usize = 64;
const MAX_DISCOVERY_BODY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    AnchorHref,
    FormSubmission,
    ScriptSrc,
    LinkHref,
    JsonUrlField,
    RedirectLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredEndpoint {
    pub url: String,
    pub method: String,
    pub source: DiscoverySource,
    pub depth: u8,
    pub request: HttpRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlConfig {
    pub max_depth: u8,
    pub max_requests: usize,
}

impl Default for CrawlConfig {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_requests: 16,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CrawlReport {
    pub endpoints: Vec<DiscoveredEndpoint>,
    pub flows: Vec<FlowRecord>,
    pub findings: Vec<AuditFinding>,
    pub requests_sent: usize,
    pub cancelled: bool,
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme().eq_ignore_ascii_case(b.scheme())
        && a.host_str().map(str::to_ascii_lowercase) == b.host_str().map(str::to_ascii_lowercase)
        && a.port_or_known_default() == b.port_or_known_default()
}

fn is_excluded_by_scope(scope: &TargetScope, url: &str) -> bool {
    if scope.advanced_mode {
        scope.advanced_exclude.iter().any(|r| r.matches_url(url))
    } else {
        scope.exclude.iter().any(|r| r.matches_url(url))
    }
}

fn scope_permits_url(scope: &TargetScope, url: &str) -> bool {
    if scope.has_enabled_includes() {
        scope.is_url_in_scope(url)
    } else {
        !is_excluded_by_scope(scope, url)
    }
}

fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(
        host.map(str::to_ascii_lowercase).as_deref(),
        Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
    )
}

fn is_forbidden_scheme(raw: &str) -> bool {
    let trimmed = raw.trim();
    for prefix in [
        "javascript:",
        "mailto:",
        "data:",
        "tel:",
        "blob:",
        "vbscript:",
        "about:",
        "file:",
    ] {
        if trimmed
            .get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
        {
            return true;
        }
    }
    false
}

pub(super) fn normalize_allowed_url(
    base: &Url,
    raw_ref: &str,
    allow_empty_as_base: bool,
    scope: &TargetScope,
) -> Option<Url> {
    let trimmed = raw_ref.trim();
    if trimmed.is_empty() {
        if !allow_empty_as_base {
            return None;
        }
        let mut resolved = base.clone();
        resolved.set_fragment(None);
        return (same_origin(base, &resolved) && scope_permits_url(scope, resolved.as_str()))
            .then_some(resolved);
    }
    if (!allow_empty_as_base && trimmed.starts_with('#')) || is_forbidden_scheme(trimmed) {
        return None;
    }
    let mut resolved = base.join(trimmed).ok()?;
    if !matches!(resolved.scheme(), "http" | "https") || resolved.host_str().is_none() {
        return None;
    }
    resolved.set_fragment(None);
    if !same_origin(base, &resolved) || !scope_permits_url(scope, resolved.as_str()) {
        return None;
    }
    Some(resolved)
}

fn host_header_value(url: &Url) -> Vec<u8> {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}").into_bytes(),
        None => host.as_bytes().to_vec(),
    }
}

pub(super) fn build_get_endpoint(url: Url, source: DiscoverySource) -> DiscoveredEndpoint {
    let host = host_header_value(&url);
    let url_str = url.to_string();
    DiscoveredEndpoint {
        url: url_str.clone(),
        method: "GET".into(),
        source,
        depth: 1,
        request: HttpRequest {
            method: "GET".into(),
            url: url_str,
            version: HttpVersion::Http11,
            headers: vec![Header {
                name: "Host".into(),
                value: host,
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        },
    }
}

pub(super) fn build_form_endpoint(
    base: &Url,
    scope: &TargetScope,
    action: &str,
    method_raw: &str,
    fields: &[(String, String)],
) -> Option<DiscoveredEndpoint> {
    let mut target = normalize_allowed_url(base, action, true, scope)?;
    let is_post = method_raw.trim().eq_ignore_ascii_case("post");
    let encoded = {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in fields {
            ser.append_pair(k, v);
        }
        ser.finish()
    };
    if is_post {
        let body = encoded.into_bytes();
        let headers = vec![
            Header {
                name: "Host".into(),
                value: host_header_value(&target),
            },
            Header {
                name: "Content-Type".into(),
                value: b"application/x-www-form-urlencoded".to_vec(),
            },
            Header {
                name: "Content-Length".into(),
                value: body.len().to_string().into_bytes(),
            },
        ];
        let url_str = target.to_string();
        if !scope_permits_url(scope, &url_str) {
            return None;
        }
        Some(DiscoveredEndpoint {
            url: url_str.clone(),
            method: "POST".into(),
            source: DiscoverySource::FormSubmission,
            depth: 1,
            request: HttpRequest {
                method: "POST".into(),
                url: url_str,
                version: HttpVersion::Http11,
                headers,
                body,
                trailers: Vec::new(),
            },
        })
    } else {
        if !encoded.is_empty() {
            target.set_query(Some(&encoded));
        }
        if !scope_permits_url(scope, target.as_str()) {
            return None;
        }
        let mut ep = build_get_endpoint(target, DiscoverySource::FormSubmission);
        ep.depth = 1;
        Some(ep)
    }
}

pub fn discover_same_origin_links_and_forms(
    base_url: &str,
    response: &HttpResponse,
    scope: &TargetScope,
) -> Vec<DiscoveredEndpoint> {
    let Ok(mut base) = Url::parse(base_url.trim()) else {
        return Vec::new();
    };
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
        return Vec::new();
    }
    base.set_fragment(None);

    let mut out = Vec::new();
    let mut seen: html_json::EndpointKeySet = HashSet::new();

    for h in &response.headers {
        if h.name.eq_ignore_ascii_case("location")
            && let Ok(loc) = std::str::from_utf8(&h.value)
            && let Some(url) = normalize_allowed_url(&base, loc, false, scope)
            && out.len() < MAX_DISCOVERED_PER_PAGE
        {
            let ep = build_get_endpoint(url, DiscoverySource::RedirectLocation);
            if seen.insert((ep.method.clone(), ep.url.clone(), Vec::new())) {
                out.push(ep);
            }
        }
    }

    let body_slice = &response.body[..response.body.len().min(MAX_DISCOVERY_BODY_BYTES)];
    if body_slice.is_empty() || out.len() >= MAX_DISCOVERED_PER_PAGE {
        return out;
    }

    let ct = response
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| String::from_utf8_lossy(&h.value).to_ascii_lowercase())
        .unwrap_or_default();
    let trimmed = body_slice.trim_ascii_start();

    if ct.contains("json") || (ct.is_empty() && matches!(trimmed.first(), Some(b'{' | b'['))) {
        let clean = body_slice
            .strip_prefix(b"\xef\xbb\xbf")
            .unwrap_or(body_slice);
        if let Ok(json_val) = serde_json::from_slice::<Value>(clean) {
            let mut nodes = 0usize;
            extract_json_endpoints(&json_val, &base, scope, 0, &mut nodes, &mut seen, &mut out);
            return out;
        }
    }

    if ct.contains("html") || ct.contains("xhtml") || (ct.is_empty() && trimmed.starts_with(b"<")) {
        let html = String::from_utf8_lossy(body_slice);
        extract_html_endpoints(&html, &base, scope, &mut seen, &mut out);
    }
    out
}

fn crawl_authorized(url: &Url, scope: &TargetScope) -> bool {
    if is_excluded_by_scope(scope, url.as_str()) {
        return false;
    }
    if scope.has_enabled_includes() {
        scope.is_url_in_scope(url.as_str())
    } else {
        is_loopback_host(url.host_str())
    }
}

use crate::codec_util::now_unix_ms;

pub(crate) fn apply_same_origin_context_headers(req: &mut HttpRequest, seed_headers: &[Header]) {
    for h in seed_headers {
        if (h.name.eq_ignore_ascii_case("cookie")
            || h.name.eq_ignore_ascii_case("authorization")
            || h.name.eq_ignore_ascii_case("user-agent"))
            && !req
                .headers
                .iter()
                .any(|existing| existing.name.eq_ignore_ascii_case(&h.name))
        {
            req.headers.push(h.clone());
        }
    }
}

pub async fn crawl_in_scope_endpoints(
    seed_url: &str,
    scope: &TargetScope,
    crawl_config: &CrawlConfig,
    send_config: &SendConfig,
    cancel: &CancellationToken,
) -> Result<CrawlReport> {
    crawl_in_scope_endpoints_with_headers(seed_url, &[], scope, crawl_config, send_config, cancel)
        .await
}

pub async fn crawl_in_scope_endpoints_with_headers(
    seed_url: &str,
    seed_headers: &[Header],
    scope: &TargetScope,
    crawl_config: &CrawlConfig,
    send_config: &SendConfig,
    cancel: &CancellationToken,
) -> Result<CrawlReport> {
    ensure!(
        (1..=MAX_CRAWL_DEPTH).contains(&crawl_config.max_depth),
        "目录遍历最大深度必须在 1..={MAX_CRAWL_DEPTH} 之间"
    );
    ensure!(
        (1..=MAX_CRAWL_REQUESTS).contains(&crawl_config.max_requests),
        "目录遍历最大总请求数必须在 1..={MAX_CRAWL_REQUESTS} 之间"
    );
    let mut seed = Url::parse(seed_url.trim()).context("起始 URL 格式无效")?;
    ensure!(
        matches!(seed.scheme(), "http" | "https") && seed.host_str().is_some(),
        "起始 URL 必须为 http 或 https 协议"
    );
    seed.set_fragment(None);
    ensure!(
        crawl_authorized(&seed, scope),
        "同域目录遍历仅允许 127.0.0.1/localhost 或显式包含在 TargetScope 内的目标"
    );

    let mut report = CrawlReport {
        endpoints: Vec::new(),
        flows: Vec::new(),
        findings: Vec::new(),
        requests_sent: 0,
        cancelled: false,
    };
    let mut queue: VecDeque<(Url, u8)> = VecDeque::new();
    let mut visited_get: HashSet<String> = HashSet::new();
    let mut recorded_endpoints: HashSet<(String, String, Vec<u8>)> = HashSet::new();

    visited_get.insert(seed.to_string());
    queue.push_back((seed.clone(), 0));

    while let Some((current, depth)) = queue.pop_front() {
        if cancel.is_cancelled() {
            report.cancelled = true;
            break;
        }
        if report.requests_sent >= crawl_config.max_requests {
            break;
        }
        let mut req = build_get_endpoint(current.clone(), DiscoverySource::AnchorHref).request;
        apply_same_origin_context_headers(&mut req, seed_headers);
        let started_ms = now_unix_ms();
        let timer = Instant::now();
        let send_res = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                report.cancelled = true;
                break;
            }
            res = send(req.clone(), send_config.clone(), cancel.child_token()) => res,
        };
        let resp = match send_res {
            Ok(resp) => {
                report.requests_sent += 1;
                resp
            }
            Err(_) if cancel.is_cancelled() => {
                report.cancelled = true;
                break;
            }
            Err(err) => {
                if report.requests_sent == 0 {
                    return Err(anyhow::anyhow!("抓取起始页面失败：{err}"));
                }
                report.requests_sent += 1;
                continue;
            }
        };
        let elapsed = timer.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let flow = FlowRecord {
            summary: FlowSummary {
                id: Uuid::new_v4(),
                revision: 1,
                source: TrafficSource::Scanner,
                method: req.method.clone(),
                url: req.url.clone(),
                status: Some(resp.status),
                state: FlowState::Complete,
                request_bytes: req.body.len() as u64,
                response_bytes: resp.body.len() as u64,
                started_unix_ms: started_ms,
                elapsed_ms: elapsed,
            },
            request: req,
            response: Some(resp.clone()),
        };
        let passive_hits = inspect_flow_passive(&flow, now_unix_ms());
        let _ = merge_audit_findings(&mut report.findings, passive_hits);
        report.flows.push(flow);

        let next_depth = depth.saturating_add(1);
        for mut ep in discover_same_origin_links_and_forms(current.as_str(), &resp, scope) {
            ep.depth = next_depth;
            let Ok(ep_url) = Url::parse(&ep.url) else {
                continue;
            };
            if !same_origin(&seed, &ep_url) || !crawl_authorized(&ep_url, scope) {
                continue;
            }
            apply_same_origin_context_headers(&mut ep.request, seed_headers);
            if recorded_endpoints.insert((
                ep.method.clone(),
                ep.url.clone(),
                ep.request.body.clone(),
            )) {
                report.endpoints.push(ep.clone());
            }
            if next_depth <= crawl_config.max_depth
                && ep.method == "GET"
                && visited_get.insert(ep.url.clone())
            {
                queue.push_back((ep_url, next_depth));
            }
        }
    }
    Ok(report)
}
