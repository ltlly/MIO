use std::collections::HashSet;

use serde_json::Value;
use url::Url;

use super::{
    DiscoveredEndpoint, DiscoverySource, MAX_DISCOVERED_PER_PAGE, build_form_endpoint,
    build_get_endpoint, normalize_allowed_url,
};
use crate::TargetScope;

const MAX_JSON_WALK_DEPTH: usize = 32;
const MAX_JSON_WALK_NODES: usize = 4096;

pub(super) type EndpointKey = (String, String, Vec<u8>);
pub(super) type EndpointKeySet = HashSet<EndpointKey>;

struct PendingForm {
    action: String,
    method: String,
    fields: Vec<(String, String)>,
}

impl PendingForm {
    fn flush(
        self,
        base: &Url,
        scope: &TargetScope,
        seen: &mut EndpointKeySet,
        out: &mut Vec<DiscoveredEndpoint>,
    ) {
        if let Some(ep) = build_form_endpoint(base, scope, &self.action, &self.method, &self.fields)
            && seen.insert((ep.method.clone(), ep.url.clone(), ep.request.body.clone()))
        {
            out.push(ep);
        }
    }
}

fn decode_html_attr_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    raw.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn find_tag_close(rest: &str) -> Option<usize> {
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

fn find_ci(haystack: &str, needle_lower: &str) -> Option<usize> {
    if needle_lower.is_empty() || haystack.len() < needle_lower.len() {
        return None;
    }
    haystack
        .as_bytes()
        .windows(needle_lower.len())
        .position(|w| w.eq_ignore_ascii_case(needle_lower.as_bytes()))
}

fn parse_tag_attrs(mut attrs: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    loop {
        attrs = attrs.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if attrs.is_empty() || out.len() >= 64 {
            break;
        }
        let name_end = attrs
            .find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '/' | '"' | '\''))
            .unwrap_or(attrs.len());
        if name_end == 0 {
            let step = attrs.chars().next().map_or(1, char::len_utf8);
            attrs = &attrs[step..];
            continue;
        }
        let name = attrs[..name_end].to_ascii_lowercase();
        attrs = attrs[name_end..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        let Some(after_eq) = attrs.strip_prefix('=') else {
            out.push((name, String::new()));
            continue;
        };
        attrs = after_eq.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let raw_val = if let Some(q) = attrs.chars().next().filter(|&c| c == '"' || c == '\'') {
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
        out.push((name, decode_html_attr_entities(raw_val)));
    }
    out
}

fn attr_val<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn record_get(
    url: Url,
    source: DiscoverySource,
    seen: &mut EndpointKeySet,
    out: &mut Vec<DiscoveredEndpoint>,
) {
    let ep = build_get_endpoint(url, source);
    if seen.insert(("GET".into(), ep.url.clone(), Vec::new())) {
        out.push(ep);
    }
}

pub(super) fn extract_html_endpoints(
    html: &str,
    base: &Url,
    scope: &TargetScope,
    seen: &mut EndpointKeySet,
    out: &mut Vec<DiscoveredEndpoint>,
) {
    let mut cursor = 0usize;
    let mut active_form: Option<PendingForm> = None;
    let mut active_select: Option<(String, Option<String>, bool)> = None;

    while cursor < html.len() && out.len() < MAX_DISCOVERED_PER_PAGE {
        let Some(rel_open) = html[cursor..].find('<') else {
            break;
        };
        let tag_start = cursor + rel_open;
        let rest = &html[tag_start..];
        if let Some(after_comment) = rest.strip_prefix("<!--") {
            let Some(end_c) = after_comment.find("-->") else {
                break;
            };
            cursor = tag_start + 4 + end_c + 3;
            continue;
        }
        let inside_start = tag_start + 1;
        let Some(rel_close) = find_tag_close(&html[inside_start..]) else {
            break;
        };
        let tag_end = inside_start + rel_close + 1;
        let inside = html[inside_start..tag_end - 1].trim();
        cursor = tag_end;

        if let Some(closing) = inside.strip_prefix('/') {
            let close_name = closing
                .trim()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap_or_default();
            if close_name.eq_ignore_ascii_case("select") {
                if let Some((name, value, _)) = active_select.take()
                    && let Some(pf) = active_form.as_mut()
                    && pf.fields.len() < 64
                {
                    pf.fields.push((name, value.unwrap_or_default()));
                }
            } else if close_name.eq_ignore_ascii_case("form")
                && let Some(pf) = active_form.take()
            {
                pf.flush(base, scope, seen, out);
            }
            continue;
        }

        let self_closing = inside.ends_with('/');
        let name_len = inside
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(inside.len());
        let tag_name = &inside[..name_len];
        let attr_slice = &inside[name_len..];

        if tag_name.eq_ignore_ascii_case("select") {
            if let Some((name, value, _)) = active_select.take()
                && let Some(pf) = active_form.as_mut()
                && pf.fields.len() < 64
            {
                pf.fields.push((name, value.unwrap_or_default()));
            }
            if !self_closing {
                let attrs = parse_tag_attrs(attr_slice);
                let name = attr_val(&attrs, "name").unwrap_or_default().to_owned();
                active_select = Some((name, None, false));
            }
        } else if tag_name.eq_ignore_ascii_case("option")
            && let Some((_, selected, got_selected)) = active_select.as_mut()
        {
            let attrs = parse_tag_attrs(attr_slice);
            let value = attr_val(&attrs, "value").unwrap_or_default().to_owned();
            let marked = attrs.iter().any(|(k, _)| k == "selected");
            if (marked && !*got_selected) || (!*got_selected && selected.is_none()) {
                *selected = Some(value);
                *got_selected |= marked;
            }
        } else if tag_name.eq_ignore_ascii_case("a") {
            let attrs = parse_tag_attrs(attr_slice);
            if let Some(href) = attr_val(&attrs, "href")
                && let Some(url) = normalize_allowed_url(base, href, false, scope)
            {
                record_get(url, DiscoverySource::AnchorHref, seen, out);
            }
        } else if tag_name.eq_ignore_ascii_case("link") {
            let attrs = parse_tag_attrs(attr_slice);
            if let Some(href) = attr_val(&attrs, "href")
                && let Some(url) = normalize_allowed_url(base, href, false, scope)
            {
                record_get(url, DiscoverySource::LinkHref, seen, out);
            }
        } else if tag_name.eq_ignore_ascii_case("script") {
            let attrs = parse_tag_attrs(attr_slice);
            if let Some(src) = attr_val(&attrs, "src")
                && let Some(url) = normalize_allowed_url(base, src, false, scope)
            {
                record_get(url, DiscoverySource::ScriptSrc, seen, out);
            }
            if !self_closing {
                match find_ci(&html[cursor..], "</script") {
                    Some(end_script) => {
                        let after = cursor + end_script;
                        cursor = html[after..]
                            .find('>')
                            .map_or(html.len(), |gt| after + gt + 1);
                    }
                    None => break,
                }
            }
        } else if tag_name.eq_ignore_ascii_case("style") && !self_closing {
            match find_ci(&html[cursor..], "</style") {
                Some(end_style) => {
                    let after = cursor + end_style;
                    cursor = html[after..]
                        .find('>')
                        .map_or(html.len(), |gt| after + gt + 1);
                }
                None => break,
            }
        } else if tag_name.eq_ignore_ascii_case("form") {
            if let Some(pf) = active_form.take() {
                pf.flush(base, scope, seen, out);
            }
            let attrs = parse_tag_attrs(attr_slice);
            let action = attr_val(&attrs, "action").unwrap_or_default().to_owned();
            let method = attr_val(&attrs, "method").unwrap_or("GET").to_owned();
            if self_closing {
                PendingForm {
                    action,
                    method,
                    fields: Vec::new(),
                }
                .flush(base, scope, seen, out);
            } else {
                active_form = Some(PendingForm {
                    action,
                    method,
                    fields: Vec::new(),
                });
            }
        } else if tag_name.eq_ignore_ascii_case("input") {
            if let Some(pf) = active_form.as_mut()
                && pf.fields.len() < 64
            {
                let attrs = parse_tag_attrs(attr_slice);
                if let Some(name) = attr_val(&attrs, "name").map(str::trim)
                    && !name.is_empty()
                {
                    let input_type = attr_val(&attrs, "type").unwrap_or("text");
                    if !matches!(
                        input_type.to_ascii_lowercase().as_str(),
                        "reset" | "button" | "file" | "image"
                    ) {
                        let val = attr_val(&attrs, "value").unwrap_or_default();
                        pf.fields.push((name.to_owned(), val.to_owned()));
                    }
                }
            }
        } else if tag_name.eq_ignore_ascii_case("textarea")
            && !self_closing
            && let Some(end_ta) = find_ci(&html[cursor..], "</textarea")
        {
            if let Some(pf) = active_form.as_mut()
                && pf.fields.len() < 64
            {
                let attrs = parse_tag_attrs(attr_slice);
                if let Some(name) = attr_val(&attrs, "name").map(str::trim)
                    && !name.is_empty()
                {
                    let inner = decode_html_attr_entities(&html[cursor..cursor + end_ta]);
                    pf.fields.push((name.to_owned(), inner));
                }
            }
            let after = cursor + end_ta;
            cursor = html[after..]
                .find('>')
                .map_or(html.len(), |gt| after + gt + 1);
        }
    }

    if out.len() < MAX_DISCOVERED_PER_PAGE
        && let Some(pf) = active_form
    {
        pf.flush(base, scope, seen, out);
    }
}

pub(super) fn extract_json_endpoints(
    val: &Value,
    base: &Url,
    scope: &TargetScope,
    depth: usize,
    nodes: &mut usize,
    seen: &mut EndpointKeySet,
    out: &mut Vec<DiscoveredEndpoint>,
) {
    if depth > MAX_JSON_WALK_DEPTH
        || *nodes >= MAX_JSON_WALK_NODES
        || out.len() >= MAX_DISCOVERED_PER_PAGE
    {
        return;
    }
    *nodes += 1;
    match val {
        Value::Object(map) => {
            for child in map.values() {
                extract_json_endpoints(child, base, scope, depth + 1, nodes, seen, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                extract_json_endpoints(child, base, scope, depth + 1, nodes, seen, out);
            }
        }
        Value::String(s) => {
            let trimmed = s.trim();
            let looks_like_url = trimmed.starts_with("http://")
                || trimmed.starts_with("https://")
                || (trimmed.starts_with('/')
                    && !trimmed.starts_with("//")
                    && trimmed.len() <= 2048
                    && !trimmed.contains(' ')
                    && !trimmed.chars().any(char::is_control));
            if looks_like_url && let Some(url) = normalize_allowed_url(base, trimmed, false, scope)
            {
                record_get(url, DiscoverySource::JsonUrlField, seen, out);
            }
        }
        _ => {}
    }
}
