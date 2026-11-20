use std::{ops::Range, rc::Rc};

use gpui_kit::base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::{Context, FontWeight, HighlightStyle, Hsla, SharedString, Window, hsla, rgb};

pub(crate) const MAX_HIGHLIGHT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Method,
    Uri,
    QueryPunct,
    #[allow(dead_code)]
    QueryKey,
    QueryValue,
    HttpVersion,
    Status2xx,
    Status3xx,
    Status4xx,
    Status5xx,
    HeaderName,
    HeaderSep,
    HeaderValue,
    SensitiveHeaderValue,
    JsonKey,
    JsonString,
    JsonNumber,
    JsonKeyword,
    XmlBracket,
    XmlTag,
    XmlAttrName,
    XmlString,
    PayloadMarker,
    NonPrintableBadge,
}

impl TokenKind {
    pub(crate) fn style(self, dark: bool) -> HighlightStyle {
        let color_style = |light: u32, dark_hex: u32, weight: Option<FontWeight>| HighlightStyle {
            color: Some(rgb(if dark { dark_hex } else { light }).into()),
            font_weight: weight,
            ..Default::default()
        };
        match self {
            Self::Method => color_style(0x141414, 0xff7b72, None),
            Self::Uri => color_style(0x141414, 0xd1e8f9, None),
            Self::QueryPunct | Self::HeaderSep => color_style(0x202020, 0xd1e8f9, None),
            Self::QueryKey => color_style(0x141414, 0xbbcdff, None),
            Self::XmlAttrName => color_style(0x0000c0, 0xbbcdff, None),
            Self::QueryValue | Self::SensitiveHeaderValue => color_style(0xa31616, 0xa5c35b, None),
            Self::XmlString => color_style(0xa01010, 0xa5c35b, None),
            Self::HttpVersion => color_style(0x141414, 0xd1e8f9, None),
            Self::Status2xx => color_style(0x141414, 0x56d364, None),
            Self::Status3xx => color_style(0x141414, 0x58a6ff, None),
            Self::Status4xx => color_style(0x141414, 0xf0883e, None),
            Self::Status5xx => color_style(0x141414, 0xff7b72, None),
            Self::HeaderName => color_style(0x030075, 0xd1e8f9, None),
            Self::HeaderValue => color_style(0x202020, 0xbababa, None),
            Self::JsonKey => color_style(0x030075, 0xe9c063, None),
            Self::JsonString => color_style(0x008000, 0x93c763, None),
            Self::JsonNumber => color_style(0x0000ff, 0x79c1f4, None),
            Self::JsonKeyword => color_style(0x030075, 0xff9e57, Some(FontWeight::MEDIUM)),
            Self::XmlBracket => color_style(0x800000, 0xcecece, None),
            Self::XmlTag => color_style(0x800000, 0xe9c063, None),
            Self::PayloadMarker => {
                let fg: Hsla = rgb(0xdb612f).into();
                let bg = hsla(fg.h, fg.s, fg.l, if dark { 0.24 } else { 0.16 });
                HighlightStyle {
                    color: Some(fg),
                    background_color: Some(bg),
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                }
            }
            Self::NonPrintableBadge => HighlightStyle {
                color: Some(rgb(if dark { 0xd4d4d4 } else { 0x2b2b2b }).into()),
                background_color: Some(rgb(if dark { 0x3c3f41 } else { 0xe1e0e0 }).into()),
                ..Default::default()
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct HttpSyntaxHighlighter {
    tokens: Vec<(Range<usize>, TokenKind)>,
    extra_wrap_boundaries: Rc<[usize]>,
    badge_ranges: Rc<[Range<usize>]>,
    text_len: usize,
}

pub(crate) fn http_highlighter_factory() -> InputHighlighterFactory {
    Rc::new(|language| {
        (language.eq_ignore_ascii_case("http"))
            .then(|| Box::new(HttpSyntaxHighlighter::default()) as Box<dyn InputHighlighter>)
    })
}

pub(crate) fn http_highlighter_factory_with_badges(
    badge_ranges: Vec<Range<usize>>,
) -> InputHighlighterFactory {
    let badge_ranges: Rc<[Range<usize>]> = badge_ranges.into();
    Rc::new(move |language| {
        (language.eq_ignore_ascii_case("http")).then(|| {
            Box::new(HttpSyntaxHighlighter {
                badge_ranges: badge_ranges.clone(),
                ..Default::default()
            }) as Box<dyn InputHighlighter>
        })
    })
}

impl HttpSyntaxHighlighter {
    pub(crate) fn with_badges(badge_ranges: Vec<Range<usize>>) -> Self {
        Self {
            badge_ranges: badge_ranges.into(),
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn parse_str(&mut self, text: &str) {
        self.text_len = text.len();
        let cap = floor_char_boundary(text, text.len().min(MAX_HIGHLIGHT_BYTES));
        let slice = &text[..cap];
        self.tokens = tokenize_http_with_badges(slice, &self.badge_ranges);
        self.extra_wrap_boundaries = request_start_line_wrap_boundaries(slice, &self.tokens);
    }

    pub(crate) fn styles_for_mode(
        &self,
        range: &Range<usize>,
        dark: bool,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let start = range.start.min(self.text_len);
        let end = range.end.min(self.text_len).max(start);
        if start >= end {
            return Vec::new();
        }
        let first = self.tokens.partition_point(|(tok, _)| tok.end <= start);
        let mut runs = Vec::new();
        let mut cursor = start;
        for (tok, kind) in &self.tokens[first..] {
            if tok.start >= end {
                break;
            }
            let s = tok.start.max(start);
            let e = tok.end.min(end);
            if s >= e || e <= cursor {
                continue;
            }
            if cursor < s {
                runs.push((cursor..s, HighlightStyle::default()));
            }
            runs.push((s..e, kind.style(dark)));
            cursor = e;
        }
        if cursor < end {
            runs.push((cursor..end, HighlightStyle::default()));
        }
        runs
    }
}

impl InputHighlighter for HttpSyntaxHighlighter {
    fn language(&self) -> SharedString {
        "http".into()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        self.text_len = text.len();
        let cap = clip_rope_char_end(text, text.len().min(MAX_HIGHLIGHT_BYTES));
        let slice = text.slice(0..cap).to_string();
        self.tokens = tokenize_http_with_badges(&slice, &self.badge_ranges);
        self.extra_wrap_boundaries = request_start_line_wrap_boundaries(&slice, &self.tokens);
    }

    fn update_batch(
        &mut self,
        edits: &[(InputEdit, Rope)],
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        if let Some((edit, text)) = edits.last() {
            self.update(Some(*edit), text, folding, window, cx);
        }
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        self.styles_for_mode(range, crate::theme::is_dark())
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }

    fn extra_wrap_boundaries(&self, _text: &Rope) -> Rc<[usize]> {
        self.extra_wrap_boundaries.clone()
    }
}

fn request_start_line_wrap_boundaries(
    text: &str,
    tokens: &[(Range<usize>, TokenKind)],
) -> Rc<[usize]> {
    if text.is_empty() || text.starts_with("HTTP/") {
        return Rc::from([]);
    }
    let first_nl = text.find('\n').unwrap_or(text.len());
    let first_end = text[..first_nl]
        .strip_suffix('\r')
        .map_or(first_nl, str::len);
    let first_line = &text[..first_end];
    if !is_http_start_line(first_line) {
        return Rc::from([]);
    }
    let mut boundaries = Vec::new();
    for (range, kind) in tokens {
        if range.start >= first_end {
            break;
        }
        if *kind == TokenKind::QueryPunct
            && range.end <= first_end
            && text.get(range.clone()) == Some("&")
        {
            boundaries.push(range.end);
        }
    }
    Rc::from(boundaries)
}

fn clip_rope_char_end(text: &Rope, byte_limit: usize) -> usize {
    if byte_limit >= text.len() {
        return text.len();
    }
    let char_ix = text.byte_to_char_idx(byte_limit);
    text.char_to_byte_idx(char_ix)
}

#[cfg(test)]
fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

pub(crate) fn tokenize_http(text: &str) -> Vec<(Range<usize>, TokenKind)> {
    tokenize_http_with_badges(text, &[])
}

pub(crate) fn tokenize_http_with_badges(
    text: &str,
    badges: &[Range<usize>],
) -> Vec<(Range<usize>, TokenKind)> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut base = Vec::new();
    let first_nl = text.find('\n').unwrap_or(text.len());
    let first_end = text[..first_nl]
        .strip_suffix('\r')
        .map_or(first_nl, str::len);
    let first_line = &text[..first_end];

    if is_http_start_line(first_line) {
        tokenize_start_line(first_line, 0, badges, &mut base);
        let mut cursor = (first_nl + 1).min(text.len());
        let mut content_type = String::new();
        cursor = tokenize_headers_block(text, cursor, badges, &mut content_type, &mut base);
        if cursor < text.len() {
            tokenize_body(&text[cursor..], cursor, &content_type, badges, &mut base);
        }
    } else {
        tokenize_body(text, 0, "", badges, &mut base);
    }

    let markers = find_payload_markers(text);
    if !markers.is_empty() {
        base = overlay_spans(base, &markers, TokenKind::PayloadMarker);
    }
    if !badges.is_empty() {
        let capped_end = text.len();
        let mut valid_badges = Vec::new();
        for range in badges {
            if range.start >= capped_end {
                break;
            }
            let end = range.end.min(capped_end);
            if range.start < end
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(end)
            {
                valid_badges.push(range.start..end);
            }
        }
        if !valid_badges.is_empty() {
            base = overlay_spans(base, &valid_badges, TokenKind::NonPrintableBadge);
        }
    }
    base
}

fn line_offset(whole: &str, part: &str) -> Range<usize> {
    let start = part.as_ptr() as usize - whole.as_ptr() as usize;
    start..start + part.len()
}

fn tokenize_headers_block(
    text: &str,
    start_cursor: usize,
    badges: &[Range<usize>],
    content_type: &mut String,
    out: &mut Vec<(Range<usize>, TokenKind)>,
) -> usize {
    let rest = &text[start_cursor..];
    let mut headers = [httparse::EMPTY_HEADER; 512];
    if badges.is_empty()
        && let Ok(httparse::Status::Complete((rel_end, parsed))) =
        httparse::parse_headers(rest.as_bytes(), &mut headers)
    {
        for header in parsed {
            if header.name.is_empty() {
                continue;
            }
            let name_range = line_offset(text, header.name);
            out.push((name_range.clone(), TokenKind::HeaderName));
            let after_name = &text[name_range.end..];
            if let Some(colon_rel) = after_name.find(':') {
                let colon_pos = name_range.end + colon_rel;
                out.push((colon_pos..colon_pos + 1, TokenKind::HeaderSep));
            }
            if !header.value.is_empty() {
                let val_start = header.value.as_ptr() as usize - text.as_ptr() as usize;
                let val_end = val_start + header.value.len();
                let kind = if header.name.eq_ignore_ascii_case("cookie")
                    || header.name.eq_ignore_ascii_case("set-cookie")
                    || header.name.eq_ignore_ascii_case("authorization")
                    || header.name.eq_ignore_ascii_case("proxy-authorization")
                {
                    TokenKind::SensitiveHeaderValue
                } else {
                    TokenKind::HeaderValue
                };
                out.push((val_start..val_end, kind));
                if header.name.eq_ignore_ascii_case("content-type")
                    && let Ok(val_str) = std::str::from_utf8(header.value)
                {
                    *content_type = val_str.to_ascii_lowercase();
                }
            }
        }
        return start_cursor + rel_end;
    }

    let mut cursor = start_cursor;
    while cursor < text.len() {
        let rel_nl = text[cursor..].find('\n').unwrap_or(text.len() - cursor);
        let line_nl = cursor + rel_nl;
        let line_end = text[cursor..line_nl]
            .strip_suffix('\r')
            .map_or(line_nl, |s| cursor + s.len());
        let raw_line = &text[cursor..line_end];
        let line = trim_line_badges(raw_line, cursor, badges);
        cursor = (line_nl + 1).min(text.len());
        if line.is_empty() {
            break;
        }
        if let Some(colon) = line.find(':') {
            let name = &line[..colon];
            if !name.is_empty() {
                out.push((line_offset(text, name), TokenKind::HeaderName));
            }
            let sep_pos = line_offset(text, &line[colon..colon + 1]);
            out.push((sep_pos, TokenKind::HeaderSep));
            let val_slice = &line[colon + 1..];
            let trimmed = val_slice.trim_start_matches([' ', '\t']);
            if !trimmed.is_empty() {
                let kind = if name.eq_ignore_ascii_case("cookie")
                    || name.eq_ignore_ascii_case("set-cookie")
                    || name.eq_ignore_ascii_case("authorization")
                    || name.eq_ignore_ascii_case("proxy-authorization")
                {
                    TokenKind::SensitiveHeaderValue
                } else {
                    TokenKind::HeaderValue
                };
                out.push((line_offset(text, trimmed), kind));
            }
            if name.eq_ignore_ascii_case("content-type") {
                *content_type = trimmed.to_ascii_lowercase();
            }
        }
    }
    cursor
}

fn trim_line_badges<'a>(
    line: &'a str,
    line_offset_start: usize,
    badges: &[Range<usize>],
) -> &'a str {
    if badges.is_empty() || line.is_empty() {
        return line;
    }
    let line_end = line_offset_start + line.len();
    let mut cut = line.len();
    let idx = badges.partition_point(|r| r.end <= line_end);
    for r in badges[..idx].iter().rev() {
        if r.end < line_offset_start {
            break;
        }
        let rel_start = r.start.saturating_sub(line_offset_start);
        let rel_end = r.end.saturating_sub(line_offset_start);
        if rel_end <= cut && line[rel_end..cut].bytes().all(|b| b == b' ') {
            cut = rel_start;
        } else {
            break;
        }
    }
    &line[..cut]
}

fn is_http_method(token: &str) -> bool {
    matches!(
        token,
        "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "OPTIONS" | "HEAD" | "CONNECT" | "TRACE"
    )
}

fn is_http_start_line(line: &str) -> bool {
    if line.starts_with("HTTP/") {
        return true;
    }
    let Some((first, rest)) = line.split_once(' ') else {
        return false;
    };
    is_http_method(first) || rest.contains("HTTP/")
}

fn tokenize_start_line(
    line: &str,
    offset: usize,
    badges: &[Range<usize>],
    out: &mut Vec<(Range<usize>, TokenKind)>,
) {
    let line = trim_line_badges(line, offset, badges);
    if line.starts_with("HTTP/") {
        let ver_end = line.find(' ').unwrap_or(line.len());
        out.push((offset..offset + ver_end, TokenKind::HttpVersion));
        let rest = &line[ver_end..];
        let ws = rest.len() - rest.trim_start_matches(' ').len();
        let code_start = ver_end + ws;
        let after_ws = &line[code_start..];
        let code_len = after_ws.bytes().take_while(|b| b.is_ascii_digit()).count();
        if code_len >= 3
            && let Ok(code) = after_ws[..3].parse::<u16>()
        {
            let status_end = after_ws.len();
            let kind = match code {
                200..=299 => TokenKind::Status2xx,
                300..=399 => TokenKind::Status3xx,
                400..=499 => TokenKind::Status4xx,
                500..=599 => TokenKind::Status5xx,
                _ => TokenKind::HeaderValue,
            };
            out.push((offset + code_start..offset + code_start + status_end, kind));
        }
        return;
    }

    let Some(sp1) = line.find(' ') else {
        return;
    };
    if sp1 > 0 {
        out.push((offset..offset + sp1, TokenKind::Method));
    }
    let after_method = &line[sp1..];
    let ws1 = after_method.len() - after_method.trim_start_matches(' ').len();
    let uri_start = sp1 + ws1;
    let rest = &line[uri_start..];
    let (uri, ver_range) = if let Some(sp2) = rest.rfind(' ') {
        let tail = &rest[sp2 + 1..];
        if tail.starts_with("HTTP/") {
            (
                &rest[..sp2],
                Some(offset + uri_start + sp2 + 1..offset + line.len()),
            )
        } else {
            (rest, None)
        }
    } else {
        (rest, None)
    };
    tokenize_uri(uri, offset + uri_start, out);
    if let Some(vr) = ver_range {
        out.push((vr, TokenKind::HttpVersion));
    }
}

fn tokenize_uri(uri: &str, offset: usize, out: &mut Vec<(Range<usize>, TokenKind)>) {
    if uri.is_empty() {
        return;
    }
    let Some(q_idx) = uri.find('?') else {
        out.push((offset..offset + uri.len(), TokenKind::Uri));
        return;
    };
    if q_idx > 0 {
        out.push((offset..offset + q_idx, TokenKind::Uri));
    }
    out.push((offset + q_idx..offset + q_idx + 1, TokenKind::QueryPunct));
    let query_and_frag = &uri[q_idx + 1..];
    let q_end = query_and_frag.find('#').unwrap_or(query_and_frag.len());
    tokenize_form_pairs(&query_and_frag[..q_end], offset + q_idx + 1, out);
    if q_end < query_and_frag.len() {
        out.push((
            offset + q_idx + 1 + q_end..offset + uri.len(),
            TokenKind::Uri,
        ));
    }
}

fn tokenize_form_pairs(input: &str, offset: usize, out: &mut Vec<(Range<usize>, TokenKind)>) {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let delim = input[i..]
            .find(['&', '\r', '\n'])
            .map_or(bytes.len(), |rel| i + rel);
        let seg = &input[i..delim];
        if let Some(eq_rel) = seg.find('=') {
            if eq_rel > 0 {
                out.push((offset + i..offset + i + eq_rel, TokenKind::HeaderName));
            }
            out.push((
                offset + i + eq_rel..offset + i + eq_rel + 1,
                TokenKind::QueryPunct,
            ));
            if eq_rel + 1 < seg.len() {
                out.push((offset + i + eq_rel + 1..offset + delim, TokenKind::QueryValue));
            }
        } else if !seg.is_empty() {
            out.push((offset + i..offset + delim, TokenKind::HeaderName));
        }
        if delim < bytes.len() {
            if bytes[delim] == b'&' {
                out.push((offset + delim..offset + delim + 1, TokenKind::QueryPunct));
            }
            i = delim + 1;
        } else {
            break;
        }
    }
}

fn tokenize_body(
    body: &str,
    offset: usize,
    content_type: &str,
    badges: &[Range<usize>],
    out: &mut Vec<(Range<usize>, TokenKind)>,
) {
    let trimmed = body.trim_start();
    if trimmed.is_empty() {
        return;
    }
    if content_type.contains("json") || trimmed.starts_with('{') || trimmed.starts_with('[') {
        super::syntax_highlight_body::tokenize_json(body, offset, out);
    } else if content_type.contains("html")
        || content_type.contains("xml")
        || trimmed.starts_with('<')
    {
        super::syntax_highlight_body::tokenize_markup(body, offset, out);
    } else if content_type.contains("x-www-form-urlencoded")
        || looks_like_form_urlencoded(trimmed, offset + (body.len() - trimmed.len()), badges)
    {
        let lead = body.len() - trimmed.len();
        let end = trimmed.trim_end_matches(['\r', '\n']).len();
        tokenize_form_body_pairs(&trimmed[..end], offset + lead, badges, out);
    }
}

fn tokenize_form_body_pairs(
    input: &str,
    offset: usize,
    badges: &[Range<usize>],
    out: &mut Vec<(Range<usize>, TokenKind)>,
) {
    let input = trim_line_badges(input, offset, badges);
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let delim = input[i..]
            .find(['&', ' ', '\t', '\r', '\n'])
            .map_or(bytes.len(), |rel| i + rel);
        let seg = &input[i..delim];
        if let Some(eq_rel) = seg.find('=') {
            if eq_rel > 0 {
                out.push((offset + i..offset + i + eq_rel, TokenKind::HeaderName));
            }
            out.push((
                offset + i + eq_rel..offset + i + eq_rel + 1,
                TokenKind::QueryPunct,
            ));
            if eq_rel + 1 < seg.len() {
                out.push((
                    offset + i + eq_rel + 1..offset + delim,
                    TokenKind::QueryValue,
                ));
            }
        } else if !seg.is_empty() {
            out.push((offset + i..offset + delim, TokenKind::HeaderName));
        }
        if delim < bytes.len() && bytes[delim] == b'&' {
            out.push((offset + delim..offset + delim + 1, TokenKind::QueryPunct));
            i = delim + 1;
        } else {
            break;
        }
    }
}

fn looks_like_form_urlencoded(s: &str, offset: usize, badges: &[Range<usize>]) -> bool {
    let first_line = s.lines().next().unwrap_or("");
    let line = trim_line_badges(first_line, offset, badges);
    !line.is_empty()
        && line.contains('=')
        && !line.contains(' ')
        && !line.contains('\t')
        && line.bytes().all(|b| b.is_ascii_graphic())
}

fn find_payload_markers(text: &str) -> Vec<Range<usize>> {
    const MARKER: &str = "§";
    const M_LEN: usize = MARKER.len();
    let mut positions = text.match_indices(MARKER).map(|(idx, _)| idx);
    let mut out = Vec::new();
    while let (Some(open), Some(close)) = (positions.next(), positions.next()) {
        out.push(open..close + M_LEN);
    }
    out
}

fn overlay_spans(
    base: Vec<(Range<usize>, TokenKind)>,
    spans: &[Range<usize>],
    span_kind: TokenKind,
) -> Vec<(Range<usize>, TokenKind)> {
    let mut out = Vec::with_capacity(base.len() + spans.len() * 2);
    let mut span_index = 0;
    let mut covered_until = 0;
    for (mut range, kind) in base {
        range.start = range.start.max(covered_until);
        if range.start >= range.end {
            continue;
        }
        while span_index < spans.len() && spans[span_index].start < range.end {
            let span = &spans[span_index];
            if range.start < span.start {
                out.push((range.start..span.start, kind));
            }
            out.push((span.clone(), span_kind));
            covered_until = span.end;
            range.start = range.start.max(span.end);
            span_index += 1;
            if range.start >= range.end {
                break;
            }
        }
        if range.start < range.end {
            out.push((range, kind));
        }
    }
    out.extend(spans[span_index..].iter().cloned().map(|range| (range, span_kind)));
    out
}
