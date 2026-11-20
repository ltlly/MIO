use super::*;

pub(in crate::workbench) struct TargetRowSummary {
    pub(in crate::workbench) host: String,
    pub(in crate::workbench) method: String,
    pub(in crate::workbench) url: String,
    pub(in crate::workbench) params: String,
    pub(in crate::workbench) status: String,
    pub(in crate::workbench) length: String,
    pub(in crate::workbench) mime: String,
    pub(in crate::workbench) title: String,
}

pub(in crate::workbench) fn format_site_map_host_origin(raw: &str) -> String {
    let Some((scheme_raw, rest)) = raw.split_once("://") else {
        return String::new();
    };
    let scheme = scheme_raw.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return String::new();
    }
    let stripped = if scheme == "http" {
        authority.strip_suffix(":80").unwrap_or(authority)
    } else if scheme == "https" {
        authority.strip_suffix(":443").unwrap_or(authority)
    } else {
        authority
    };
    format!("{scheme}://{stripped}")
}

fn path_has_non_empty_query(path_and_query: &str) -> bool {
    path_and_query
        .split_once('?')
        .is_some_and(|(_, query)| !query.split('#').next().unwrap_or("").is_empty())
}

fn extract_html_title_from_raw(raw: &[u8]) -> String {
    let body = if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        &raw[pos + 4..]
    } else if raw.starts_with(b"HTTP/")
        && let Some(pos) = raw.windows(2).position(|w| w == b"\n\n")
    {
        &raw[pos + 2..]
    } else {
        raw
    };
    let slice = &body[..body.len().min(16 * 1024)];
    let text = String::from_utf8_lossy(slice);
    let lower = text.to_ascii_lowercase();
    let Some(start) = lower.find("<title") else {
        return String::new();
    };
    let Some(gt) = lower[start..].find('>') else {
        return String::new();
    };
    let content_start = start + gt + 1;
    let Some(end) = lower[content_start..].find("</title>") else {
        return String::new();
    };
    text[content_start..content_start + end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[allow(dead_code)]
pub(in crate::workbench) fn target_row_summary(row: &HistoryRow) -> TargetRowSummary {
    target_row_summary_with_metadata(row, None, None)
}

#[allow(dead_code)]
pub(in crate::workbench) fn target_row_summary_with_response(
    row: &HistoryRow,
    loaded_response: Option<&[u8]>,
) -> TargetRowSummary {
    target_row_summary_with_metadata(row, loaded_response, None)
}

pub(in crate::workbench) fn target_row_summary_with_metadata(
    row: &HistoryRow,
    loaded_response: Option<&[u8]>,
    meta: Option<&HttpHistoryRowMetadata>,
) -> TargetRowSummary {
    let loaded_non_empty = loaded_response.filter(|bytes| !bytes.is_empty());
    let resolve_mime = |row: &HistoryRow| -> String {
        if let Some(m) = meta
            && !m.mime_type.trim().is_empty()
        {
            return crate::history::format_history_mime_display(&m.mime_type);
        }
        let label = response_mime_label(row, loaded_non_empty);
        if label.is_empty() {
            label
        } else {
            crate::history::format_history_mime_display(&label)
        }
    };
    let resolve_title = || -> String {
        if let Some(m) = meta
            && !m.title.is_empty()
        {
            return m.title.clone();
        }
        loaded_non_empty
            .map(extract_html_title_from_raw)
            .unwrap_or_default()
    };
    match row {
        HistoryRow::Live(summary) => {
            let path_and_query = http_history_path_and_query(&summary.url);
            let params = if let Some(m) = meta {
                m.params_cell_text().to_owned()
            } else if path_has_non_empty_query(&path_and_query) {
                "✓".into()
            } else {
                String::new()
            };
            let length = meta
                .and_then(|m| m.response_length)
                .or_else(|| loaded_non_empty.map(|b| b.len() as u64))
                .or_else(|| summary.status.is_some().then_some(summary.response_bytes))
                .map(|n| n.to_string())
                .unwrap_or_default();
            TargetRowSummary {
                host: format_site_map_host_origin(&summary.url),
                method: summary.method.clone(),
                url: path_and_query,
                params,
                status: summary
                    .status
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
                length,
                mime: resolve_mime(row),
                title: resolve_title(),
            }
        }
        HistoryRow::ExtProxy(archive) => {
            let req = archive.edited_request.as_ref().or(archive.request.as_ref());
            let resp = archive
                .edited_response
                .as_ref()
                .or(archive.response.as_ref());
            let origin_str = archive
                .origin
                .as_deref()
                .or_else(|| {
                    req.and_then(|r| r.request_target.as_deref())
                        .filter(|t| t.contains("://"))
                })
                .unwrap_or("");
            let path_and_query = req
                .and_then(|r| r.request_target.as_deref())
                .map(http_history_path_and_query)
                .unwrap_or_else(|| http_history_path_and_query(&row_url(row)));
            let params = if let Some(m) = meta {
                m.params_cell_text().to_owned()
            } else if path_has_non_empty_query(&path_and_query) {
                "✓".into()
            } else {
                String::new()
            };
            let length = meta
                .and_then(|m| m.response_length)
                .or_else(|| loaded_non_empty.map(|b| b.len() as u64))
                .or_else(|| resp.map(|r| r.raw_bytes))
                .map(|n| n.to_string())
                .unwrap_or_default();
            TargetRowSummary {
                host: format_site_map_host_origin(origin_str),
                method: req
                    .and_then(|request| request.method.clone())
                    .unwrap_or_default(),
                url: path_and_query,
                params,
                status: resp
                    .and_then(|response| response.status)
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
                length,
                mime: resolve_mime(row),
                title: resolve_title(),
            }
        }
    }
}

pub(in crate::workbench) fn response_mime_label(
    row: &HistoryRow,
    loaded_response: Option<&[u8]>,
) -> String {
    let (url, method, status, has_response) = match row {
        HistoryRow::Live(summary) => (
            summary.url.clone(),
            summary.method.as_str(),
            summary.status,
            summary.status.is_some() || loaded_response.is_some_and(|bytes| !bytes.is_empty()),
        ),
        HistoryRow::ExtProxy(archive) => {
            let req = archive.edited_request.as_ref().or(archive.request.as_ref());
            let resp = archive
                .edited_response
                .as_ref()
                .or(archive.response.as_ref());
            (
                row_url(row),
                req.and_then(|r| r.method.as_deref()).unwrap_or(""),
                resp.and_then(|r| r.status),
                resp.is_some() || loaded_response.is_some_and(|bytes| !bytes.is_empty()),
            )
        }
    };
    if !has_response {
        return String::new();
    }
    if let Some(raw) = loaded_response.filter(|bytes| !bytes.is_empty())
        && let Some(label) = mime_from_raw_response(raw)
    {
        return label.to_owned();
    }
    if let Some(label) = mime_from_url_extension(&url) {
        return label.to_owned();
    }
    if status == Some(204) {
        return "Data".into();
    }
    if method.is_empty()
        || method.eq_ignore_ascii_case("GET")
        || method.eq_ignore_ascii_case("HEAD")
    {
        "HTML".into()
    } else {
        "Data".into()
    }
}

pub(in crate::workbench) fn mime_from_url_extension(url: &str) -> Option<&'static str> {
    let without_query = url.split(['?', '#']).next().unwrap_or(url).trim();
    let path = if let Some((_, tail)) = without_query.split_once("://") {
        let slash = tail.find('/')?;
        &tail[slash..]
    } else if without_query.starts_with('/') {
        without_query
    } else {
        return None;
    };
    let filename = path.rsplit('/').next().unwrap_or("");
    let (_, ext) = filename.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => Some("HTML"),
        "json" => Some("JSON"),
        "js" | "mjs" => Some("Script"),
        "xml" | "svg" => Some("XML"),
        "css" => Some("CSS"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" => Some("Image"),
        "txt" | "csv" => Some("Text"),
        "pdf" | "zip" | "gz" | "tar" | "woff" | "woff2" | "ttf" | "otf" | "wasm" | "bin" => {
            Some("Data")
        }
        _ => None,
    }
}

pub(in crate::workbench) fn mime_from_raw_response(raw: &[u8]) -> Option<&'static str> {
    let (head, body) = if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        (&raw[..pos], &raw[pos + 4..])
    } else if raw.starts_with(b"HTTP/")
        && let Some(pos) = raw.windows(2).position(|w| w == b"\n\n")
    {
        (&raw[..pos], &raw[pos + 2..])
    } else {
        (&[][..], raw)
    };
    let mut explicit_octet_stream = false;
    for line in head.split(|&b| b == b'\n') {
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        if !line[..colon]
            .trim_ascii()
            .eq_ignore_ascii_case(b"content-type")
        {
            continue;
        }
        let essence = String::from_utf8_lossy(&line[colon + 1..])
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        match essence.as_str() {
            "text/html" | "application/xhtml+xml" => return Some("HTML"),
            "application/json" | "text/json" => return Some("JSON"),
            s if s.ends_with("+json") => return Some("JSON"),
            "application/javascript"
            | "text/javascript"
            | "application/x-javascript"
            | "application/ecmascript"
            | "text/ecmascript" => return Some("Script"),
            "application/xml" | "text/xml" | "image/svg+xml" => return Some("XML"),
            s if s.ends_with("+xml") => return Some("XML"),
            "text/css" => return Some("CSS"),
            s if s.starts_with("image/") => return Some("Image"),
            "text/plain" | "text/csv" => return Some("Text"),
            s if s.starts_with("text/") => return Some("Text"),
            "application/octet-stream" => explicit_octet_stream = true,
            s if !s.is_empty() => return Some(sniff_body_mime(body).unwrap_or("Data")),
            _ => {}
        }
    }
    if let Some(sniffed) = sniff_body_mime(body) {
        return Some(sniffed);
    }
    explicit_octet_stream.then_some("Data")
}

fn sniff_body_mime(body: &[u8]) -> Option<&'static str> {
    if body.is_empty() {
        return None;
    }
    if body.starts_with(b"\x89PNG\r\n\x1a\n")
        || body.starts_with(b"GIF87a")
        || body.starts_with(b"GIF89a")
        || body.starts_with(b"\xff\xd8\xff")
        || body.starts_with(b"\x00\x00\x01\x00")
        || (body.len() >= 12 && &body[..4] == b"RIFF" && &body[8..12] == b"WEBP")
    {
        return Some("Image");
    }
    if body.starts_with(b"%PDF-")
        || body.starts_with(b"PK\x03\x04")
        || body.starts_with(b"\x1f\x8b")
        || body.starts_with(b"\x00asm")
    {
        return Some("Data");
    }
    let trimmed = body.trim_ascii_start();
    if (trimmed.len() >= 14 && trimmed[..14].eq_ignore_ascii_case(b"<!doctype html"))
        || (trimmed.len() >= 5 && trimmed[..5].eq_ignore_ascii_case(b"<html"))
    {
        return Some("HTML");
    }
    if (trimmed.len() >= 5 && trimmed[..5].eq_ignore_ascii_case(b"<?xml"))
        || (trimmed.len() >= 4 && trimmed[..4].eq_ignore_ascii_case(b"<svg"))
    {
        return Some("XML");
    }
    let trimmed_end = trimmed.trim_ascii_end();
    if ((trimmed_end.starts_with(b"{") && trimmed_end.ends_with(b"}"))
        || (trimmed_end.starts_with(b"[") && trimmed_end.ends_with(b"]")))
        && std::str::from_utf8(trimmed_end).is_ok()
    {
        return Some("JSON");
    }
    if std::str::from_utf8(body).is_err() {
        return Some("Data");
    }
    None
}

impl Workbench {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn on_target_ext_message(
        &mut self,
        project: ProjectKey,
        selection_revision: u64,
        message_id: Uuid,
        offset: u64,
        result: Result<mio_core::ExtMessageFragment, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project != Some(project) {
            return false;
        }
        let Some(read) = self
            .target_pane
            .pending_reads
            .get(&selection_revision)
            .cloned()
        else {
            return false;
        };
        let TargetRead::ExtRequest {
            id,
            message_id: expected,
        } = read
        else {
            let TargetRead::ExtResponse {
                id,
                message_id: expected,
            } = read
            else {
                return false;
            };
            if expected != message_id || offset != 0 {
                return false;
            }
            self.target_pane.pending_reads.remove(&selection_revision);
            match result {
                Ok(fragment) => {
                    self.target_pane.selected_id = Some(id);
                    self.target_pane.selected_response_bytes = fragment.bytes.clone();
                    self.target_pane.response.load(&fragment.bytes, window, cx);
                    if self.inspector.open {
                        self.sync_inspector_selection(window, cx);
                    }
                }
                Err(error) => {
                    self.target_pane.site_map_error = Some(error);
                }
            }
            cx.notify();
            return true;
        };
        if expected != message_id || offset != 0 {
            return false;
        }
        self.target_pane.pending_reads.remove(&selection_revision);
        match result {
            Ok(fragment) => {
                self.target_pane.selected_id = Some(id);
                self.target_pane.request.load(&fragment.bytes, window, cx);
                if self.inspector.open {
                    self.sync_inspector_selection(window, cx);
                }
                if self.target_pane.pending_branch_scan == Some(id) {
                    self.target_pane.pending_branch_scan = None;
                    let action = self.target_pane.pending_branch_action;
                    if let Ok(draft) = self.current_transfer_draft("执行分支操作", window, cx) {
                        self.execute_branch_action_with_draft(draft, action, window, cx);
                    } else {
                        self.send_to_scanner(window, cx);
                    }
                }
            }
            Err(error) => {
                if self.target_pane.pending_branch_scan == Some(id) {
                    self.target_pane.pending_branch_scan = None;
                }
                self.target_pane.site_map_error = Some(error);
            }
        }
        cx.notify();
        true
    }
}
