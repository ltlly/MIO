use std::collections::BTreeMap;
use std::fmt::Write as _;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

use crate::{WebSocketDirection, WebSocketKind, http_history_path_and_query};

pub struct XmlHttpExportItem<'a> {
    pub url: &'a str,
    pub method: &'a str,
    pub status: Option<u16>,
    pub time_str: &'a str,
    pub request_bytes: &'a [u8],
    pub response_bytes: &'a [u8],
    pub comment: &'a str,
}

fn is_valid_xml_1_0_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF
    )
}

fn push_xml_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        if !is_valid_xml_1_0_char(ch) {
            continue;
        }
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

fn push_xml_attr(out: &mut String, value: &str) {
    for ch in value.chars() {
        if !is_valid_xml_1_0_char(ch) {
            continue;
        }
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

fn push_element(out: &mut String, tag: &str, attrs: &[(&str, &str)], text: &str) {
    out.push('<');
    out.push_str(tag);
    for (name, value) in attrs {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        push_xml_attr(out, value);
        out.push('"');
    }
    out.push('>');
    push_xml_text(out, text);
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

fn append_http_item_xml(
    out: &mut String,
    item: &XmlHttpExportItem<'_>,
    host: &str,
    port: u16,
    parsed: &url::Url,
) {
    let mut path = parsed.path().to_owned();
    if let Some(query) = parsed.query() {
        path.push('?');
        path.push_str(query);
    }
    let extension = parsed
        .path_segments()
        .and_then(Iterator::last)
        .and_then(|segment| segment.rsplit_once('.').map(|(_, ext)| ext))
        .unwrap_or("");
    let port_str = port.to_string();
    let status_str = item.status.map(|v| v.to_string()).unwrap_or_default();
    let resp_len_str = item.response_bytes.len().to_string();
    let req_b64 = BASE64_STANDARD.encode(item.request_bytes);
    let resp_b64 = BASE64_STANDARD.encode(item.response_bytes);

    out.push_str("<item>");
    push_element(out, "time", &[], item.time_str);
    push_element(out, "url", &[], item.url);
    push_element(out, "host", &[], host);
    push_element(out, "port", &[], &port_str);
    push_element(out, "protocol", &[], parsed.scheme());
    push_element(out, "method", &[], item.method);
    push_element(out, "path", &[], &path);
    push_element(out, "extension", &[], extension);
    push_element(out, "request", &[("base64", "true")], &req_b64);
    push_element(out, "status", &[], &status_str);
    push_element(out, "responselength", &[], &resp_len_str);
    push_element(out, "response", &[("base64", "true")], &resp_b64);
    push_element(out, "comment", &[], item.comment);
    out.push_str("</item>");
}

pub fn build_http_item_xml(item: &XmlHttpExportItem<'_>) -> Result<String, String> {
    for field in [item.time_str, item.url, item.method, item.comment] {
        if !field.chars().all(is_valid_xml_1_0_char) {
            return Err("XML 导出包含 XML 1.0 不允许的控制字符。".into());
        }
    }
    let parsed = url::Url::parse(item.url).map_err(|error| format!("导出 URL 无效：{error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("XML 导出 URL 必须使用 HTTP 协议。".into());
    }
    let host = parsed.host_str().ok_or("XML 导出 URL 缺少主机。")?;
    let port = parsed
        .port_or_known_default()
        .ok_or("XML 导出 URL 缺少端口。")?;

    let mut xml = String::with_capacity(
        512 + item.request_bytes.len() * 4 / 3 + item.response_bytes.len() * 4 / 3,
    );
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?><items mioVersion=""#);
    push_xml_attr(&mut xml, env!("CARGO_PKG_VERSION"));
    xml.push_str(r#"" exportTime=""#);
    push_xml_attr(&mut xml, item.time_str);
    xml.push_str(r#"">"#);
    append_http_item_xml(&mut xml, item, host, port, &parsed);
    xml.push_str("</items>");
    Ok(xml)
}

pub fn build_http_items_xml(export_time: &str, items: &[XmlHttpExportItem<'_>]) -> String {
    let mut xml = String::with_capacity(256 + items.len() * 512);
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?><items mioVersion=""#);
    push_xml_attr(&mut xml, env!("CARGO_PKG_VERSION"));
    xml.push_str(r#"" exportTime=""#);
    push_xml_attr(&mut xml, export_time);
    xml.push_str(r#"">"#);
    for item in items {
        let Ok(parsed) = url::Url::parse(item.url) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            continue;
        }
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let port = parsed.port_or_known_default().unwrap_or(80);
        append_http_item_xml(&mut xml, item, host, port, &parsed);
    }
    xml.push_str("</items>");
    xml
}

pub fn build_websocket_item_xml(
    url: &str,
    direction: WebSocketDirection,
    kind: WebSocketKind,
    time_str: &str,
    connection_id: u64,
    notes: &str,
    payload: &[u8],
) -> String {
    let kind_str = match kind {
        WebSocketKind::Text => "text",
        WebSocketKind::Binary => "binary",
        _ => "message",
    };
    let dir_str = match direction {
        WebSocketDirection::ClientToServer => "client_to_server",
        WebSocketDirection::ServerToClient => "server_to_client",
    };
    let conn_str = connection_id.to_string();
    let len_str = payload.len().to_string();
    let payload_b64 = BASE64_STANDARD.encode(payload);

    let mut xml = String::with_capacity(384 + payload_b64.len());
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?><websocket-items mioVersion=""#);
    push_xml_attr(&mut xml, env!("CARGO_PKG_VERSION"));
    xml.push_str(r#""><item>"#);
    push_element(&mut xml, "url", &[], url);
    push_element(&mut xml, "direction", &[], dir_str);
    push_element(&mut xml, "kind", &[], kind_str);
    push_element(&mut xml, "time", &[], time_str);
    push_element(&mut xml, "connection", &[], &conn_str);
    push_element(&mut xml, "length", &[], &len_str);
    push_element(&mut xml, "notes", &[], notes);
    push_element(&mut xml, "payload", &[("base64", "true")], &payload_b64);
    xml.push_str("</item></websocket-items>");
    xml
}

pub fn parse_xml_site_map_entries(xml: &str) -> Result<BTreeMap<String, String>, String> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| format!("无法解析 MIO XML 站点地图文件：{e}"))?;
    let mut map = BTreeMap::new();
    for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
        let path = item
            .children()
            .find(|c| c.has_tag_name("path"))
            .and_then(|c| c.text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                item.children()
                    .find(|c| c.has_tag_name("url"))
                    .and_then(|c| c.text())
                    .map(http_history_path_and_query)
            });
        let Some(path) = path else {
            continue;
        };
        let method = item
            .children()
            .find(|c| c.has_tag_name("method"))
            .and_then(|c| c.text())
            .unwrap_or("GET")
            .trim();
        let status = item
            .children()
            .find(|c| c.has_tag_name("status"))
            .and_then(|c| c.text())
            .unwrap_or("-")
            .trim();
        let len = item
            .children()
            .find(|c| c.has_tag_name("responselength"))
            .and_then(|c| c.text())
            .unwrap_or("0")
            .trim();
        let mut summary = String::new();
        let _ = write!(summary, "{method} · status={status} · len={len}");
        map.insert(path, summary);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_exports_parse_cleanly_with_escaping_and_pkg_version() {
        let item = XmlHttpExportItem {
            url: "https://example.invalid:8443/api/items.json?a=1&b=<2>",
            method: "POST",
            status: Some(200),
            time_str: "12:00:00 1 1月 \"2026\" & <test>",
            request_bytes: b"POST /api/items.json HTTP/1.1\r\nHost: example.invalid\r\n\r\n{}",
            response_bytes: b"HTTP/1.1 200 OK\r\n\r\n<ok/>",
            comment: "note <&> \"quote\" 'single'",
        };
        let single_xml = build_http_item_xml(&item).unwrap();
        let doc = roxmltree::Document::parse(&single_xml).unwrap();
        assert_eq!(
            doc.root_element().attribute("mioVersion"),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(doc.root_element().attribute("exportTime"), Some(item.time_str));
        let parsed = parse_xml_site_map_entries(&single_xml).unwrap();
        assert_eq!(
            parsed
                .get("/api/items.json?a=1&b=%3C2%3E")
                .map(String::as_str),
            Some("POST · status=200 · len=24")
        );

        let multi_xml = build_http_items_xml(
            "now",
            &[
                item,
                XmlHttpExportItem {
                    url: "ftp://ignored.invalid/file",
                    method: "GET",
                    status: None,
                    time_str: "now",
                    request_bytes: b"",
                    response_bytes: b"",
                    comment: "",
                },
            ],
        );
        let multi_doc = roxmltree::Document::parse(&multi_xml).unwrap();
        assert_eq!(
            multi_doc
                .descendants()
                .filter(|n| n.has_tag_name("item"))
                .count(),
            1
        );

        let ws_xml = build_websocket_item_xml(
            "wss://example.invalid/ws",
            WebSocketDirection::ClientToServer,
            WebSocketKind::Text,
            "12:00:00",
            7,
            "<ws>",
            b"hi",
        );
        let ws_doc = roxmltree::Document::parse(&ws_xml).unwrap();
        assert_eq!(ws_doc.root_element().tag_name().name(), "websocket-items");
        assert_eq!(
            ws_doc.root_element().attribute("mioVersion"),
            Some(env!("CARGO_PKG_VERSION"))
        );

        let control_item = XmlHttpExportItem {
            url: "https://example.invalid/",
            method: "GET",
            status: Some(200),
            time_str: "12:00:00",
            request_bytes: b"GET / HTTP/1.1\r\nHost: example.invalid\r\n\r\n",
            response_bytes: b"HTTP/1.1 200 OK\r\n\r\nok",
            comment: "bad\x00\x0bcomment",
        };
        assert!(build_http_item_xml(&control_item).is_err());
        let cleaned_multi = build_http_items_xml("now\x00", &[control_item]);
        let cleaned_doc = roxmltree::Document::parse(&cleaned_multi).unwrap();
        assert_eq!(
            cleaned_doc
                .descendants()
                .find(|n| n.has_tag_name("comment"))
                .and_then(|n| n.text()),
            Some("badcomment")
        );
    }
}
