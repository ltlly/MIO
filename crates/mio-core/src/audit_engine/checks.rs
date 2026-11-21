use anyhow::{Result, ensure};
use url::Url;

use super::*;
use crate::target_issues::{IssueSeverity, issue_definitions};
use crate::{Header, HttpRequest, HttpResponse, request_to_raw, response_to_raw};

const MAX_SCAN_BODY_BYTES: usize = 256 * 1024;

impl AuditFinding {
    pub fn severity(&self) -> IssueSeverity {
        let all = IssueSeverity::ALL;
        let found = all.into_iter().find(|s| s.rank() == self.severity_rank);
        found.unwrap_or(IssueSeverity::Information)
    }
}

fn hdr<'a>(hs: &'a [Header], k: &str) -> Option<&'a str> {
    let h = hs.iter().find(|h| h.name.eq_ignore_ascii_case(k))?;
    Some(std::str::from_utf8(&h.value).ok()?.trim())
}

pub(super) fn spans(b: &[u8], n: &[u8]) -> Vec<EvidenceSpan> {
    let pos = (!n.is_empty()).then(|| b.windows(n.len()).position(|w| w == n));
    let Some(offset) = pos.flatten() else {
        return vec![];
    };
    let length = n.len();
    vec![EvidenceSpan { offset, length }]
}

fn meta(code: u32, p: AuditPhase) -> (&'static str, IssueConfidence, &'static str) {
    use IssueConfidence::{Certain as C, Firm as F, Tentative as T};
    match code {
        0x01000300 => ("0x01000300", C, "HTTPS 缺少 HSTS。"),
        0x005009a0 => ("0x005009a0", F, "HTML 缺少抗点击劫持。"),
        0x00500600 => ("0x00500600", C, "Cookie 缺 HttpOnly。"),
        0x00500200 => ("0x00500200", C, "HTTPS Cookie 缺 Secure。"),
        0x00500601 => ("0x00500601", C, "Cookie 缺少安全 SameSite 属性。"),
        0x00700100 => ("0x00700100", F, "敏感响应缺少 Cache-Control: no-store。"),
        0x01000400 => ("0x01000400", C, "HTTPS HTML 引用明文 http:// 脚本或表单。"),
        0x005009b0 => (
            "0x005009b0",
            C,
            "响应缺少 X-Content-Type-Options: nosniff。",
        ),
        0x005009c0 => ("0x005009c0", C, "响应缺少 Referrer-Policy。"),
        0x00200601 => ("0x00200601", C, "CORS 任意源+凭据。"),
        0x00200602 => (
            "0x00200602",
            C,
            "CORS 信任 null 源且开启 Access-Control-Allow-Credentials。",
        ),
        0x00200903 => ("0x00200903", C, "检测到 alg=none 未签名 JWT 令牌。"),
        0x00300100 => ("0x00300100", F, "HTTP 明文提交凭据。"),
        0x00600100 => ("0x00600100", F, "目录列表开启。"),
        0x00200300 => ("0x00200300", F, "无害 canary 原样反射。"),
        0x00500100 => ("0x00500100", F, "开放重定向。"),
        0x00200200 => ("0x00200200", C, "响应头拆分。"),
        0x00100200 => ("0x00100200", T, "新增数据库异常。"),
        _ if p == AuditPhase::Passive => ("0x00600080", F, "私钥或数据库诊断泄露。"),
        _ => ("0x00600080", T, "新增 5xx 异常差分。"),
    }
}

fn response_to_raw_capped(resp: &HttpResponse) -> Vec<u8> {
    if resp.body.len() <= MAX_SCAN_BODY_BYTES {
        return response_to_raw(resp);
    }
    let capped = HttpResponse {
        status: resp.status,
        version: resp.version,
        reason_phrase: resp.reason_phrase.clone(),
        headers: resp.headers.clone(),
        body: resp.body[..MAX_SCAN_BODY_BYTES].to_vec(),
        trailers: resp.trailers.clone(),
    };
    response_to_raw(&capped)
}

pub(super) struct Bag<'a> {
    pub(super) req: &'a HttpRequest,
    pub(super) resp: &'a HttpResponse,
    pub(super) param: Option<&'a str>,
    pub(super) flow: Option<FlowId>,
    pub(super) phase: AuditPhase,
    pub(super) rqs: Vec<EvidenceSpan>,
    pub(super) raw: Option<Vec<u8>>,
    pub(super) now: i64,
    pub(super) out: Vec<AuditFinding>,
}

impl Bag<'_> {
    pub(super) fn hit_detail(
        &mut self,
        code: u32,
        summary_override: Option<&str>,
        ev: &str,
        hl: bool,
    ) {
        let (id, confidence, default_summary) = meta(code, self.phase);
        let def = issue_definitions().iter().find(|d| d.id == id);
        let sev = def.map_or(IssueSeverity::Information, |d| d.typical_severity);
        let host = Url::parse(&self.req.url)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_default();
        let response_highlights = if hl {
            let resp = self.resp;
            let raw = self.raw.get_or_insert_with(|| response_to_raw_capped(resp));
            spans(raw, ev.as_bytes())
        } else {
            vec![]
        };
        self.out.push(AuditFinding {
            id: Uuid::new_v4(),
            issue_id: id.into(),
            title: def.map_or(id, |d| d.name).into(),
            severity_label: sev.label().into(),
            severity_rank: sev.rank(),
            confidence,
            phase: self.phase,
            host,
            url: self.req.url.clone(),
            parameter: self.param.map(String::from),
            flow_id: self.flow,
            summary: summary_override.unwrap_or(default_summary).into(),
            evidence: ev.chars().take(512).collect(),
            request_highlights: self.rqs.clone(),
            response_highlights,
            remediation: def.map_or("按最小权限修复。", |d| d.remediation).into(),
            detected_unix_ms: self.now,
        });
    }

    fn hit(&mut self, ok: bool, code: u32, ev: &str, hl: bool) {
        if !ok {
            return;
        }
        self.hit_detail(code, None, ev, hl);
    }
}

pub fn inspect_passive(
    flow_id: Option<FlowId>,
    req: &HttpRequest,
    resp: &HttpResponse,
    now_unix_ms: i64,
) -> Vec<AuditFinding> {
    let Ok(url) = Url::parse(&req.url) else {
        return vec![];
    };
    let (https, hs) = (url.scheme() == "https", &resp.headers);
    let body = String::from_utf8_lossy(&resp.body[..resp.body.len().min(MAX_SCAN_BODY_BYTES)]);
    let body = body.as_ref();
    let mut b = Bag {
        req,
        resp,
        param: None,
        flow: flow_id,
        phase: AuditPhase::Passive,
        rqs: vec![],
        raw: None,
        now: now_unix_ms,
        out: vec![],
    };
    b.hit(
        https && hdr(hs, "strict-transport-security").is_none(),
        0x01000300,
        &req.url,
        false,
    );
    let html = hdr(hs, "content-type")
        .is_some_and(|v| v.to_ascii_lowercase().contains("text/html"))
        && body.contains('<');
    let no_frame = hdr(hs, "x-frame-options").is_none()
        && !hdr(hs, "content-security-policy")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("frame-ancestors");
    b.hit(html && no_frame, 0x005009a0, "<", true);
    for h in hs {
        if h.name.eq_ignore_ascii_case("set-cookie")
            && let Ok(t) = std::str::from_utf8(&h.value)
        {
            let lo = t.to_ascii_lowercase();
            b.hit(!lo.contains("httponly"), 0x00500600, t, true);
            b.hit(https && !lo.contains("secure"), 0x00500200, t, true);
        }
    }
    let cors = hdr(hs, "access-control-allow-origin")
        .is_some_and(|v| v == "*" || hdr(&req.headers, "origin") == Some(v));
    let cred =
        hdr(hs, "access-control-allow-credentials").is_some_and(|v| v.eq_ignore_ascii_case("true"));
    b.hit(
        cors && cred,
        0x00200601,
        "Access-Control-Allow-Credentials",
        true,
    );
    if !https {
        let rb = String::from_utf8_lossy(&req.body[..req.body.len().min(MAX_SCAN_BODY_BYTES)]);
        if rb.to_ascii_lowercase().contains("password=")
            || hdr(&req.headers, "authorization").is_some()
        {
            b.rqs = spans(&request_to_raw(req), b"password=");
            b.hit(true, 0x00300100, &rb, false);
            b.rqs.clear();
        }
    }
    b.hit(body.contains("Index of /"), 0x00600100, "Index of /", true);
    if let Some(m) = "BEGIN PRIVATE KEY|BEGIN RSA PRIVATE KEY|jdbc:|mysql_connect|SQLSTATE["
        .split('|')
        .find(|m| body.contains(m))
    {
        b.hit(true, 0x00600080, m, true);
    }
    let cors_null =
        hdr(hs, "access-control-allow-origin").is_some_and(|v| v.eq_ignore_ascii_case("null"));
    b.hit(
        cors_null && cred,
        0x00200602,
        "Access-Control-Allow-Origin: null",
        true,
    );
    if let Some((tok, in_req)) = find_alg_none_jwt(req, resp, body) {
        if in_req {
            b.rqs = spans(&request_to_raw(req), tok.as_bytes());
        }
        b.hit(true, 0x00200903, &tok, !in_req);
        b.rqs.clear();
    }
    super::linter::inspect_passive_compliance(&mut b, https, resp);
    b.out
}

fn find_alg_none_jwt(req: &HttpRequest, resp: &HttpResponse, body: &str) -> Option<(String, bool)> {
    for h in &req.headers {
        if (h.name.eq_ignore_ascii_case("authorization") || h.name.eq_ignore_ascii_case("cookie"))
            && let Ok(v) = std::str::from_utf8(&h.value)
            && let Some(t) = scan_alg_none_jwt_in_text(v)
        {
            return Some((t, true));
        }
    }
    for h in &resp.headers {
        if h.name.eq_ignore_ascii_case("set-cookie")
            && let Ok(v) = std::str::from_utf8(&h.value)
            && let Some(t) = scan_alg_none_jwt_in_text(v)
        {
            return Some((t, false));
        }
    }
    scan_alg_none_jwt_in_text(body).map(|t| (t, false))
}

fn scan_alg_none_jwt_in_text(text: &str) -> Option<String> {
    for raw in text.split(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_' | '.')) {
        let mut parts = raw.split('.');
        let (Some(hdr_b64), Some(pay_b64), Some(_sig)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if parts.next().is_some() || !hdr_b64.starts_with("eyJ") || pay_b64.len() < 4 {
            continue;
        }
        if let Some(decoded) = decode_b64url_prefix(hdr_b64)
            && let Ok(json_str) = std::str::from_utf8(&decoded)
        {
            let compact: String = json_str
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            if compact.contains("\"alg\":\"none\"") {
                return Some(raw.to_owned());
            }
        }
    }
    None
}

fn decode_b64url_prefix(s: &str) -> Option<Vec<u8>> {
    let clean = s.trim_end_matches('=');
    if clean.is_empty() || clean.len() > 256 || clean.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for b in clean.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

pub fn extract_insertion_points(req: &HttpRequest) -> Vec<InsertionPoint> {
    super::insertion_points::extract_structured_parameters(req)
}

fn url_encode(val: &str) -> String {
    let mut s = String::with_capacity(val.len());
    for b in val.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn replace_pair(raw: &str, pt: &InsertionPoint, enc: &str) -> Result<String> {
    let (mut seen, mut ok, mut out) = (0usize, false, Vec::new());
    for pair in raw.split('&') {
        let (k, _) = pair.split_once('=').unwrap_or((pair, ""));
        let eq = k == pt.name && seen == pt.occurrence;
        seen += usize::from(k == pt.name);
        ok |= eq;
        let item = if eq {
            format!("{k}={enc}")
        } else {
            pair.into()
        };
        out.push(item);
    }
    ensure!(ok, "插入点不存在: {}", pt.name);
    Ok(out.join("&"))
}

pub fn build_probe_request(
    req: &HttpRequest,
    point: &InsertionPoint,
    value: &str,
) -> Result<HttpRequest> {
    if !matches!(
        point.location,
        InsertionLocation::Query | InsertionLocation::Form
    ) {
        return super::insertion_points::rebuild_structured_parameter(req, point, value);
    }
    let (mut next, enc) = (req.clone(), url_encode(value));
    if point.location == InsertionLocation::Query {
        let (h, f) = match req.url.split_once('#') {
            Some((a, b)) => (a, Some(b)),
            None => (req.url.as_str(), None),
        };
        let (base, q0) = h
            .split_once('?')
            .ok_or_else(|| anyhow::anyhow!("缺少 Query"))?;
        let q = replace_pair(q0, point, &enc)?;
        next.url = f.map_or_else(|| format!("{base}?{q}"), |f| format!("{base}?{q}#{f}"));
    } else {
        next.body = replace_pair(std::str::from_utf8(&req.body)?, point, &enc)?.into_bytes();
        let len = next.body.len().to_string().into_bytes();
        for h in &mut next.headers {
            if h.name.eq_ignore_ascii_case("content-length") {
                h.value.clone_from(&len);
            }
        }
    }
    Ok(next)
}

#[allow(clippy::too_many_arguments)]
pub fn evaluate_active_check(
    flow_id: Option<FlowId>,
    req: &HttpRequest,
    point: &InsertionPoint,
    payload: &str,
    base: &HttpResponse,
    probe: &HttpResponse,
    now_unix_ms: i64,
) -> Vec<AuditFinding> {
    if base.status == probe.status && base.headers == probe.headers && base.body == probe.body {
        return vec![];
    }
    let bb = String::from_utf8_lossy(&base.body[..base.body.len().min(MAX_SCAN_BODY_BYTES)]);
    let pb = String::from_utf8_lossy(&probe.body[..probe.body.len().min(MAX_SCAN_BODY_BYTES)]);
    let raw_req = request_to_raw(req);
    let mut rqs = spans(&raw_req, url_encode(payload).as_bytes());
    if rqs.is_empty() && !payload.is_empty() {
        rqs = spans(&raw_req, payload.as_bytes());
    }
    if rqs.is_empty() && payload.contains(ACTIVE_CANARY) {
        rqs = spans(&raw_req, ACTIVE_CANARY.as_bytes());
    }
    let mut b = Bag {
        req,
        resp: probe,
        param: Some(&point.name),
        flow: flow_id,
        phase: AuditPhase::Active,
        rqs,
        raw: None,
        now: now_unix_ms,
        out: vec![],
    };
    let c = format!("<{ACTIVE_CANARY}>");
    b.hit(
        payload.contains(&c) && pb.contains(&c) && !bb.contains(&c),
        0x00200300,
        &c,
        true,
    );
    if let Some(loc) = hdr(&probe.headers, "location") {
        let ok = (301..=308).contains(&probe.status)
            && loc.starts_with(ACTIVE_REDIRECT_TARGET)
            && hdr(&base.headers, "location") != Some(loc);
        b.hit(ok, 0x00500100, loc, true);
    }
    b.hit(
        hdr(&probe.headers, ACTIVE_SPLIT_HEADER).is_some()
            && hdr(&base.headers, ACTIVE_SPLIT_HEADER).is_none(),
        0x00200200,
        ACTIVE_SPLIT_HEADER,
        true,
    );
    if let Some(m) = "SQLSTATE[|You have an error in your SQL syntax|ORA-01756|PG::SyntaxError"
        .split('|')
        .find(|m| pb.contains(m) && !bb.contains(m))
    {
        b.hit(true, 0x00100200, m, true);
    }
    let diff = format!("{}->{}", base.status, probe.status);
    b.hit(
        b.out.is_empty() && base.status < 500 && probe.status >= 500,
        0x00600080,
        &diff,
        false,
    );
    b.out
}

pub fn evaluate_custom_scan_checks(
    flow_id: Option<FlowId>,
    req: &HttpRequest,
    resp: &HttpResponse,
    checks: &[crate::CustomScanCheck],
    phase: AuditPhase,
    now_unix_ms: i64,
) -> Vec<AuditFinding> {
    let mut findings = Vec::new();
    let mut raw_resp: Option<Vec<u8>> = None;
    let host = Url::parse(&req.url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default();
    for check in checks.iter().filter(|c| c.enabled && !c.script.trim().is_empty()) {
        if let Ok(Some(evidence)) =
            crate::http::eval_custom_scan_check_script(&check.script, req, resp)
        {
            let sev: IssueSeverity = check.severity.into();
            let raw = raw_resp.get_or_insert_with(|| response_to_raw_capped(resp));
            findings.push(AuditFinding {
                id: Uuid::new_v4(),
                issue_id: format!("custom:{}", check.id),
                title: if check.name.trim().is_empty() {
                    "自定义扫描检查".into()
                } else {
                    check.name.trim().to_owned()
                },
                severity_label: sev.label().into(),
                severity_rank: sev.rank(),
                confidence: check.confidence,
                phase,
                host: host.clone(),
                url: req.url.clone(),
                parameter: None,
                flow_id,
                summary: if check.description.trim().is_empty() {
                    format!("命中自定义扫描检查：{}", check.name)
                } else {
                    check.description.trim().to_owned()
                },
                evidence: evidence.chars().take(512).collect(),
                request_highlights: Vec::new(),
                response_highlights: spans(raw, evidence.as_bytes()),
                remediation: "请根据自定义扫描检查逻辑复核目标响应与安全控制。".into(),
                detected_unix_ms: now_unix_ms,
            });
        }
    }
    findings
}
