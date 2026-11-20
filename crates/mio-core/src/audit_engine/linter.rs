use super::checks::Bag;
use super::{AuditFinding, EvidenceSpan, inspect_passive};
use crate::{FlowRecord, Header, HttpResponse, request_to_raw};

const MAX_PASSIVE_HTML_BYTES: usize = 64 * 1024;

pub fn inspect_flow_passive(record: &FlowRecord, now_unix_ms: i64) -> Vec<AuditFinding> {
    let Some(resp) = record.response.as_ref() else {
        return Vec::new();
    };
    inspect_passive(Some(record.summary.id), &record.request, resp, now_unix_ms)
}

fn spans_ci(haystack: &[u8], needle: &[u8]) -> Vec<EvidenceSpan> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    let Some(offset) = haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
    else {
        return Vec::new();
    };
    vec![EvidenceSpan {
        offset,
        length: needle.len(),
    }]
}

fn cookie_samesite_violation(cookie_header: &str) -> Option<&'static str> {
    let mut parts = cookie_header.split(';');
    let name_pair = parts.next()?.trim();
    if name_pair.is_empty() {
        return None;
    }
    let mut samesite: Option<&str> = None;
    let mut secure = false;
    for raw_attr in parts {
        let attr = raw_attr.trim();
        if attr.is_empty() {
            continue;
        }
        if attr.eq_ignore_ascii_case("secure") {
            secure = true;
        } else if let Some((k, v)) = attr.split_once('=') {
            if k.trim().eq_ignore_ascii_case("samesite") {
                samesite = Some(v.trim().trim_matches('"'));
            }
        } else if attr.eq_ignore_ascii_case("samesite") {
            samesite = Some("");
        }
    }
    match samesite {
        None => Some("Set-Cookie 缺少 SameSite 属性。"),
        Some(v) if v.eq_ignore_ascii_case("none") && !secure => {
            Some("Set-Cookie 设置 SameSite=None 但缺少 Secure 标志。")
        }
        Some(v)
            if v.eq_ignore_ascii_case("strict")
                || v.eq_ignore_ascii_case("lax")
                || v.eq_ignore_ascii_case("none") =>
        {
            None
        }
        Some(_) => Some("Set-Cookie 包含无效的 SameSite 属性值。"),
    }
}

fn has_cache_control_no_store(headers: &[Header]) -> bool {
    headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("cache-control"))
        .filter_map(|h| std::str::from_utf8(&h.value).ok())
        .flat_map(|v| v.split(','))
        .any(|directive| {
            let token = directive.split('=').next().unwrap_or("").trim();
            token.eq_ignore_ascii_case("no-store")
        })
}

fn is_html_response(resp: &HttpResponse) -> bool {
    let mut saw_ct = false;
    for h in &resp.headers {
        if h.name.eq_ignore_ascii_case("content-type") {
            saw_ct = true;
            let ct = String::from_utf8_lossy(&h.value).to_ascii_lowercase();
            if ct.contains("text/html") || ct.contains("application/xhtml+xml") {
                return true;
            }
        }
    }
    if saw_ct {
        return false;
    }
    let prefix = String::from_utf8_lossy(&resp.body[..resp.body.len().min(256)]);
    let trimmed = prefix.trim_start().to_ascii_lowercase();
    trimmed.starts_with("<!doctype html") || trimmed.starts_with("<html")
}

fn find_tag_end(rest: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (idx, ch) in rest.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch == '>' {
            return Some(idx);
        }
    }
    None
}

fn has_cleartext_http_attr(mut attrs: &str, target_attr: &str) -> bool {
    loop {
        attrs = attrs.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if attrs.is_empty() {
            return false;
        }
        let name_end = attrs
            .find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '/' | '"' | '\''))
            .unwrap_or(attrs.len());
        if name_end == 0 {
            let step = attrs.chars().next().map_or(1, char::len_utf8);
            attrs = &attrs[step..];
            continue;
        }
        let name = &attrs[..name_end];
        attrs = attrs[name_end..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        let Some(after_eq) = attrs.strip_prefix('=') else {
            continue;
        };
        attrs = after_eq.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let val = if let Some(q) = attrs.chars().next().filter(|&c| c == '"' || c == '\'') {
            let inner = &attrs[q.len_utf8()..];
            match inner.find(q) {
                Some(end) => {
                    attrs = &inner[end + q.len_utf8()..];
                    &inner[..end]
                }
                None => {
                    attrs = "";
                    inner
                }
            }
        } else {
            let end = attrs
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(attrs.len());
            let v = &attrs[..end];
            attrs = &attrs[end..];
            v
        };
        if name.eq_ignore_ascii_case(target_attr)
            && val
                .trim_start()
                .get(..7)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        {
            return true;
        }
    }
}

pub(super) fn find_mixed_content_html(html: &str) -> Option<&str> {
    let mut cursor = 0usize;
    while let Some(rel_open) = html[cursor..].find('<') {
        let tag_start = cursor + rel_open;
        let from_open = &html[tag_start..];
        if let Some(after_comment) = from_open.strip_prefix("<!--") {
            let Some(end_comment) = after_comment.find("-->") else {
                break;
            };
            cursor = tag_start + 4 + end_comment + 3;
            continue;
        }
        let inside_start = tag_start + 1;
        let Some(rel_close) = find_tag_end(&html[inside_start..]) else {
            break;
        };
        let tag_end = inside_start + rel_close + 1;
        let inside = html[inside_start..tag_end - 1].trim_start();
        let name_len = inside
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(inside.len());
        let tag_name = &inside[..name_len];
        let target_attr = if tag_name.eq_ignore_ascii_case("script") {
            Some("src")
        } else if tag_name.eq_ignore_ascii_case("form") {
            Some("action")
        } else {
            None
        };
        if let Some(attr) = target_attr
            && has_cleartext_http_attr(&inside[name_len..], attr)
        {
            return Some(&html[tag_start..tag_end]);
        }
        cursor = tag_end;
    }
    None
}

pub(super) fn inspect_passive_compliance(b: &mut Bag<'_>, https: bool, resp: &HttpResponse) {
    let hs = &resp.headers;
    for h in hs {
        if h.name.eq_ignore_ascii_case("set-cookie") {
            let val = String::from_utf8_lossy(&h.value);
            let trimmed = val.trim();
            if let Some(reason) = cookie_samesite_violation(trimmed) {
                b.hit_detail(0x00500601, Some(reason), trimmed, true);
            }
        }
    }

    let has_req_auth = b.req.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("authorization") && !h.value.iter().all(u8::is_ascii_whitespace)
    });
    let first_cookie = hs
        .iter()
        .find(|h| {
            h.name.eq_ignore_ascii_case("set-cookie")
                && !h.value.iter().all(u8::is_ascii_whitespace)
        })
        .map(|h| String::from_utf8_lossy(&h.value).trim().to_owned());
    if (has_req_auth || first_cookie.is_some()) && !has_cache_control_no_store(hs) {
        if has_req_auth {
            b.rqs = spans_ci(&request_to_raw(b.req), b"authorization:");
        }
        let existing_cc = hs
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("cache-control"))
            .map(|h| String::from_utf8_lossy(&h.value).trim().to_owned())
            .filter(|v| !v.is_empty());
        if let Some(cc) = existing_cc {
            b.hit_detail(
                0x00700100,
                Some("敏感响应 Cache-Control 未包含 no-store。"),
                &cc,
                true,
            );
        } else if let Some(cookie_ev) = first_cookie {
            b.hit_detail(
                0x00700100,
                Some("下发 Set-Cookie 的响应缺少 Cache-Control: no-store。"),
                &cookie_ev,
                true,
            );
        } else {
            b.hit_detail(
                0x00700100,
                Some("含 Authorization 请求的敏感响应缺少 Cache-Control: no-store。"),
                "Authorization (missing Cache-Control: no-store)",
                false,
            );
        }
        b.rqs.clear();
    }

    if https && is_html_response(resp) {
        let slice = &resp.body[..resp.body.len().min(MAX_PASSIVE_HTML_BYTES)];
        let html = String::from_utf8_lossy(slice);
        if let Some(snippet) = find_mixed_content_html(&html) {
            b.hit_detail(0x01000400, None, snippet, true);
        }
    }

    if !matches!(resp.status, 100..=199 | 204 | 304) {
        let has_content_type = hs.iter().any(|h| {
            h.name.eq_ignore_ascii_case("content-type")
                && !h.value.iter().all(u8::is_ascii_whitespace)
        });
        let html_or_redirect =
            is_html_response(resp) || matches!(resp.status, 301 | 302 | 303 | 307 | 308);
        let has_nosniff = hs.iter().any(|h| {
            h.name.eq_ignore_ascii_case("x-content-type-options")
                && String::from_utf8_lossy(&h.value)
                    .trim()
                    .eq_ignore_ascii_case("nosniff")
        });
        if !has_nosniff {
            let bad_val = hs
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("x-content-type-options"))
                .map(|h| String::from_utf8_lossy(&h.value).trim().to_owned())
                .filter(|v| !v.is_empty());
            if let Some(val) = bad_val {
                b.hit_detail(
                    0x005009b0,
                    Some("X-Content-Type-Options 未设置为 nosniff。"),
                    &val,
                    true,
                );
            } else if has_content_type || html_or_redirect {
                let url = b.req.url.clone();
                b.hit_detail(0x005009b0, None, &url, false);
            }
        }

        let has_referrer_policy = hs.iter().any(|h| {
            h.name.eq_ignore_ascii_case("referrer-policy")
                && !String::from_utf8_lossy(&h.value).trim().is_empty()
        });
        if !has_referrer_policy && html_or_redirect {
            let url = b.req.url.clone();
            b.hit_detail(0x005009c0, None, &url, false);
        }
    }
}
