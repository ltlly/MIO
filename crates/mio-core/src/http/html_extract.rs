use std::collections::HashSet;
use std::sync::OnceLock;

use mio_plugin_api::HttpVersion;
use scraper::{ElementRef, Html, Node, Selector};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedHtmlLink {
    pub href: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedHtmlFormInput {
    pub name: String,
    pub input_type: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedHtmlForm {
    pub action: String,
    pub method: String,
    pub inputs: Vec<ExtractedHtmlFormInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedHtmlSummary {
    pub title: Option<String>,
    pub headings: Vec<(u8, String)>,
    pub paragraphs: Vec<String>,
    pub links: Vec<ExtractedHtmlLink>,
    pub forms: Vec<ExtractedHtmlForm>,
    pub scripts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedHtmlScript {
    pub is_external: bool,
    pub index: usize,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedHtmlFormStats {
    pub form_count: usize,
    pub param_names: Vec<String>,
}

fn static_selector(cell: &'static OnceLock<Selector>, query: &str) -> &'static Selector {
    cell.get_or_init(|| Selector::parse(query).expect("static HTML selector must be valid"))
}

pub fn parse_svg_dimensions(svg_text: &str) -> (Option<u32>, Option<u32>) {
    static SVG_SEL: OnceLock<Selector> = OnceLock::new();
    let parse_dim_val = |raw: &str| -> Option<u32> {
        let digits: String = raw
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<u32>().ok()
    };
    if let Ok(doc) = roxmltree::Document::parse(svg_text) {
        let root = doc.root_element();
        if root.has_tag_name("svg") {
            let w = root.attribute("width").and_then(parse_dim_val);
            let h = root.attribute("height").and_then(parse_dim_val);
            return (w, h);
        }
    }
    let html_doc = Html::parse_fragment(svg_text);
    let sel = static_selector(&SVG_SEL, "svg");
    if let Some(svg_el) = html_doc.select(sel).next() {
        let w = svg_el.value().attr("width").and_then(parse_dim_val);
        let h = svg_el.value().attr("height").and_then(parse_dim_val);
        return (w, h);
    }
    (None, None)
}

fn normalize_visible_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collect_visible_nodes(el: ElementRef<'_>, out: &mut Vec<String>) {
    if matches!(
        el.value().name(),
        "script" | "style" | "noscript" | "template"
    ) {
        return;
    }
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_owned());
                }
            }
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    collect_visible_nodes(child_el, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_visible_element_text(root: ElementRef<'_>) -> String {
    let mut parts = Vec::new();
    collect_visible_nodes(root, &mut parts);
    normalize_visible_whitespace(&parts.join(" "))
}

fn extract_form_control(el: ElementRef<'_>) -> ExtractedHtmlFormInput {
    let tag_name = el.value().name();
    let name = el
        .value()
        .attr("name")
        .or_else(|| el.value().attr("id"))
        .unwrap_or("")
        .to_owned();
    let input_type = el
        .value()
        .attr("type")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if tag_name == "input" {
                "text".into()
            } else {
                tag_name.to_owned()
            }
        });
    let value = el
        .value()
        .attr("value")
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if tag_name == "textarea" {
                el.text().collect::<String>()
            } else {
                String::new()
            }
        });
    ExtractedHtmlFormInput {
        name,
        input_type,
        value,
    }
}

pub fn extract_html_summary(html: &str) -> ExtractedHtmlSummary {
    static TITLE_SEL: OnceLock<Selector> = OnceLock::new();
    static HEADING_SEL: OnceLock<Selector> = OnceLock::new();
    static PARA_SEL: OnceLock<Selector> = OnceLock::new();
    static BODY_SEL: OnceLock<Selector> = OnceLock::new();
    static LINK_SEL: OnceLock<Selector> = OnceLock::new();
    static CTRL_SEL: OnceLock<Selector> = OnceLock::new();
    static FORM_SEL: OnceLock<Selector> = OnceLock::new();
    static SCRIPT_SRC_SEL: OnceLock<Selector> = OnceLock::new();

    let document = Html::parse_document(html);
    let title = document
        .select(static_selector(&TITLE_SEL, "title"))
        .next()
        .map(|el| normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" ")))
        .filter(|s| !s.is_empty());

    let mut headings = Vec::new();
    for el in document
        .select(static_selector(&HEADING_SEL, "h1, h2, h3, h4, h5, h6"))
        .take(64)
    {
        let name = el.value().name();
        let level = name.as_bytes().get(1).map(|b| b - b'0').unwrap_or(1);
        let text = normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" "));
        if !text.is_empty() {
            headings.push((level, text));
        }
    }

    let mut paragraphs = Vec::new();
    for el in document.select(static_selector(
        &PARA_SEL,
        "p, li, blockquote, pre, td, th",
    )) {
        if paragraphs.len() >= 120 {
            break;
        }
        let text = normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" "));
        if !text.is_empty() && !paragraphs.contains(&text) {
            paragraphs.push(text);
        }
    }
    if paragraphs.is_empty()
        && let Some(body_el) = document.select(static_selector(&BODY_SEL, "body")).next()
    {
        let text = collect_visible_element_text(body_el);
        if !text.is_empty() {
            paragraphs.push(text);
        }
    }

    let mut links = Vec::new();
    for el in document
        .select(static_selector(&LINK_SEL, "a[href]"))
        .take(200)
    {
        if let Some(href) = el.value().attr("href") {
            let text = normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" "));
            links.push(ExtractedHtmlLink {
                href: href.to_owned(),
                text,
            });
        }
    }

    let mut forms = Vec::new();
    let ctrl_sel = static_selector(&CTRL_SEL, "input, select, textarea, button");
    for form_el in document
        .select(static_selector(&FORM_SEL, "form"))
        .take(64)
    {
        let action = form_el.value().attr("action").unwrap_or("/").to_owned();
        let method = form_el
            .value()
            .attr("method")
            .map(|m| m.trim().to_ascii_uppercase())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "GET".into());
        let inputs = form_el
            .select(ctrl_sel)
            .take(128)
            .map(extract_form_control)
            .collect();
        forms.push(ExtractedHtmlForm {
            action,
            method,
            inputs,
        });
    }
    if forms.is_empty() {
        let standalone: Vec<ExtractedHtmlFormInput> = document
            .select(ctrl_sel)
            .take(128)
            .map(extract_form_control)
            .filter(|c| !c.name.is_empty())
            .collect();
        if !standalone.is_empty() {
            forms.push(ExtractedHtmlForm {
                action: "(独立表单控件)".into(),
                method: "GET".into(),
                inputs: standalone,
            });
        }
    }

    let mut scripts = Vec::new();
    for el in document
        .select(static_selector(&SCRIPT_SRC_SEL, "script[src]"))
        .take(128)
    {
        if let Some(src) = el.value().attr("src") {
            let trimmed = src.trim();
            if !trimmed.is_empty() {
                scripts.push(trimmed.to_owned());
            }
        }
    }

    ExtractedHtmlSummary {
        title,
        headings,
        paragraphs,
        links,
        forms,
        scripts,
    }
}

fn append_response_link(
    base: &url::Url,
    reference: &str,
    seen: &mut HashSet<String>,
    links: &mut Vec<String>,
) -> Result<(), String> {
    let reference = reference.trim();
    if reference.is_empty() || reference.starts_with('#') {
        return Ok(());
    }
    let mut resolved = base
        .join(reference)
        .map_err(|error| format!("响应链接 {reference:?} 无效：{error}"))?;
    if !matches!(resolved.scheme(), "http" | "https") {
        return Ok(());
    }
    if resolved.host_str().is_none() {
        return Err(format!("响应链接 {reference:?} 缺少主机。"));
    }
    resolved.set_fragment(None);
    let value = resolved.to_string();
    if seen.insert(value.clone()) {
        links.push(value);
    }
    Ok(())
}

pub fn extract_response_links(base_url: &str, raw_response: &[u8]) -> Result<Vec<String>, String> {
    static BASE_SEL: OnceLock<Selector> = OnceLock::new();
    static MEDIA_LINK_SEL: OnceLock<Selector> = OnceLock::new();

    let base = url::Url::parse(base_url)
        .map_err(|error| format!("响应链接基准 URL 无效：{error}"))?;
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
        return Err("响应链接基准 URL 必须包含 HTTP 主机。".into());
    }
    let response = super::parse_response(raw_response, HttpVersion::Http11)
        .map_err(|error| format!("响应链接提取失败：{error}"))?;
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    for header in &response.headers {
        if header.name.eq_ignore_ascii_case("location") {
            let value = std::str::from_utf8(&header.value)
                .map_err(|error| format!("Location 标头文本无效：{error}"))?;
            append_response_link(&base, value, &mut seen, &mut links)?;
        }
    }
    let content_type = response
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| {
            std::str::from_utf8(&header.value)
                .map_err(|error| format!("Content-Type 标头文本无效：{error}"))
        })
        .transpose()?
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.starts_with("image/")
        || content_type.starts_with("audio/")
        || content_type.starts_with("video/")
        || content_type.contains("octet-stream")
    {
        return Ok(links);
    }
    let body = std::str::from_utf8(&response.body)
        .map_err(|error| format!("响应正文文本无效：{error}"))?;
    if content_type.contains("json") {
        let value: serde_json::Value =
            serde_json::from_str(body).map_err(|error| format!("响应 JSON 无效：{error}"))?;
        let mut pending = vec![&value];
        while let Some(value) = pending.pop() {
            match value {
                serde_json::Value::String(value)
                    if value.starts_with("http://")
                        || value.starts_with("https://")
                        || value.starts_with('/') =>
                {
                    append_response_link(&base, value, &mut seen, &mut links)?;
                }
                serde_json::Value::Array(values) => pending.extend(values.iter().rev()),
                serde_json::Value::Object(values) => pending.extend(values.values().rev()),
                _ => {}
            }
        }
    } else {
        let document = Html::parse_document(body);
        let mut document_base = base.clone();
        if let Some(element) = document
            .select(static_selector(&BASE_SEL, "base[href]"))
            .next()
        {
            let href = element
                .value()
                .attr("href")
                .expect("base 元素必须具有 href");
            document_base = base
                .join(href)
                .map_err(|error| format!("HTML base URL 无效：{error}"))?;
        }
        let selector = static_selector(
            &MEDIA_LINK_SEL,
            "a[href], area[href], link[href], script[src], img[src], iframe[src], source[src], form[action]",
        );
        for element in document.select(selector) {
            let reference = element
                .value()
                .attr("href")
                .or_else(|| element.value().attr("src"))
                .or_else(|| element.value().attr("action"))
                .expect("链接元素必须具有引用属性");
            append_response_link(&document_base, reference, &mut seen, &mut links)?;
        }
        for candidate in body.split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | '<' | '>' | '(' | ')' | '{' | '}' | '`'
                )
        }) {
            if candidate.starts_with("http://") || candidate.starts_with("https://") {
                append_response_link(
                    &base,
                    candidate.trim_end_matches(['.', ',', ';']),
                    &mut seen,
                    &mut links,
                )?;
            }
        }
    }
    Ok(links)
}

pub fn extract_scripts_from_html(base_url: &str, html: &str) -> Vec<ExtractedHtmlScript> {
    static SCRIPT_SEL: OnceLock<Selector> = OnceLock::new();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let base = url::Url::parse(base_url).ok();
    let document = Html::parse_document(html);
    for (idx, el) in document
        .select(static_selector(&SCRIPT_SEL, "script"))
        .enumerate()
    {
        if let Some(src) = el.value().attr("src") {
            let resolved = base
                .as_ref()
                .and_then(|b| b.join(src).ok())
                .map(|u| u.to_string())
                .unwrap_or_else(|| src.to_owned());
            if seen.insert(format!("SRC:{resolved}")) {
                entries.push(ExtractedHtmlScript {
                    is_external: true,
                    index: idx,
                    detail: resolved,
                });
            }
        } else {
            let inline = el
                .text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !inline.is_empty() {
                let snippet: String = inline.chars().take(160).collect();
                if seen.insert(format!("INLINE:{snippet}")) {
                    entries.push(ExtractedHtmlScript {
                        is_external: false,
                        index: idx,
                        detail: snippet,
                    });
                }
            }
        }
    }
    entries
}

pub fn extract_html_form_stats(html: &str) -> ExtractedHtmlFormStats {
    static FORM_SEL: OnceLock<Selector> = OnceLock::new();
    static NAMED_CTRL_SEL: OnceLock<Selector> = OnceLock::new();
    let doc = Html::parse_document(html);
    let form_count = doc.select(static_selector(&FORM_SEL, "form")).count();
    let mut seen = HashSet::new();
    let mut param_names = Vec::new();
    for el in doc.select(static_selector(
        &NAMED_CTRL_SEL,
        "input[name], select[name], textarea[name]",
    )) {
        if let Some(name) = el.value().attr("name") {
            let trimmed = name.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_owned()) {
                param_names.push(trimmed.to_owned());
            }
        }
    }
    ExtractedHtmlFormStats {
        form_count,
        param_names,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_dimensions_and_html_summary_extract_expected_elements() {
        assert_eq!(
            parse_svg_dimensions(r#"<svg width="120px" height="48"></svg>"#),
            (Some(120), Some(48))
        );
        assert_eq!(
            parse_svg_dimensions(r#"<svg width="64" height="32">"#),
            (Some(64), Some(32))
        );

        let html = r#"<html><head><title>  Demo Page </title></head><body><h1>Hi</h1><p>Hello</p><a href="/a">Link</a><form action="/login" method="post"><input name="user" type="text" value="a"></form><script src="/app.js"></script></body></html>"#;
        let summary = extract_html_summary(html);
        assert_eq!(summary.title.as_deref(), Some("Demo Page"));
        assert_eq!(summary.headings, vec![(1, "Hi".to_owned())]);
        assert_eq!(summary.paragraphs, vec!["Hello".to_owned()]);
        assert_eq!(summary.links.len(), 1);
        assert_eq!(summary.forms.len(), 1);
        assert_eq!(summary.forms[0].method, "POST");
        assert_eq!(summary.scripts, vec!["/app.js".to_owned()]);

        let scripts = extract_scripts_from_html(
            "https://example.invalid/dir/index.html",
            r#"<script src="app.js"></script><script>console.log(1)</script>"#,
        );
        assert_eq!(scripts.len(), 2);
        assert!(scripts[0].is_external);
        assert_eq!(scripts[0].detail, "https://example.invalid/dir/app.js");
        assert!(!scripts[1].is_external);

        let stats = extract_html_form_stats(html);
        assert_eq!(stats.form_count, 1);
        assert_eq!(stats.param_names, vec!["user".to_owned()]);
    }

    #[test]
    fn extract_response_links_handles_html_base_and_json() {
        let html_resp = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nLocation: /redir\r\n\r\n<html><head><base href=\"/sub/\"></head><body><a href=\"page#frag\">P</a></body></html>";
        let links = extract_response_links("https://example.invalid/root", html_resp).unwrap();
        assert_eq!(
            links,
            vec![
                "https://example.invalid/redir".to_owned(),
                "https://example.invalid/sub/page".to_owned(),
            ]
        );

        let json_resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"next\":\"/api/v2\",\"ext\":\"https://lab.example.invalid/cb\"}";
        let json_links =
            extract_response_links("https://example.invalid/", json_resp).unwrap();
        assert_eq!(
            json_links,
            vec![
                "https://example.invalid/api/v2".to_owned(),
                "https://lab.example.invalid/cb".to_owned(),
            ]
        );
    }
}
