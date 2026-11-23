use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use mio_core::{
    IntruderResultOutcome, OrganiserItem, next_organiser_number, validate_organiser_items,
};

use super::*;
use crate::{
    SendToComparerRequest, SendToComparerResponse, SendToDecoder, SendToOrganiser,
    workbench::tools_extra,
};

const MAX_DECODER_BYTES: usize = 4 * 1024 * 1024;
const MAX_COMPARER_ITEMS: usize = 64;
const MAX_COMPARER_ITEM_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(in crate::workbench::intruder::results_view) enum RowOp {
    Comparer(bool),
    Decoder,
    Organiser,
    Scanner,
    Sequencer,
    RecordIssue,
    SendToMioAi,
    CopyUrl,
    CopyCurl,
    CopyResponse,
    GenerateCsrfPoc,
    SaveItem,
    AddToScope,
    ExcludeFromScope,
}

fn serialize_intruder_result_ext_xml(record: &mio_core::FlowRecord) -> Result<Vec<u8>, String> {
    let time_str = crate::history::local_time(record.summary.started_unix_ms);
    let request_bytes = request_to_raw(&record.request);
    let response_bytes = record
        .response
        .as_ref()
        .map(response_to_raw)
        .unwrap_or_default();
    let status = record.response.as_ref().map(|resp| resp.status);
    mio_core::export_xml::build_http_item_xml(&mio_core::export_xml::XmlHttpExportItem {
        url: record.request.url.as_str(),
        method: &record.request.method,
        status,
        time_str: &time_str,
        request_bytes: &request_bytes,
        response_bytes: &response_bytes,
        comment: "",
    })
    .map(String::into_bytes)
}

fn origin_of(url: &str) -> String {
    url.split_once("://").map_or_else(String::new, |(s, t)| {
        format!("{s}://{}", &t[..t.find(['/', '?', '#']).unwrap_or(t.len())])
    })
}

fn fallback_request_bytes_for_flow(flow: &mio_core::FlowSummary) -> Vec<u8> {
    if let Ok(parsed) = url::Url::parse(&flow.url) {
        let mut path = parsed.path().to_owned();
        if let Some(q) = parsed.query() {
            path.push('?');
            path.push_str(q);
        }
        let host = match (parsed.host_str(), parsed.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_owned(),
            _ => "127.0.0.1".to_owned(),
        };
        format!("{} {path} HTTP/1.1\r\nHost: {host}\r\n\r\n", flow.method).into_bytes()
    } else {
        format!("{} / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", flow.method).into_bytes()
    }
}

pub(in crate::workbench::intruder::results_view) fn run_batch_row_op(
    target: ResultTarget,
    ordinals: Vec<u64>,
    op: RowOp,
    window: &mut Window,
    cx: &mut App,
) {
    if ordinals.len() <= 1 {
        run_row_op(target, ordinals.first().copied(), op, window, cx);
        return;
    }
    if window.window_handle() != target.owner {
        return;
    }
    let Some(owner) = target.workbench.upgrade() else {
        return;
    };
    let main_win = owner.read(cx).owner_window;
    if window.window_handle() != main_win {
        cx.defer(move |cx| {
            let _ = main_win.update(cx, |_, w, cx| {
                owner.update(cx, |wb, cx| {
                    exec_batch_row_op(wb, &target, &ordinals, op, w, cx)
                });
            });
        });
    } else {
        owner.update(cx, |wb, cx| {
            exec_batch_row_op(wb, &target, &ordinals, op, window, cx)
        });
    }
}

fn exec_batch_row_op(
    wb: &mut Workbench,
    target: &ResultTarget,
    ordinals: &[u64],
    op: RowOp,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if !target.active(wb) || !wb.backend_available() || wb.transition.is_some() {
        return;
    }
    match op {
        RowOp::CopyUrl => {
            let view = &wb.intruder.results.views[&target.view_id];
            let mut urls = Vec::new();
            for &ord in ordinals {
                if let Some(IntruderResultOutcome::Flow(flow)) = view.row(ord).map(|r| &r.outcome) {
                    if let Some(rec) = view.detail.as_ref().filter(|d| d.summary.id == flow.id) {
                        let raw = request_to_raw(&rec.request);
                        if let Ok(u) = mio_core::copy_url_for_request(
                            &raw,
                            &origin_of(&rec.request.url),
                            rec.request.version,
                        ) {
                            urls.push(u);
                            continue;
                        }
                    }
                    if !flow.url.is_empty() {
                        urls.push(flow.url.clone());
                    }
                }
            }
            if urls.is_empty() {
                wb.fail("所选 Intruder 结果没有可复制的 URL。", cx);
                return;
            }
            let count = urls.len();
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(urls.join("\n")));
            wb.error = None;
            wb.status = format!("已批量复制 {count} 条 URL 到剪贴板。");
            cx.notify();
        }
        RowOp::Organiser => {
            if !wb.organiser_editable() || !wb.flush_workspace(window, cx) {
                return;
            }
            if let Err(err) = wb.validate_organiser_composition(window, cx) {
                return wb.fail(err, cx);
            }
            let entries_data: Vec<_> = {
                let view = &wb.intruder.results.views[&target.view_id];
                ordinals
                    .iter()
                    .filter_map(|&ord| {
                        let row = view.row(ord)?;
                        let IntruderResultOutcome::Flow(flow) = &row.outcome else {
                            return None;
                        };
                        let vrow = view.query.view_row(ord);
                        let notes = vrow.map(|r| r.notes_preview.clone()).unwrap_or_default();
                        let highlight = vrow.and_then(|r| r.highlight);
                        let detail_rec = view
                            .detail
                            .as_ref()
                            .filter(|d| d.summary.id == flow.id)
                            .cloned();
                        Some((flow.clone(), detail_rec, notes, highlight))
                    })
                    .collect()
            };
            if entries_data.is_empty() {
                return wb.fail("所选 Intruder 结果没有可发送到 Organiser 的报文。", cx);
            }
            let mut added = 0usize;
            let mut first_id = None;
            for (flow, detail_rec, notes, highlight) in entries_data {
                let num = next_organiser_number(&wb.workspace.organiser_items);
                let item_res = if let Some(rec) = detail_rec {
                    OrganiserItem::from_flow_record(num, "Intruder", &rec)
                } else {
                    let req_raw = fallback_request_bytes_for_flow(&flow);
                    OrganiserItem::from_raw_parts(
                        num,
                        "Intruder",
                        &origin_of(&flow.url),
                        req_raw,
                        None,
                        flow.status,
                        mio_core::HttpVersion::Http11,
                    )
                };
                if let Ok(mut item) = item_res {
                    if !notes.is_empty() {
                        item.notes = notes;
                    }
                    if highlight.is_some() {
                        item.highlight = highlight;
                    }
                    let id = item.id;
                    wb.workspace.organiser_items.push(item);
                    if validate_organiser_items(&wb.workspace.organiser_items)
                        .and_then(|()| wb.workspace.validate_structure())
                        .is_err()
                    {
                        wb.workspace.organiser_items.pop();
                        break;
                    }
                    first_id.get_or_insert(id);
                    added += 1;
                }
            }
            if added == 0 {
                return wb.fail("未能将所选结果添加到 Organiser。", cx);
            }
            let had_sel = wb.organiser.selected_id.is_some();
            if !had_sel {
                wb.organiser.selected_id = first_id;
            }
            wb.sync_organiser_table(window, cx);
            if !had_sel {
                wb.reload_selected_organiser_detail(window, cx);
            }
            wb.save_workspace(cx);
            wb.error = None;
            wb.status = format!("已批量添加 {added} 条结果到 Organiser。");
            cx.notify();
        }
        RowOp::Comparer(is_resp) => {
            let items_data: Vec<_> = {
                let view = &wb.intruder.results.views[&target.view_id];
                ordinals
                    .iter()
                    .filter_map(|&ord| {
                        let row = view.row(ord)?;
                        let IntruderResultOutcome::Flow(flow) = &row.outcome else {
                            return None;
                        };
                        let num = view
                            .summary
                            .as_ref()
                            .and_then(|s| s.request_number(ord))
                            .unwrap_or(ord + 1);
                        let bytes = if let Some(rec) =
                            view.detail.as_ref().filter(|d| d.summary.id == flow.id)
                        {
                            if is_resp {
                                rec.response.as_ref().map(response_to_raw)?
                            } else {
                                request_to_raw(&rec.request)
                            }
                        } else if is_resp {
                            let status = flow.status?;
                            format!(
                                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\n\r\n",
                                flow.response_bytes
                            )
                            .into_bytes()
                        } else {
                            fallback_request_bytes_for_flow(flow)
                        };
                        (bytes.len() <= MAX_COMPARER_ITEM_BYTES).then_some((num, bytes))
                    })
                    .collect()
            };
            if items_data.is_empty() {
                return wb.fail("所选 Intruder 结果没有可发送到 Comparer 的报文。", cx);
            }
            let kind = if is_resp { "响应" } else { "请求" };
            let mut added = 0usize;
            for (number, bytes) in items_data {
                if wb.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
                    break;
                }
                let id = wb.extra_tools.comparer_items.last().map_or(1, |i| i.id + 1);
                let label = format!("{id}: Intruder #{number} {kind} ({} bytes)", bytes.len());
                wb.extra_tools
                    .comparer_items
                    .push(tools_extra::ComparerItem { id, label, bytes });
                added += 1;
            }
            if added == 0 {
                return wb.fail(
                    format!("Comparer 最多保存 {MAX_COMPARER_ITEMS} 项，未添加。"),
                    cx,
                );
            }
            let n = wb.extra_tools.comparer_items.len();
            wb.extra_tools.comparer_selected = n - 1;
            if n >= 2 {
                (wb.extra_tools.left_index, wb.extra_tools.right_index) = (n - 2, n - 1);
            }
            wb.refresh_comparer(window, cx);
            wb.cancel_plugin_interactions(cx);
            wb.tool = Tool::Comparer;
            wb.focus.focus(window, cx);
            wb.error = None;
            wb.status = format!("已批量发送 {added} 条{kind}到 Comparer。");
            cx.notify();
        }
        _ => {
            exec_row_op(wb, target, ordinals.first().copied(), op, window, cx);
        }
    }
}

pub(in crate::workbench::intruder::results_view) fn run_row_op(
    target: ResultTarget,
    ordinal: Option<u64>,
    op: RowOp,
    window: &mut Window,
    cx: &mut App,
) {
    if window.window_handle() != target.owner {
        return;
    }
    let Some(owner) = target.workbench.upgrade() else {
        return;
    };
    let main_win = owner.read(cx).owner_window;
    if window.window_handle() != main_win {
        cx.defer(move |cx| {
            let _ = main_win.update(cx, |_, w, cx| {
                owner.update(cx, |wb, cx| exec_row_op(wb, &target, ordinal, op, w, cx));
            });
        });
    } else {
        owner.update(cx, |wb, cx| {
            exec_row_op(wb, &target, ordinal, op, window, cx)
        });
    }
}

fn exec_row_op(
    wb: &mut Workbench,
    target: &ResultTarget,
    ordinal: Option<u64>,
    op: RowOp,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if !target.active(wb) || !wb.backend_available() || wb.transition.is_some() {
        return;
    }
    let (record, number) = {
        let view = &wb.intruder.results.views[&target.view_id];
        let Some(ord) = ordinal.or(view.selected) else {
            return;
        };
        let Some(IntruderResultOutcome::Flow(flow)) = view.row(ord).map(|r| &r.outcome) else {
            return wb.fail("所选 Intruder 结果没有可用的 HTTP 报文。", cx);
        };
        if matches!(
            op,
            RowOp::CopyUrl | RowOp::AddToScope | RowOp::ExcludeFromScope
        ) && view.detail.as_ref().is_none_or(|d| d.summary.id != flow.id)
        {
            if flow.url.is_empty() {
                wb.fail("所选 Intruder 结果 URL 尚未读取完成。", cx);
            } else if matches!(op, RowOp::CopyUrl) {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(flow.url.clone()));
                wb.error = None;
                wb.status = "已复制 URL 到剪贴板。".into();
                cx.notify();
            } else {
                let prefix = origin_of(&flow.url);
                wb.add_scope_rule(wb.project, matches!(op, RowOp::AddToScope), prefix, cx);
            }
            return;
        }
        let Some(rec) = view
            .detail
            .as_ref()
            .filter(|d| d.summary.id == flow.id)
            .cloned()
        else {
            return wb.fail("正在读取所选 Intruder 结果报文详情，请稍后重试。", cx);
        };
        let num = view
            .summary
            .as_ref()
            .and_then(|s| s.request_number(ord))
            .unwrap_or(ord + 1);
        (rec, num)
    };
    match op {
        RowOp::Comparer(is_resp) => {
            let bytes = if is_resp {
                let Some(resp) = record.response.as_ref() else {
                    return wb.fail("所选 Intruder 结果没有响应报文。", cx);
                };
                response_to_raw(resp)
            } else {
                request_to_raw(&record.request)
            };
            if bytes.len() > MAX_COMPARER_ITEM_BYTES {
                return wb.fail(
                    format!("Comparer 单项超过 {MAX_COMPARER_ITEM_BYTES} 字节上限，未添加。"),
                    cx,
                );
            }
            if wb.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
                return wb.fail(
                    format!("Comparer 最多保存 {MAX_COMPARER_ITEMS} 项，未添加。"),
                    cx,
                );
            }
            let id = wb.extra_tools.comparer_items.last().map_or(1, |i| i.id + 1);
            let kind = if is_resp { "响应" } else { "请求" };
            let label = format!("{id}: Intruder #{number} {kind} ({} bytes)", bytes.len());
            wb.extra_tools
                .comparer_items
                .push(tools_extra::ComparerItem { id, label, bytes });
            let n = wb.extra_tools.comparer_items.len();
            wb.extra_tools.comparer_selected = n - 1;
            if n >= 2 {
                (wb.extra_tools.left_index, wb.extra_tools.right_index) = (n - 2, n - 1);
            }
            wb.refresh_comparer(window, cx);
            wb.cancel_plugin_interactions(cx);
            wb.tool = Tool::Comparer;
            wb.focus.focus(window, cx);
        }
        RowOp::Decoder => {
            if let Err(err) = editing::snapshot(&wb.extra_tools.decoder_input, window, cx) {
                return wb.fail(err, cx);
            }
            let bytes = request_to_raw(&record.request);
            if bytes.len() > MAX_DECODER_BYTES {
                return wb.fail(
                    format!("Decoder 输入超过 {MAX_DECODER_BYTES} 字节上限，未覆盖原内容。"),
                    cx,
                );
            }
            let (disp, bak) = String::from_utf8(bytes.clone()).map_or_else(
                |_| (editing::hex_encode(&bytes), Some(bytes)),
                |t| (t, None),
            );
            wb.extra_tools
                .decoder_input
                .update(cx, |s, cx| s.set_value(disp, window, cx));
            let rev = wb.extra_tools.decoder_input.read(cx).document_revision();
            wb.extra_tools.decoder_input_bytes = bak.map(|b| (rev, b));
            wb.extra_tools
                .decoder_output
                .update(cx, |s, cx| s.set_value("", window, cx));
            wb.extra_tools.decoder_output_bytes = None;
            wb.extra_tools.decoder_mode = "".into();
            wb.extra_tools.steps.clear();
            wb.cancel_plugin_interactions(cx);
            wb.tool = Tool::Decoder;
            wb.focus.focus(window, cx);
        }
        RowOp::Organiser => {
            if !wb.organiser_editable() || !wb.flush_workspace(window, cx) {
                return;
            }
            if let Err(err) = wb.validate_organiser_composition(window, cx) {
                return wb.fail(err, cx);
            }
            let item = match OrganiserItem::from_flow_record(
                next_organiser_number(&wb.workspace.organiser_items),
                "Intruder",
                &record,
            ) {
                Ok(item) => item,
                Err(err) => return wb.fail(err.to_string(), cx),
            };
            let (id, num) = (item.id, item.number);
            wb.workspace.organiser_items.push(item);
            if let Err(err) = validate_organiser_items(&wb.workspace.organiser_items)
                .and_then(|()| wb.workspace.validate_structure())
            {
                wb.workspace.organiser_items.pop();
                return wb.fail(format!("未添加到 Organiser：{err}"), cx);
            }
            let had_sel = wb.organiser.selected_id.is_some();
            if !had_sel {
                wb.organiser.selected_id = Some(id);
            }
            wb.sync_organiser_table(window, cx);
            if !had_sel {
                wb.reload_selected_organiser_detail(window, cx);
            }
            wb.save_workspace(cx);
            wb.error = None;
            wb.status = format!("已添加到 Organiser (#{num})。");
        }
        RowOp::Scanner => {
            let draft = Draft {
                revision: 1,
                target: origin_of(&record.request.url),
                raw: request_to_raw(&record.request),
                version: record.request.version,
            };
            wb.cancel_plugin_interactions(cx);
            wb.send_draft_to_scanner(
                draft,
                mio_core::AuditTaskMode::PassiveAndActive,
                true,
                window,
                cx,
            );
        }
        RowOp::Sequencer => {
            let draft = Draft {
                revision: 1,
                target: origin_of(&record.request.url),
                raw: request_to_raw(&record.request),
                version: record.request.version,
            };
            wb.send_draft_to_sequencer(draft, window, cx);
        }
        RowOp::CopyUrl => {
            let raw = request_to_raw(&record.request);
            match mio_core::copy_url_for_request(
                &raw,
                &origin_of(&record.request.url),
                record.request.version,
            ) {
                Ok(url) => {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url));
                    wb.error = None;
                    wb.status = "已复制 URL 到剪贴板。".into();
                }
                Err(err) => wb.fail(err.to_string(), cx),
            }
        }
        RowOp::CopyCurl => {
            let redact = theme::redact_sensitive_headers_in_copy_curl()
                || wb
                    .user_preferences
                    .misc
                    .redact_sensitive_headers_in_copy_curl;
            let raw = request_to_raw(&record.request);
            match mio_core::curl_command_for_request_with_options(
                &raw,
                &origin_of(&record.request.url),
                record.request.version,
                redact,
            ) {
                Ok(cmd) => {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(cmd));
                    wb.error = None;
                    wb.status = "已复制 curl 命令到剪贴板。".into();
                }
                Err(err) => wb.fail(err.to_string(), cx),
            }
        }
        RowOp::CopyResponse => {
            let Some(resp) = record.response.as_ref() else {
                return wb.fail("所选 Intruder 结果没有响应报文。", cx);
            };
            let bytes = response_to_raw(resp);
            match String::from_utf8(bytes) {
                Ok(text) => {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    wb.error = None;
                    wb.status = "已复制响应到剪贴板。".into();
                }
                Err(_) => wb.fail(
                    "响应包含非 UTF-8 二进制字节；请切换到 Hex 视图复制，未替换或丢失原始字节。",
                    cx,
                ),
            }
        }
        RowOp::RecordIssue => {
            let Some(project) = wb.project else {
                return;
            };
            let url = record.request.url.clone();
            let host = url::Url::parse(&url)
                .ok()
                .and_then(|parsed| parsed.host_str().map(str::to_owned))
                .unwrap_or_default();
            let now_unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let finding = mio_core::AuditFinding {
                id: Uuid::new_v4(),
                issue_id: "0x00800100".into(),
                title: format!("自定义问题 · {url}"),
                severity_label: "信息".into(),
                severity_rank: 1,
                confidence: mio_core::IssueConfidence::Certain,
                phase: mio_core::AuditPhase::Passive,
                host,
                url: url.clone(),
                parameter: None,
                flow_id: Some(record.summary.id),
                summary: format!("从 Intruder 攻击结果 #{number} 创建的自定义问题（{url}）。"),
                evidence: url.clone(),
                request_highlights: Vec::new(),
                response_highlights: Vec::new(),
                remediation: "核查关联的 HTTP 请求与响应报文并补充修复建议。".into(),
                detected_unix_ms: now_unix_ms,
            };
            if wb.enqueue(
                Command::Audit(mio_core::AuditCommand::RecordManualFinding {
                    project,
                    finding: Box::new(finding),
                }),
                cx,
            ) {
                wb.error = None;
                wb.status = format!("已记录新问题：{url}");
            }
        }
        RowOp::SendToMioAi => {
            let req_bytes = request_to_raw(&record.request);
            let req_text = String::from_utf8_lossy(&req_bytes);
            let req_preview: String = req_text.chars().take(16 * 1024).collect();
            let mut context = format!(
                "Intruder 攻击结果 #{number}\nURL: {}\nMethod: {}\n\n--- Request ---\n{req_preview}",
                record.request.url, record.request.method,
            );
            if let Some(resp) = record.response.as_ref() {
                let resp_bytes = response_to_raw(resp);
                let resp_text = String::from_utf8_lossy(&resp_bytes);
                let resp_preview: String = resp_text.chars().take(16 * 1024).collect();
                context.push_str("\n\n--- Response ---\n");
                context.push_str(&resp_preview);
            }
            wb.open_mio_ai_with_context(format!("Intruder 结果 #{number}"), context, cx);
        }
        RowOp::GenerateCsrfPoc => {
            let raw = request_to_raw(&record.request);
            wb.open_csrf_poc_modal(&record.request.url, &raw, cx);
        }
        RowOp::SaveItem => {
            if wb.file_picker_pending() {
                return;
            }
            let payload = match serialize_intruder_result_ext_xml(&record) {
                Ok(bytes) => bytes,
                Err(err) => return wb.fail(err, cx),
            };
            let suggested = format!("intruder-result-{number}.xml");
            let project = wb.project;
            let picker = Uuid::new_v4();
            wb.file_picker = Some(picker);
            let prompt = crate::native_terminate::prompt_for_new_path(
                window,
                cx,
                &wb.default_directory(),
                Some(&suggested),
            );
            cx.spawn_in(window, async move |this, cx| {
                let result = prompt.await;
                let _ = this.update_in(cx, |view, _, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，本次条目保存未写入磁盘。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            if path.extension().is_none() {
                                path.set_extension("xml");
                            }
                            match mio_core::ui_files::write_export_file(&path, &payload) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status =
                                        format!("已保存 Intruder 条目到 {}。", path.display());
                                }
                                Err(err) => {
                                    view.fail(format!("保存条目失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => {
                            view.fail(format!("选择保存路径失败：{err}"), cx);
                        }
                        Err(_) => view.fail("文件选择器已关闭。", cx),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        RowOp::AddToScope | RowOp::ExcludeFromScope => {
            let raw = request_to_raw(&record.request);
            match mio_core::copy_url_for_request(
                &raw,
                &origin_of(&record.request.url),
                record.request.version,
            ) {
                Ok(url) => {
                    let prefix = origin_of(&url);
                    wb.add_scope_rule(wb.project, matches!(op, RowOp::AddToScope), prefix, cx);
                }
                Err(err) => wb.fail(err.to_string(), cx),
            }
        }
    }
    cx.notify();
}

pub(super) fn build_result_row_context_menu(
    table: &ResultTable,
    row: usize,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<TableState<ResultTable>>,
) -> PopupMenu {
    let ordinal = table.logical_ordinal(row, cx);
    let selected_ordinals: Vec<u64> = table
        .target
        .view(cx)
        .zip(ordinal)
        .map(|(view, ord)| {
            if view.selected_ordinals.len() > 1 && view.selected_ordinals.contains(&ord) {
                view.selected_ordinals.iter().copied().collect()
            } else {
                vec![ord]
            }
        })
        .unwrap_or_default();
    let title = table
        .target
        .view(cx)
        .zip(ordinal)
        .and_then(|(view, ordinal)| view.summary.as_ref()?.request_number(ordinal))
        .map_or_else(|| "Result".to_owned(), |number| format!("Result #{number}"));
    let entry = ordinal
        .ok_or_else(|| "所选 Intruder 结果尚未读取完成；未转送请求。".to_owned())
        .and_then(|ordinal| table.target.capture_transfer(Some(ordinal), window, cx));
    let batch_entries = ordinal
        .ok_or_else(|| "所选 Intruder 结果尚未读取完成；未转送请求。".to_owned())
        .and_then(|ordinal| table.target.capture_transfers(Some(ordinal), window, cx));
    let unavailable = entry.is_err() && batch_entries.is_err();
    let has_resp = table.target.view(cx).is_some_and(|v| {
        selected_ordinals.iter().any(|&o| {
            v.row(o).is_some_and(
                |r| matches!(&r.outcome, IntruderResultOutcome::Flow(f) if f.status.is_some()),
            )
        })
    });
    let selected_set: std::collections::BTreeSet<u64> = selected_ordinals.iter().copied().collect();
    let selected_tsv = (!selected_set.is_empty())
        .then(|| table.export_rows_delimited_filtered(Some(&selected_set), false, cx))
        .flatten();
    let intruder_target = table.target.clone();
    let intruder_entry = entry.clone();
    let repeater_target = table.target.clone();
    let repeater_entries = batch_entries.clone();
    let batch_ordinals_ref = selected_ordinals.clone();
    let add_op = |m: PopupMenu,
                  lbl: &'static str,
                  icon: IconName,
                  act: Box<dyn gpui_kit::Action>,
                  dis: bool,
                  op: RowOp| {
        let t = table.target.clone();
        let ords = batch_ordinals_ref.clone();
        m.item(
            PopupMenuItem::new(lbl)
                .icon(icon)
                .action(act)
                .disabled(dis)
                .on_click(move |_, window, cx| {
                    run_batch_row_op(t.clone(), ords.clone(), op, window, cx)
                }),
        )
    };
    let add_plain_op =
        |m: PopupMenu, lbl: &'static str, icon: IconName, dis: bool, op: RowOp| {
            let t = table.target.clone();
            let ords = batch_ordinals_ref.clone();
            m.item(PopupMenuItem::new(lbl).icon(icon).disabled(dis).on_click(
                move |_, window, cx| run_batch_row_op(t.clone(), ords.clone(), op, window, cx),
            ))
        };
    let menu = menu
        .item(PopupMenuItem::new(title).disabled(true))
        .separator()
        .item(
            PopupMenuItem::new("发送到 Intruder")
                .icon(IconName::Crosshair)
                .action(Box::new(OpenInIntruder))
                .disabled(unavailable)
                .on_click(move |_, window, cx| {
                    if let Ok(entry) = &intruder_entry {
                        intruder_target.transfer_result(entry.clone(), true, window, cx);
                    }
                }),
        )
        .item(
            PopupMenuItem::new("发送到 Repeater")
                .icon(IconName::Repeat)
                .action(Box::new(OpenInRepeater))
                .disabled(unavailable)
                .on_click(move |_, window, cx| {
                    if let Ok(entries) = &repeater_entries {
                        repeater_target.transfer_results(entries.clone(), false, window, cx);
                    } else if let Ok(entry) = &entry {
                        repeater_target.transfer_result(entry.clone(), false, window, cx);
                    }
                }),
        );
    let menu = add_op(
        menu,
        "发起主动扫描 (Scan)",
        IconName::Activity,
        Box::new(crate::SendToScanner),
        unavailable,
        RowOp::Scanner,
    );
    let menu = add_op(
        menu,
        "发送到 Sequencer",
        IconName::Activity,
        Box::new(crate::SendToSequencer),
        unavailable,
        RowOp::Sequencer,
    );
    let menu = add_op(
        menu,
        "发送到 Organiser",
        IconName::Bookmark,
        Box::new(SendToOrganiser),
        unavailable,
        RowOp::Organiser,
    );
    let menu = add_op(
        menu,
        "发送到 Decoder",
        IconName::Binary,
        Box::new(SendToDecoder),
        unavailable,
        RowOp::Decoder,
    );
    let menu = add_op(
        menu,
        "发送到 Comparer (请求)",
        IconName::GitCompare,
        Box::new(SendToComparerRequest),
        unavailable,
        RowOp::Comparer(false),
    );
    let menu = add_op(
        menu,
        "发送到 Comparer (响应)",
        IconName::GitCompare,
        Box::new(SendToComparerResponse),
        unavailable || !has_resp,
        RowOp::Comparer(true),
    );
    let menu = add_plain_op(
        menu,
        "发送到 MioAT",
        IconName::Sparkles,
        unavailable,
        RowOp::SendToMioAi,
    );
    let menu = add_plain_op(
        menu,
        "记录问题",
        IconName::StickyNote,
        unavailable,
        RowOp::RecordIssue,
    );
    let menu = add_plain_op(
        menu,
        "生成 CSRF PoC",
        IconName::Code,
        unavailable,
        RowOp::GenerateCsrfPoc,
    );
    let menu = results::results_annotations::append_annotation_menu(
        &table.target,
        ordinal,
        menu,
        window,
        cx,
    )
    .separator();
    let menu = add_op(
        menu,
        "复制 URL",
        IconName::Link,
        Box::new(CopyUrl),
        unavailable,
        RowOp::CopyUrl,
    );
    let menu = add_op(
        menu,
        "复制为 curl 命令 (bash)",
        IconName::Terminal,
        Box::new(CopyCurlCommand),
        unavailable,
        RowOp::CopyCurl,
    );
    let menu = add_op(
        menu,
        "复制响应",
        IconName::Copy,
        Box::new(crate::CopyResponse),
        unavailable || !has_resp,
        RowOp::CopyResponse,
    );
    let tsv_target = table.target.clone();
    let has_tsv = selected_tsv.is_some();
    let menu = menu.item(
        PopupMenuItem::new("复制所选结果 (TSV)")
            .icon(IconName::Copy)
            .disabled(!has_tsv)
            .on_click(move |_, window, cx| {
                if let Some((tsv, count)) = &selected_tsv {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(tsv.clone()));
                    let count = *count;
                    tsv_target.update(window, cx, |workbench, _, cx| {
                        workbench.error = None;
                        workbench.status =
                            format!("已复制 {count} 条 Intruder 结果（TSV）到剪贴板。");
                        cx.notify();
                    });
                }
            }),
    );
    let menu = add_plain_op(
        menu,
        "保存条目",
        IconName::Download,
        unavailable,
        RowOp::SaveItem,
    )
    .separator();
    let delete_target = table.target.clone();
    let delete_ordinals = selected_ordinals.clone();
    let can_delete = !delete_ordinals.is_empty();
    let menu = menu
        .item(
            PopupMenuItem::new("删除条目")
                .icon(IconName::Trash)
                .disabled(!can_delete)
                .on_click(move |_, window, cx| {
                    let ords = delete_ordinals.clone();
                    delete_target.update(window, cx, |workbench, window, cx| {
                        workbench.remove_intruder_result_rows(
                            delete_target.view_id,
                            &ords,
                            window,
                            cx,
                        );
                    });
                }),
        )
        .separator();
    let menu = add_op(
        menu,
        "添加到范围",
        IconName::Plus,
        Box::new(crate::AddToScope),
        unavailable,
        RowOp::AddToScope,
    );
    add_op(
        menu,
        "从范围排除",
        IconName::Minus,
        Box::new(crate::ExcludeFromScope),
        unavailable,
        RowOp::ExcludeFromScope,
    )
}
