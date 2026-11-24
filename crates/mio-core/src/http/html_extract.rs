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
pub struct ExtractedHtmlImage {
    pub src: String,
    pub alt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedHtmlTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtractedInlineTheme {
    pub background_hex: Option<u32>,
    pub text_hex: Option<u32>,
    pub accent_hex: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedHtmlSummary {
    pub title: Option<String>,
    pub meta_description: Option<String>,
    pub headings: Vec<(u8, String)>,
    pub paragraphs: Vec<String>,
    pub links: Vec<ExtractedHtmlLink>,
    pub forms: Vec<ExtractedHtmlForm>,
    pub scripts: Vec<String>,
    pub stylesheets: Vec<String>,
    pub images: Vec<ExtractedHtmlImage>,
    pub tables: Vec<ExtractedHtmlTable>,
    pub code_blocks: Vec<String>,
    pub inline_theme: ExtractedInlineTheme,
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
    static META_SEL: OnceLock<Selector> = OnceLock::new();
    static HEADING_SEL: OnceLock<Selector> = OnceLock::new();
    static PARA_SEL: OnceLock<Selector> = OnceLock::new();
    static BODY_SEL: OnceLock<Selector> = OnceLock::new();
    static LINK_SEL: OnceLock<Selector> = OnceLock::new();
    static CTRL_SEL: OnceLock<Selector> = OnceLock::new();
    static FORM_SEL: OnceLock<Selector> = OnceLock::new();
    static SCRIPT_SRC_SEL: OnceLock<Selector> = OnceLock::new();
    static STYLESHEET_SEL: OnceLock<Selector> = OnceLock::new();
    static IMG_SEL: OnceLock<Selector> = OnceLock::new();
    static TABLE_SEL: OnceLock<Selector> = OnceLock::new();
    static TR_SEL: OnceLock<Selector> = OnceLock::new();
    static TH_SEL: OnceLock<Selector> = OnceLock::new();
    static CELL_SEL: OnceLock<Selector> = OnceLock::new();
    static PRE_CODE_SEL: OnceLock<Selector> = OnceLock::new();
    static STYLE_SEL: OnceLock<Selector> = OnceLock::new();

    let document = Html::parse_document(html);
    let title = document
        .select(static_selector(&TITLE_SEL, "title"))
        .next()
        .map(|el| normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" ")))
        .filter(|s| !s.is_empty());

    let mut meta_description = None;
    for el in document.select(static_selector(&META_SEL, "meta[content]")) {
        let is_desc = el
            .value()
            .attr("name")
            .is_some_and(|n| n.trim().eq_ignore_ascii_case("description"))
            || el
                .value()
                .attr("property")
                .is_some_and(|p| p.trim().eq_ignore_ascii_case("og:description"));
        if is_desc
            && let Some(content) = el.value().attr("content")
        {
            let clean = normalize_visible_whitespace(content);
            if !clean.is_empty() {
                meta_description = Some(clean.chars().take(512).collect());
                break;
            }
        }
    }

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
    for el in document.select(static_selector(&PARA_SEL, "p, li, blockquote")) {
        if paragraphs.len() >= 120 {
            break;
        }
        let text = normalize_visible_whitespace(&el.text().collect::<Vec<_>>().join(" "));
        if !text.is_empty() && !paragraphs.contains(&text) {
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

    let mut stylesheets = Vec::new();
    for el in document.select(static_selector(&STYLESHEET_SEL, "link[href]")) {
        if stylesheets.len() >= 64 {
            break;
        }
        let is_stylesheet = el
            .value()
            .attr("rel")
            .is_some_and(|rel| rel.split_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet")));
        if is_stylesheet
            && let Some(href) = el.value().attr("href")
        {
            let trimmed = href.trim();
            if !trimmed.is_empty() && !stylesheets.iter().any(|s| s == trimmed) {
                stylesheets.push(trimmed.to_owned());
            }
        }
    }

    let mut images = Vec::new();
    for el in document
        .select(static_selector(&IMG_SEL, "img[src]"))
        .take(64)
    {
        if let Some(src) = el.value().attr("src") {
            let trimmed = src.trim();
            if !trimmed.is_empty() {
                let alt = el
                    .value()
                    .attr("alt")
                    .map(normalize_visible_whitespace)
                    .unwrap_or_default();
                images.push(ExtractedHtmlImage {
                    src: trimmed.to_owned(),
                    alt,
                });
            }
        }
    }

    let tr_sel = static_selector(&TR_SEL, "tr");
    let th_sel = static_selector(&TH_SEL, "th");
    let cell_sel = static_selector(&CELL_SEL, "th, td");
    let mut tables = Vec::new();
    for table_el in document
        .select(static_selector(&TABLE_SEL, "table"))
        .take(8)
    {
        let mut headers = Vec::new();
        let mut rows = Vec::new();
        for tr in table_el.select(tr_sel) {
            if rows.len() >= 24 {
                break;
            }
            let cells: Vec<String> = tr
                .select(cell_sel)
                .take(12)
                .map(|cell| normalize_visible_whitespace(&cell.text().collect::<Vec<_>>().join(" ")))
                .collect();
            if cells.iter().all(String::is_empty) {
                continue;
            }
            if headers.is_empty() && tr.select(th_sel).next().is_some() {
                headers = cells;
            } else {
                rows.push(cells);
            }
        }
        if !headers.is_empty() || !rows.is_empty() {
            tables.push(ExtractedHtmlTable { headers, rows });
        }
    }

    let mut code_blocks = Vec::new();
    for el in document.select(static_selector(&PRE_CODE_SEL, "pre, code")) {
        if code_blocks.len() >= 16 {
            break;
        }
        if el.value().name() == "code"
            && el
                .ancestors()
                .any(|anc| anc.value().as_element().is_some_and(|e| e.name() == "pre"))
        {
            continue;
        }
        let raw_code = el.text().collect::<String>();
        let trimmed = raw_code.trim();
        let min_len = if el.value().name() == "pre" { 1 } else { 8 };
        if trimmed.len() >= min_len {
            let capped: String = trimmed.chars().take(2048).collect();
            if !code_blocks.contains(&capped) {
                code_blocks.push(capped);
            }
        }
    }

    if paragraphs.is_empty()
        && headings.is_empty()
        && tables.is_empty()
        && let Some(body_el) = document.select(static_selector(&BODY_SEL, "body")).next()
    {
        let text = collect_visible_element_text(body_el);
        if !text.is_empty() {
            paragraphs.push(text);
        }
    }

    let mut css_chunks = Vec::new();
    if let Some(body_el) = document.select(static_selector(&BODY_SEL, "body")).next()
        && let Some(style_attr) = body_el.value().attr("style")
    {
        css_chunks.push(style_attr.to_owned());
    }
    for style_el in document
        .select(static_selector(&STYLE_SEL, "style"))
        .take(16)
    {
        css_chunks.push(style_el.text().collect::<String>());
    }
    let inline_theme = extract_inline_theme_from_css(&css_chunks.join("\n"));

    ExtractedHtmlSummary {
        title,
        meta_description,
        headings,
        paragraphs,
        links,
        forms,
        scripts,
        stylesheets,
        images,
        tables,
        code_blocks,
        inline_theme,
    }
}

fn parse_css_hex_color(value: &str) -> Option<u32> {
    let idx = value.find('#')?;
    let hex_part: String = value[idx + 1..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    match hex_part.len() {
        3 => {
            let b = hex_part.as_bytes();
            let r = u32::from_str_radix(std::str::from_utf8(&[b[0], b[0]]).ok()?, 16).ok()?;
            let g = u32::from_str_radix(std::str::from_utf8(&[b[1], b[1]]).ok()?, 16).ok()?;
            let bl = u32::from_str_radix(std::str::from_utf8(&[b[2], b[2]]).ok()?, 16).ok()?;
            Some((r << 16) | (g << 8) | bl)
        }
        6 | 8 => u32::from_str_radix(&hex_part[..6], 16).ok(),
        _ => None,
    }
}

fn extract_inline_theme_from_css(css: &str) -> ExtractedInlineTheme {
    let mut theme = ExtractedInlineTheme::default();
    if css.trim().is_empty() {
        return theme;
    }
    for decl in css.split([';', '{', '}']) {
        let Some((raw_prop, raw_val)) = decl.split_once(':') else {
            continue;
        };
        let prop = raw_prop.trim().to_ascii_lowercase();
        let Some(hex) = parse_css_hex_color(raw_val) else {
            continue;
        };
        if matches!(prop.as_str(), "background-color" | "background")
            && theme.background_hex.is_none()
        {
            theme.background_hex = Some(hex);
        } else if prop == "color" && theme.text_hex.is_none() {
            theme.text_hex = Some(hex);
        } else if (prop == "accent-color"
            || prop == "border-color"
            || prop.contains("accent")
            || prop.contains("primary"))
            && theme.accent_hex.is_none()
        {
            theme.accent_hex = Some(hex);
        }
    }
    theme
}

pub fn format_xml_preview(xml: &str) -> Option<(Option<String>, usize, String)> {
    const MAX_PREVIEW_CHARS: usize = 16 * 1024;
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    let root_tag = (!root.tag_name().name().is_empty()).then(|| root.tag_name().name().to_owned());
    let mut element_count = 0usize;
    let mut out = String::new();

    fn format_node(
        node: roxmltree::Node<'_, '_>,
        depth: usize,
        element_count: &mut usize,
        out: &mut String,
    ) {
        if out.len() >= MAX_PREVIEW_CHARS {
            return;
        }
        if !node.is_element() {
            return;
        }
        *element_count += 1;
        let indent = "  ".repeat(depth.min(24));
        let tag = node.tag_name().name();
        out.push_str(&indent);
        out.push('<');
        out.push_str(tag);
        for attr in node.attributes() {
            out.push(' ');
            out.push_str(attr.name());
            out.push_str("=\"");
            out.push_str(attr.value());
            out.push('"');
        }
        let child_elements: Vec<_> = node.children().filter(|c| c.is_element()).collect();
        let direct_text: String = node
            .children()
            .filter(|c| c.is_text())
            .filter_map(|c| c.text())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if child_elements.is_empty() && direct_text.is_empty() {
            out.push_str(" />\n");
        } else if child_elements.is_empty() {
            out.push('>');
            out.push_str(&direct_text);
            out.push_str("</");
            out.push_str(tag);
            out.push_str(">\n");
        } else {
            out.push_str(">\n");
            if !direct_text.is_empty() {
                out.push_str(&indent);
                out.push_str("  ");
                out.push_str(&direct_text);
                out.push('\n');
            }
            for child in child_elements {
                format_node(child, depth + 1, element_count, out);
                if out.len() >= MAX_PREVIEW_CHARS {
                    break;
                }
            }
            out.push_str(&indent);
            out.push_str("</");
            out.push_str(tag);
            out.push_str(">\n");
        }
    }

    format_node(root, 0, &mut element_count, &mut out);
    let capped: String = out.trim_end().chars().take(MAX_PREVIEW_CHARS).collect();
    Some((root_tag, element_count, capped))
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

        let rich_html = r##"
        <html>
          <head>
            <meta name="description" content="Status console overview">
            <link rel="stylesheet" href="/static/theme.css">
            <style>body { background-color: #1e293b; color: #f8fafc; --accent: #38bdf8; }</style>
          </head>
          <body>
            <img src="/logo.svg" alt="System Logo">
            <table>
              <tr><th>Service</th><th>Status</th></tr>
              <tr><td>Proxy</td><td>OK</td></tr>
            </table>
            <pre>GET /health HTTP/1.1</pre>
          </body>
        </html>"##;
        let rich = extract_html_summary(rich_html);
        assert_eq!(
            rich.meta_description.as_deref(),
            Some("Status console overview")
        );
        assert_eq!(rich.stylesheets, vec!["/static/theme.css".to_owned()]);
        assert_eq!(
            rich.images,
            vec![ExtractedHtmlImage {
                src: "/logo.svg".into(),
                alt: "System Logo".into(),
            }]
        );
        assert_eq!(rich.tables.len(), 1);
        assert_eq!(rich.tables[0].headers, vec!["Service", "Status"]);
        assert_eq!(rich.tables[0].rows, vec![vec!["Proxy", "OK"]]);
        assert_eq!(rich.code_blocks, vec!["GET /health HTTP/1.1".to_owned()]);
        assert_eq!(rich.inline_theme.background_hex, Some(0x1e293b));
        assert_eq!(rich.inline_theme.text_hex, Some(0xf8fafc));
        assert_eq!(rich.inline_theme.accent_hex, Some(0x38bdf8));

        let (root_tag, count, pretty_xml) =
            format_xml_preview("<config env=\"lab\"><item id=\"1\">ok</item></config>").unwrap();
        assert_eq!(root_tag.as_deref(), Some("config"));
        assert_eq!(count, 2);
        assert!(pretty_xml.contains("  <item id=\"1\">ok</item>"));

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
