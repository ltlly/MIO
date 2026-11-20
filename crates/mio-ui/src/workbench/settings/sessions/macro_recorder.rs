use mio_core::{HistoryEntryId, HistoryRow, HttpRequest, HttpVersion, parse_request};

use super::*;

pub(super) struct ImportedMacroStepDraft {
    pub(super) method: String,
    pub(super) url: String,
    pub(super) headers_line: String,
    pub(super) body_text: String,
}

fn extract_host_header_origin(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).ok()?;
    for line in text.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, val)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("host")
        {
            let host = val.trim();
            if !host.is_empty() {
                return Some(format!("http://{host}"));
            }
        }
    }
    None
}

fn format_macro_step_from_request(req: &HttpRequest, skip_cookie: bool) -> ImportedMacroStepDraft {
    let mut header_parts = Vec::new();
    for h in &req.headers {
        let lower = h.name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding"
        ) {
            continue;
        }
        if skip_cookie && lower == "cookie" {
            continue;
        }
        if let Ok(val_str) = std::str::from_utf8(&h.value) {
            let val_trimmed = val_str.trim();
            if !val_trimmed.is_empty() && !val_trimmed.contains(';') {
                header_parts.push(format!("{}: {}", h.name.trim(), val_trimmed));
            }
        }
    }
    ImportedMacroStepDraft {
        method: req.method.trim().to_ascii_uppercase(),
        url: req.url.clone(),
        headers_line: header_parts.join("; "),
        body_text: String::from_utf8_lossy(&req.body).into_owned(),
    }
}

pub(super) fn snapshot_active_request_for_macro(
    view: &Workbench,
    skip_cookie: bool,
    cx: &App,
) -> Result<ImportedMacroStepDraft, String> {
    if let Some(selected) = view.selected_history {
        match selected {
            HistoryEntryId::Live(flow_id) => {
                let detail = view
                    .detail
                    .as_ref()
                    .filter(|d| d.summary.id == flow_id)
                    .ok_or_else(|| "选中的 Proxy 历史记录详情尚未加载完成。".to_owned())?;
                let req = if view.history_original_view {
                    view.detail_original
                        .request
                        .as_ref()
                        .unwrap_or(&detail.request)
                } else {
                    &detail.request
                };
                if !mio_core::sessions::valid_macro_step_url(&req.url) {
                    return Err("选中的历史记录 URL 不是合法的 http/https 地址。".into());
                }
                return Ok(format_macro_step_from_request(req, skip_cookie));
            }
            HistoryEntryId::External { .. } => {
                if !view.imported_history_request_ready() {
                    return Err("选中的归档历史请求正文尚未加载完成。".into());
                }
                let Some(HistoryRow::ExtProxy(row)) = view
                    .imported_history
                    .row
                    .as_ref()
                    .filter(|r| Some(r.id()) == view.selected_history)
                else {
                    return Err("未找到选中的归档历史记录。".into());
                };
                let raw = view
                    .history_request
                    .raw
                    .read(cx)
                    .value()
                    .as_bytes()
                    .to_vec();
                let origin = row
                    .origin
                    .clone()
                    .or_else(|| extract_host_header_origin(&raw))
                    .ok_or_else(|| "归档请求缺少目标 Origin。".to_owned())?;
                let req = parse_request(&raw, &origin, HttpVersion::Http11)
                    .map_err(|e| format!("解析归档 HTTP 请求失败：{e}"))?;
                return Ok(format_macro_step_from_request(&req, skip_cookie));
            }
        }
    }

    if let Some(active_id) = view.workspace.active
        && let Some(tab) = view.workspace.tabs.iter().find(|t| t.id == active_id)
    {
        let (target, raw) = if let Some(editors) = view.tab_editors.get(&active_id) {
            let t = editors.target.read(cx).value().trim().to_owned();
            let target = if t.is_empty() {
                tab.draft.target.trim().to_owned()
            } else {
                t
            };
            let raw = editors.request.bytes_lossless(cx);
            (target, raw)
        } else {
            (tab.draft.target.trim().to_owned(), tab.draft.raw.clone())
        };
        let effective_target = if target.is_empty() {
            extract_host_header_origin(&raw)
                .ok_or_else(|| "当前 Repeater 草稿缺少连接目标 URL（http/https）。".to_owned())?
        } else {
            target
        };
        let req = parse_request(&raw, &effective_target, tab.draft.version)
            .map_err(|e| format!("无法从当前 Repeater 草稿解析请求：{e}"))?;
        if !mio_core::sessions::valid_macro_step_url(&req.url) {
            return Err("当前 Repeater 草稿 URL 不是合法的 http/https 地址。".into());
        }
        return Ok(format_macro_step_from_request(&req, skip_cookie));
    }

    Err("请先在 Proxy HTTP 历史中选中一条请求，或打开一个 Repeater 请求标签。".into())
}
