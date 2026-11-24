use base64::{Engine as _, engine::general_purpose::STANDARD as B64_STANDARD};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    input::{Editor, EditorState, Input, InputState},
};

use super::*;

mod explanation;
mod presets;
#[cfg(test)]
mod tests;

pub(super) use explanation::{ExplanationCacheKey, RepeaterExplanationReport};
use explanation::xml_escape_text;
use presets::*;

struct CustomActionsUiState {
    script_editor: Entity<EditorState>,
    script_name: Entity<InputState>,
    active_preset: Option<usize>,
    selected_saved: Option<String>,
    saved_scripts: Vec<CustomActionScriptEntry>,
    output: String,
    error: Option<String>,
    saved_notice: Option<String>,
}

fn fallback_target_from_raw(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let head = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .map_or(text.as_ref(), |(h, _)| h);
    for line in head.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("host")
            && !v.trim().is_empty()
        {
            return format!("http://{}", v.trim());
        }
    }
    "http://localhost".into()
}

fn detect_response_line_version(raw: &[u8], fallback: HttpVersion) -> HttpVersion {
    let end = raw
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(raw.len());
    let first = std::str::from_utf8(&raw[..end]).unwrap_or_default();
    match first.split_ascii_whitespace().next() {
        Some("HTTP/1.0") => HttpVersion::Http10,
        Some("HTTP/1.1") => HttpVersion::Http11,
        Some("HTTP/2" | "HTTP/2.0") => HttpVersion::Http2,
        _ => fallback,
    }
}

fn truncate_for_ai_context(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.chars().count() <= max_chars {
        text.into_owned()
    } else {
        let head: String = text.chars().take(max_chars).collect();
        format!("{head}\n… [已截断，原始总计 {} 字节]", bytes.len())
    }
}

impl Workbench {
    pub(in crate::workbench) fn open_active_repeater_in_mio_ai(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let http_payload = if let Some(tab_id) = self.workspace.active
            && let Some(editors) = self.tab_editors.get(&tab_id)
        {
            let tab_title = self
                .workspace
                .tabs
                .iter()
                .find(|t| t.id == tab_id)
                .map_or_else(|| "Repeater".to_owned(), |t| t.title.clone());
            let target = editing::input_snapshot(&editors.target, window, cx)
                .map(|s| s.trim().to_owned())
                .unwrap_or_else(|_| {
                    self.workspace
                        .tabs
                        .iter()
                        .find(|t| t.id == tab_id)
                        .map_or_else(String::new, |t| t.draft.target.clone())
                });
            let req_bytes = editors
                .request
                .bytes(window, cx)
                .unwrap_or_else(|_| editors.request.bytes_lossless(cx));
            let resp_bytes = editors.response.bytes_lossless(cx);
            let mut ctx = format!(
                "[Repeater 标签] {tab_title}\n[目标] {target}\n\n=== HTTP Request ===\n{}",
                truncate_for_ai_context(&req_bytes, 12_288)
            );
            if !resp_bytes.is_empty() {
                ctx.push_str("\n\n=== HTTP Response ===\n");
                ctx.push_str(&truncate_for_ai_context(&resp_bytes, 12_288));
            }
            Some((tab_title, ctx))
        } else {
            None
        };
        if let Some((tab_title, ctx)) = http_payload {
            self.open_mio_ai_with_context(tab_title, ctx, cx);
            return;
        }
        let ws_payload = if let Some(ws_id) = self.workspace.active_websocket_tab
            && let Some(ws_tab) = self
                .workspace
                .websocket_tabs
                .iter()
                .find(|t| !t.closed && t.id == ws_id)
        {
            let draft_bytes = self
                .websocket_repeater
                .editors
                .get(&ws_id)
                .map(|ed| ed.draft.bytes_lossless(cx))
                .unwrap_or_else(|| ws_tab.draft_payload.clone());
            let ctx = format!(
                "[WebSocket Repeater 标签] {}\n[URL] {}\n[方向] {:?}\n[历史帧数] {}\n\n=== Draft Payload ===\n{}",
                ws_tab.title,
                ws_tab.url,
                ws_tab.direction,
                ws_tab.history.len(),
                truncate_for_ai_context(&draft_bytes, 12_288)
            );
            Some((ws_tab.title.clone(), ctx))
        } else {
            None
        };
        if let Some((title, ctx)) = ws_payload {
            self.open_mio_ai_with_context(title, ctx, cx);
            return;
        }
        self.mio_ai_panel_open = true;
        cx.notify();
    }

    pub(in crate::workbench) fn save_repeater_entire_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_picker_pending()
            || !self.backend_available()
            || self.transition.is_some()
            || self.project.is_none()
        {
            return;
        }
        let (suggested_name, xml_payload, json_payload, count_label) =
            if let Some(tab_id) = self.workspace.active
                && let Some(editors) = self.tab_editors.get(&tab_id)
            {
                let tab_meta = self.workspace.tabs.iter().find(|t| t.id == tab_id);
                let title = tab_meta.map_or("repeater", |t| t.title.as_str()).to_owned();
                let target = editing::input_snapshot(&editors.target, window, cx)
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_else(|_| {
                        tab_meta.map_or_else(String::new, |t| t.draft.target.clone())
                    });
                let version = tab_meta.map_or(HttpVersion::Http11, |t| t.draft.version);
                let raw_req = editors
                    .request
                    .bytes(window, cx)
                    .unwrap_or_else(|_| editors.request.bytes_lossless(cx));
                let raw_resp = editors.response.bytes_lossless(cx);
                let effective_target = if target.is_empty() {
                    fallback_target_from_raw(&raw_req)
                } else {
                    target.clone()
                };
                let parsed_req = mio_core::parse_request(&raw_req, &effective_target, version).ok();
                let parsed_resp = if raw_resp.is_empty() {
                    editors
                        .loaded_flow
                        .as_ref()
                        .and_then(|f| f.response.clone())
                } else {
                    mio_core::parse_response(
                        &raw_resp,
                        detect_response_line_version(&raw_resp, version),
                    )
                    .ok()
                };
                let history_rows: Vec<mio_core::RepeaterHistoryEntry> = if self
                    .repeater_history
                    .owner
                    .is_some_and(|(_, id)| id == tab_id) { {
                        self.repeater_history
                            .page
                            .as_ref()
                            .map(|p| p.rows.clone())
                            .unwrap_or_default()
                    } } else { Default::default() };

                let url_str = parsed_req
                    .as_ref()
                    .map_or_else(|| effective_target.clone(), |r| r.url.clone());
                let parsed_url = url::Url::parse(&url_str).ok();
                let host = parsed_url
                    .as_ref()
                    .and_then(|u| u.host_str())
                    .unwrap_or("")
                    .to_owned();
                let port = parsed_url
                    .as_ref()
                    .and_then(|u| u.port_or_known_default())
                    .unwrap_or(80);
                let protocol = parsed_url
                    .as_ref()
                    .map_or("http", |u| u.scheme())
                    .to_owned();
                let method = parsed_req
                    .as_ref()
                    .map_or_else(|| "GET".to_owned(), |r| r.method.clone());
                let path = parsed_url
                    .as_ref()
                    .map_or_else(|| "/".to_owned(), |u| u.path().to_owned());
                let status = parsed_resp.as_ref().map_or(0u16, |r| r.status);
                let req_b64 = B64_STANDARD.encode(&raw_req);
                let resp_b64 = B64_STANDARD.encode(&raw_resp);

                let mut xml = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<items mioVersion=\"{}\">\n",
                    env!("CARGO_PKG_VERSION")
                );
                xml.push_str(&format!(
                    "  <item>\n    <title>{}</title>\n    <url>{}</url>\n    <host>{}</host>\n    <port>{}</port>\n    <protocol>{}</protocol>\n    <method>{}</method>\n    <path>{}</path>\n    <request base64=\"true\">{}</request>\n    <status>{}</status>\n    <responselength>{}</responselength>\n    <response base64=\"true\">{}</response>\n  </item>\n",
                    xml_escape_text(&title),
                    xml_escape_text(&url_str),
                    xml_escape_text(&host),
                    port,
                    xml_escape_text(&protocol),
                    xml_escape_text(&method),
                    xml_escape_text(&path),
                    req_b64,
                    status,
                    raw_resp.len(),
                    resp_b64,
                ));
                for row in &history_rows {
                    xml.push_str(&format!(
                        "  <historyEntry position=\"{}\">\n    <method>{}</method>\n    <url>{}</url>\n    <status>{}</status>\n    <diagnostic>{}</diagnostic>\n  </historyEntry>\n",
                        row.position + 1,
                        xml_escape_text(row.method.as_deref().unwrap_or("")),
                        xml_escape_text(row.target.as_deref().unwrap_or("")),
                        row.status.map_or_else(String::new, |s| s.to_string()),
                        xml_escape_text(row.diagnostic.as_deref().unwrap_or("")),
                    ));
                }
                xml.push_str("</items>\n");

                let json_val = serde_json::json!({
                    "tab_title": title,
                    "target": effective_target,
                    "url": url_str,
                    "method": method,
                    "status": status,
                    "request_raw": String::from_utf8_lossy(&raw_req),
                    "response_raw": String::from_utf8_lossy(&raw_resp),
                    "history_entries": history_rows.iter().map(|r| serde_json::json!({
                        "position": r.position + 1,
                        "method": r.method,
                        "target": r.target,
                        "status": r.status,
                        "diagnostic": r.diagnostic,
                    })).collect::<Vec<_>>(),
                });
                let json = serde_json::to_string_pretty(&json_val).unwrap_or_default();
                let count = 1 + history_rows.len();
                (
                    "repeater-history.xml",
                    xml.into_bytes(),
                    json.into_bytes(),
                    format!("Repeater 完整历史（共 {count} 条）"),
                )
            } else if let Some(ws_id) = self.workspace.active_websocket_tab
                && let Some(ws_tab) = self
                    .workspace
                    .websocket_tabs
                    .iter()
                    .find(|t| !t.closed && t.id == ws_id)
            {
                let mut xml = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<websocketHistory mioVersion=\"{}\">\n",
                    env!("CARGO_PKG_VERSION")
                );
                for item in &ws_tab.history {
                    xml.push_str(&format!(
                        "  <frame sequence=\"{}\" direction=\"{:?}\" kind=\"{:?}\" timestamp=\"{}\">\n    <url>{}</url>\n    <payload base64=\"true\">{}</payload>\n  </frame>\n",
                        item.sequence,
                        item.direction,
                        item.kind,
                        item.observed_unix_ms,
                        xml_escape_text(&ws_tab.url),
                        B64_STANDARD.encode(&item.payload),
                    ));
                }
                xml.push_str("</websocketHistory>\n");
                let json_val = serde_json::json!({
                    "tab_title": ws_tab.title,
                    "url": ws_tab.url,
                    "frames": ws_tab.history.iter().map(|item| serde_json::json!({
                        "sequence": item.sequence,
                        "direction": format!("{:?}", item.direction),
                        "kind": format!("{:?}", item.kind),
                        "observed_unix_ms": item.observed_unix_ms,
                        "payload": String::from_utf8_lossy(&item.payload),
                    })).collect::<Vec<_>>(),
                });
                let json = serde_json::to_string_pretty(&json_val).unwrap_or_default();
                let count = ws_tab.history.len();
                (
                    "ws-repeater-history.xml",
                    xml.into_bytes(),
                    json.into_bytes(),
                    format!("WebSocket Repeater 完整历史（共 {count} 帧）"),
                )
            } else {
                self.fail("当前没有可保存历史的 Repeater 标签页。", cx);
                return;
            };

        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(suggested_name),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let pending_write = owner
                .update(cx, |_, _, cx| {
                    this.update(cx, |view, cx| {
                        if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                            return None;
                        }
                        if view.project != project {
                            view.fail("项目已切换，本次历史保存已取消。", cx);
                            return None;
                        }
                        match result {
                            Ok(Ok(Some(mut dest))) => {
                                if dest.extension().is_none() {
                                    dest.set_extension("xml");
                                }
                                let is_json = dest
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .is_some_and(|e| e.eq_ignore_ascii_case("json"));
                                let bytes = if is_json { json_payload } else { xml_payload };
                                Some((dest, bytes))
                            }
                            Ok(Ok(None)) => None,
                            Ok(Err(err)) => {
                                view.fail(format!("无法选择保存路径：{err}"), cx);
                                None
                            }
                            Err(_) => {
                                view.fail("文件选择器已意外关闭。", cx);
                                None
                            }
                        }
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some((dest, bytes)) = pending_write else {
                return;
            };
            let write_dest = dest.clone();
            let write_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::write_export_file(&write_dest, &bytes)
                })
                .await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if view.project != project {
                        return;
                    }
                    match write_result {
                        Ok(()) => {
                            view.error = None;
                            view.status = format!("已保存{count_label}至：{}", dest.display());
                            cx.notify();
                        }
                        Err(err) => {
                            view.fail(format!("保存 Repeater 历史失败：{err}"), cx);
                        }
                    }
                })
            });
        })
        .detach();
    }

    #[inline(never)]
    fn render_custom_actions_header(
        &self,
        ui_state: &Entity<CustomActionsUiState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state_for_new = ui_state.clone();
        toolbar()
            .flex_nowrap()
            .gap(px(5.))
            .px(px(8.))
            .child(
                gpui_kit::component::Icon::new(IconName::Zap)
                    .size(theme::scaled_px(13.))
                    .text_color(theme::accent()),
            )
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Custom actions"),
            )
            .child(div().flex_1())
            .child(
                Button::new("repeater-custom-action-new")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(22.))
                    .px(px(6.))
                    .label("新建")
                    .tooltip("清空为自定义请求修改 + 日志输出示例模板")
                    .on_click(cx.listener(move |_, _, window, cx| {
                        state_for_new.update(cx, |st, cx| {
                            st.active_preset = None;
                            st.selected_saved = None;
                            st.error = None;
                            st.saved_notice = None;
                            st.script_name.update(cx, |inp, cx| {
                                inp.set_value("", window, cx);
                                inp.focus(window, cx);
                            });
                            st.script_editor.update(cx, |ed, cx| {
                                ed.set_value(STARTER_CUSTOM_ACTION, window, cx);
                            });
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("repeater-custom-actions-close")
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(24.))
                    .h(theme::scaled_px(24.))
                    .icon(IconName::Close)
                    .accessibility_label("关闭 Custom actions")
                    .tooltip("收起 Repeater Custom actions 侧栏")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.repeater_custom_actions_open = false;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_custom_actions_preset_group(
        &self,
        title: &'static str,
        category: CustomActionCategory,
        ui_state: &Entity<CustomActionsUiState>,
        active_preset: Option<usize>,
        has_http_tab: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut chips = div().flex().flex_wrap().items_center().gap(px(4.));
        for (idx, preset) in CUSTOM_ACTION_PRESETS.into_iter().enumerate() {
            if preset.category != category {
                continue;
            }
            let state_load = ui_state.clone();
            let state_exec = ui_state.clone();
            let is_active = active_preset == Some(idx);
            chips = chips.child(
                div()
                    .flex()
                    .items_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if is_active {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(if is_active {
                        theme::selected()
                    } else {
                        theme::toolbar_surface()
                    })
                    .overflow_hidden()
                    .child(
                        Button::new(preset.id)
                            .ghost()
                            .small()
                            .compact()
                            .rounded(ButtonRounded::None)
                            .h(theme::scaled_px(21.))
                            .px(px(6.))
                            .label(preset.label)
                            .tooltip(format!("载入脚本：{}", preset.description))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                state_load.update(cx, |st, cx| {
                                    st.active_preset = Some(idx);
                                    st.selected_saved = None;
                                    st.error = None;
                                    st.saved_notice = None;
                                    st.script_editor.update(cx, |ed, cx| {
                                        ed.set_value(preset.script, window, cx);
                                    });
                                    st.script_name.update(cx, |inp, cx| {
                                        inp.set_value(preset.label, window, cx);
                                    });
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{}-run", preset.id)))
                            .ghost()
                            .small()
                            .compact()
                            .rounded(ButtonRounded::None)
                            .w(theme::scaled_px(20.))
                            .h(theme::scaled_px(21.))
                            .border_l_1()
                            .border_color(theme::border())
                            .icon(IconName::Play)
                            .disabled(!has_http_tab)
                            .accessibility_label(format!("立即执行：{}", preset.label))
                            .tooltip(format!("载入并立即执行「{}」", preset.label))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                state_exec.update(cx, |st, cx| {
                                    st.active_preset = Some(idx);
                                    st.selected_saved = None;
                                    st.script_editor.update(cx, |ed, cx| {
                                        ed.set_value(preset.script, window, cx);
                                    });
                                    st.script_name.update(cx, |inp, cx| {
                                        inp.set_value(preset.label, window, cx);
                                    });
                                });
                                this.execute_custom_action_into_ui(
                                    preset.script,
                                    &state_exec,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .text_size(theme::scaled_px(11.))
                    .text_color(theme::muted_foreground())
                    .child(title),
            )
            .child(chips)
            .into_any_element()
    }

    #[inline(never)]
    fn render_custom_actions_saved_section(
        &self,
        ui_state: &Entity<CustomActionsUiState>,
        saved_scripts: &[CustomActionScriptEntry],
        selected_saved: Option<&str>,
        has_http_tab: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut saved_section = div()
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(theme::scaled_px(11.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(theme::muted_foreground())
                            .child(format!("我的自定义脚本 ({})", saved_scripts.len())),
                    ),
            );
        if saved_scripts.is_empty() {
            saved_section = saved_section.child(
                div()
                    .id("repeater-custom-actions-saved-empty")
                    .test_support()
                    .text_size(theme::scaled_px(11.))
                    .text_color(theme::muted_foreground())
                    .child("暂无已保存的自定义脚本，在下方编辑名称与代码后点击「保存脚本」。"),
            );
        } else {
            let mut list = div()
                .id("repeater-custom-actions-saved-list")
                .test_support()
                .max_h(px(86.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(2.));
            for (idx, entry) in saved_scripts.iter().enumerate() {
                let entry_name_load = entry.name.clone();
                let entry_code_load = entry.code.clone();
                let entry_name_run = entry.name.clone();
                let entry_code_run = entry.code.clone();
                let entry_name_del = entry.name.clone();
                let is_sel = selected_saved == Some(entry.name.as_str());
                let st_load = ui_state.clone();
                let st_run = ui_state.clone();
                let st_del = ui_state.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!(
                            "repeater-custom-action-saved-row-{idx}"
                        )))
                        .test_support()
                        .h(theme::scaled_px(22.))
                        .px(px(6.))
                        .rounded(px(3.))
                        .bg(if is_sel {
                            theme::selected()
                        } else {
                            theme::toolbar_surface()
                        })
                        .border_1()
                        .border_color(if is_sel {
                            theme::accent()
                        } else {
                            theme::border()
                        })
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "repeater-custom-action-saved-select-{idx}"
                                )))
                                .flex_1()
                                .min_w_0()
                                .cursor_pointer()
                                .truncate()
                                .text_size(theme::scaled_px(11.5))
                                .text_color(theme::foreground())
                                .child(entry.name.clone())
                                .on_click(cx.listener(move |_, _, window, cx| {
                                    let name = entry_name_load.clone();
                                    let code = entry_code_load.clone();
                                    st_load.update(cx, |st, cx| {
                                        st.active_preset = None;
                                        st.selected_saved = Some(name.clone());
                                        st.error = None;
                                        st.saved_notice = None;
                                        st.script_name.update(cx, |inp, cx| {
                                            inp.set_value(name, window, cx);
                                        });
                                        st.script_editor.update(cx, |ed, cx| {
                                            ed.set_value(code, window, cx);
                                        });
                                    });
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "repeater-custom-action-saved-run-{idx}"
                            )))
                            .ghost()
                            .small()
                            .compact()
                            .w(theme::scaled_px(20.))
                            .h(theme::scaled_px(18.))
                            .icon(IconName::Play)
                            .disabled(!has_http_tab)
                            .accessibility_label("运行已保存脚本")
                            .tooltip("立即在当前请求上执行此自定义脚本")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let name = entry_name_run.clone();
                                let code = entry_code_run.clone();
                                st_run.update(cx, |st, cx| {
                                    st.active_preset = None;
                                    st.selected_saved = Some(name.clone());
                                    st.script_name.update(cx, |inp, cx| {
                                        inp.set_value(name, window, cx);
                                    });
                                    st.script_editor.update(cx, |ed, cx| {
                                        ed.set_value(code.clone(), window, cx);
                                    });
                                });
                                this.execute_custom_action_into_ui(&code, &st_run, window, cx);
                            })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "repeater-custom-action-saved-del-{idx}"
                            )))
                            .ghost()
                            .small()
                            .compact()
                            .w(theme::scaled_px(20.))
                            .h(theme::scaled_px(18.))
                            .icon(IconName::Close)
                            .accessibility_label("删除已保存脚本")
                            .tooltip("从我的自定义脚本中删除")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let name = entry_name_del.clone();
                                let current = st_del.read(cx).saved_scripts.clone();
                                let res = custom_actions_library_path().and_then(|lib_path| {
                                    delete_custom_action_from_path(
                                        lib_path.as_deref(),
                                        &current,
                                        &name,
                                    )
                                });
                                match res {
                                    Ok(next) => {
                                        st_del.update(cx, |st, _| {
                                            st.saved_scripts = next;
                                            if st.selected_saved.as_deref() == Some(&name) {
                                                st.selected_saved = None;
                                            }
                                            st.error = None;
                                            st.saved_notice =
                                                Some(format!("已删除自定义脚本：{name}"));
                                        });
                                        this.status = format!("已删除自定义脚本：{name}");
                                    }
                                    Err(err) => {
                                        st_del.update(cx, |st, _| {
                                            st.saved_notice = None;
                                            st.error = Some(format!("{err:#}"));
                                        });
                                    }
                                }
                                cx.notify();
                            })),
                        ),
                );
            }
            saved_section = saved_section.child(list);
        }
        saved_section.into_any_element()
    }

    #[inline(never)]
    fn render_custom_actions_name_bar(
        &self,
        ui_state: &Entity<CustomActionsUiState>,
        script_name: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state_for_save = ui_state.clone();
        div()
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(script_name)
                        .id("repeater-custom-action-name")
                        .aria_label("自定义脚本名称")
                        .small(),
                ),
            )
            .child(
                button("repeater-custom-action-save", "保存脚本")
                    .icon(IconName::Save)
                    .tooltip("校验并保存到本地「我的自定义脚本」(repeater-custom-actions.json)")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let (name_entity, editor_entity, current_scripts) = {
                            let st = state_for_save.read(cx);
                            (
                                st.script_name.clone(),
                                st.script_editor.clone(),
                                st.saved_scripts.clone(),
                            )
                        };
                        let name_res =
                            editing::input_snapshot(&name_entity, window, cx).map(|s| s.to_string());
                        let code_res =
                            editing::snapshot(&editor_entity, window, cx).map(|s| s.to_string());
                        let result = name_res
                            .map_err(|e| anyhow::anyhow!(e))
                            .and_then(|name| {
                                let code = code_res.map_err(|e| anyhow::anyhow!(e))?;
                                let lib_path = custom_actions_library_path()?;
                                save_custom_action_to_path(
                                    lib_path.as_deref(),
                                    &current_scripts,
                                    &name,
                                    &code,
                                )
                                .map(|next| (name.trim().to_owned(), next))
                            });
                        match result {
                            Ok((name, next)) => {
                                let notice = format!("已保存自定义脚本：{name}");
                                state_for_save.update(cx, |st, _| {
                                    st.active_preset = None;
                                    st.selected_saved = Some(name);
                                    st.saved_scripts = next;
                                    st.error = None;
                                    st.saved_notice = Some(notice.clone());
                                });
                                this.error = None;
                                this.status = notice;
                            }
                            Err(err) => {
                                state_for_save.update(cx, |st, _| {
                                    st.saved_notice = None;
                                    st.error = Some(format!("{err:#}"));
                                });
                            }
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_custom_actions_run_bar(
        &self,
        ui_state: &Entity<CustomActionsUiState>,
        has_http_tab: bool,
        has_output: bool,
        has_error: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state_for_run = ui_state.clone();
        let state_for_copy = ui_state.clone();
        let state_for_clear = ui_state.clone();
        div()
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::toolbar_surface())
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                button("repeater-custom-action-run", "运行 (Run)")
                    .primary()
                    .icon(IconName::Play)
                    .disabled(!has_http_tab)
                    .tooltip("针对当前激活的 Repeater 请求与响应执行上方脚本（如修改 request 将直接更新当前请求）")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let editor_entity = state_for_run.read(cx).script_editor.clone();
                        let script = match editing::snapshot(&editor_entity, window, cx) {
                            Ok(s) => s.to_string(),
                            Err(err) => {
                                state_for_run.update(cx, |st, _| {
                                    st.error = Some(err.to_owned());
                                });
                                cx.notify();
                                return;
                            }
                        };
                        this.execute_custom_action_into_ui(&script, &state_for_run, window, cx);
                    })),
            )
            .child(
                button("repeater-custom-action-copy-output", "复制输出")
                    .icon(IconName::Copy)
                    .disabled(!has_output)
                    .tooltip("将自定义动作脚本的输出结果复制到剪贴板")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let text = state_for_copy.read(cx).output.clone();
                        if !text.is_empty() {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                            this.status = "已复制 Custom actions 输出到剪贴板".into();
                            cx.notify();
                        }
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("repeater-custom-action-clear")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(22.))
                    .px(px(6.))
                    .label("清空")
                    .disabled(!has_output && !has_error)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        state_for_clear.update(cx, |st, _| {
                            st.output.clear();
                            st.error = None;
                            st.saved_notice = None;
                        });
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_repeater_custom_actions_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui_state = window.use_keyed_state("repeater-custom-actions-ui", cx, |window, cx| {
            let script_editor = cx.new(|ecx| {
                let mut ed = EditorState::new(window, ecx)
                    .language("javascript")
                    .line_number(true)
                    .soft_wrap(true)
                    .auto_close(false)
                    .autocorrect(false);
                ed.set_value(PRESET_REMOVE_COOKIE, window, ecx);
                ed
            });
            let script_name = cx.new(|icx| {
                InputState::new(window, icx)
                    .default_value("去除 Cookie")
                    .placeholder("自定义脚本名称（保存到本地脚本库）")
            });
            let saved_scripts = custom_actions_library_path()
                .ok()
                .flatten()
                .and_then(|path| read_custom_action_library_from_path(&path).ok())
                .unwrap_or_default();
            CustomActionsUiState {
                script_editor,
                script_name,
                active_preset: Some(0),
                selected_saved: None,
                saved_scripts,
                output: String::new(),
                error: None,
                saved_notice: None,
            }
        });

        let (
            script_editor,
            script_name,
            active_preset,
            selected_saved,
            saved_scripts,
            output,
            error,
            saved_notice,
        ) = {
            let st = ui_state.read(cx);
            (
                st.script_editor.clone(),
                st.script_name.clone(),
                st.active_preset,
                st.selected_saved.clone(),
                st.saved_scripts.clone(),
                st.output.clone(),
                st.error.clone(),
                st.saved_notice.clone(),
            )
        };
        let has_http_tab = self
            .workspace
            .active
            .is_some_and(|id| self.tab_editors.contains_key(&id));
        let dock_left = self.inspector.dock_left;
        let header = self.render_custom_actions_header(&ui_state, cx);
        let presets = div()
            .id("repeater-custom-actions-presets-pane")
            .max_h(px(164.))
            .flex_shrink_0()
            .overflow_y_scroll()
            .p(px(8.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.render_custom_actions_preset_group(
                "修改当前请求 (Rhai 原地修改草稿 · 点 ▶ 立即执行)：",
                CustomActionCategory::MutateRequest,
                &ui_state,
                active_preset,
                has_http_tab,
                cx,
            ))
            .child(self.render_custom_actions_preset_group(
                "分析与提取 (解析请求/响应并输出到下方)：",
                CustomActionCategory::Inspect,
                &ui_state,
                active_preset,
                has_http_tab,
                cx,
            ));
        let saved_section = self.render_custom_actions_saved_section(
            &ui_state,
            &saved_scripts,
            selected_saved.as_deref(),
            has_http_tab,
            cx,
        );
        let name_bar = self.render_custom_actions_name_bar(&ui_state, &script_name, cx);
        let run_bar = self.render_custom_actions_run_bar(
            &ui_state,
            has_http_tab,
            !output.is_empty(),
            error.is_some(),
            cx,
        );

        div()
            .id("repeater-custom-actions-sidebar")
            .test_support()
            .w(px(352.))
            .flex_shrink_0()
            .h_full()
            .when(dock_left, |d| d.border_r_1())
            .when(!dock_left, |d| d.border_l_1())
            .border_color(theme::border())
            .bg(theme::surface())
            .flex()
            .flex_col()
            .child(header)
            .child(presets)
            .child(saved_section)
            .child(name_bar)
            .child(
                div()
                    .h(px(155.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        Editor::new(&script_editor)
                            .size_full()
                            .bordered(false)
                            .aria_label("Custom actions Rhai 脚本编辑器"),
                    ),
            )
            .child(run_bar)
            .when_some(error, |col, err| {
                col.child(
                    div()
                        .id("repeater-custom-action-error")
                        .test_support()
                        .px(px(8.))
                        .py(px(5.))
                        .bg(theme::danger_bg())
                        .border_b_1()
                        .border_color(theme::border())
                        .text_size(theme::scaled_px(11.5))
                        .text_color(theme::danger_fg())
                        .child(err),
                )
            })
            .when_some(saved_notice, |col, msg| {
                col.child(
                    div()
                        .id("repeater-custom-action-saved")
                        .test_support()
                        .px(px(8.))
                        .py(px(4.))
                        .border_b_1()
                        .border_color(theme::border())
                        .text_size(theme::scaled_px(11.5))
                        .text_color(rgb(if theme::is_dark() { 0x7ee787 } else { 0x137333 }))
                        .child(msg),
                )
            })
            .child(
                div()
                    .id("repeater-custom-action-output")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(8.))
                    .bg(theme::background())
                    .font_family(theme::mono_font_family())
                    .text_size(theme::scaled_px(11.5))
                    .text_color(if output.is_empty() {
                        theme::muted_foreground()
                    } else {
                        theme::foreground()
                    })
                    .child(if output.is_empty() {
                        if has_http_tab {
                            "点击模板「▶」或下方「运行 (Run)」执行 Rhai 脚本；若修改 request（如去除 Cookie）将同步更新当前请求草稿。"
                                .to_owned()
                        } else {
                            "请先激活一个 HTTP Repeater 标签页以运行 Custom actions 脚本。"
                                .to_owned()
                        }
                    } else {
                        output
                    }),
            )
            .into_any_element()
    }

    fn execute_custom_action_into_ui(
        &mut self,
        script: &str,
        ui_state: &Entity<CustomActionsUiState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.run_custom_action_on_active_tab(script, window, cx) {
            Ok((out, mutated)) => {
                ui_state.update(cx, |st, _| {
                    st.output = out;
                    st.error = None;
                    st.saved_notice = None;
                });
                self.status = if mutated {
                    "已应用 Custom action 并更新当前 Repeater 请求".into()
                } else {
                    "Repeater Custom actions 脚本执行完成".into()
                };
            }
            Err(err) => {
                ui_state.update(cx, |st, _| {
                    st.error = Some(err);
                    st.saved_notice = None;
                });
            }
        }
        cx.notify();
    }

    fn run_custom_action_on_active_tab(
        &mut self,
        script: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(String, bool), String> {
        let tab_id = self
            .workspace
            .active
            .ok_or_else(|| "当前没有激活的 HTTP Repeater 标签页。".to_owned())?;
        let editors = self
            .tab_editors
            .get(&tab_id)
            .ok_or_else(|| "当前 Repeater 编辑器尚未就绪。".to_owned())?;
        let tab_meta = self.workspace.tabs.iter().find(|t| t.id == tab_id);
        let raw_req = editors
            .request
            .bytes(window, cx)
            .map_err(|e| format!("读取请求草稿失败：{e}"))?;
        let target = editing::input_snapshot(&editors.target, window, cx)
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|_| tab_meta.map_or_else(String::new, |t| t.draft.target.clone()));
        let effective_target = if target.is_empty() {
            fallback_target_from_raw(&raw_req)
        } else {
            target.clone()
        };
        let fallback_ver = tab_meta.map_or(HttpVersion::Http11, |t| t.draft.version);
        let req_version =
            mio_core::detect_request_line_version(&raw_req).unwrap_or(fallback_ver);
        let parsed_req = mio_core::parse_request(&raw_req, &effective_target, req_version)
            .map_err(|err| format!("解析当前 HTTP 请求失败：{err:#}"))?;

        let raw_resp = editors.response.bytes_lossless(cx);
        let parsed_resp = if !raw_resp.is_empty() {
            let resp_ver = detect_response_line_version(&raw_resp, req_version);
            mio_core::parse_response(&raw_resp, resp_ver).ok()
        } else {
            editors
                .loaded_flow
                .as_ref()
                .and_then(|flow| flow.response.clone())
        };
        let outcome = mio_core::eval_custom_action_with_mutation(
            script,
            &parsed_req,
            parsed_resp.as_ref(),
        )
        .map_err(|err| format!("{err:#}"))?;
        let mutated = outcome.updated_request.is_some();
        if let Some(updated) = outcome.updated_request {
            if !self.can_send(tab_id) || self.extensions.active_messages.contains_key(&tab_id) {
                return Err("当前标签正在发送请求或由扩展消息编辑器接管，无法修改请求报文。".into());
            }
            let next_raw = mio_core::request_to_raw(&updated);
            if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                let was_pretty = editors.request.pretty_active();
                let prev_view = editors.request.view();
                editors.request.load(&next_raw, window, cx);
                if was_pretty {
                    let _ = editors.request.switch_pretty(window, cx);
                } else if prev_view == crate::editing::MessageView::Hex
                    && editors.request.view() != crate::editing::MessageView::Hex
                {
                    let _ = editors
                        .request
                        .switch(crate::editing::MessageView::Hex, window, cx);
                }
                editors.dirty = true;
            }
            if target.is_empty()
                && let Ok(parsed_url) = url::Url::parse(&updated.url)
                && let Some(host) = parsed_url.host_str()
            {
                let mut next_origin = format!("{}://{host}", parsed_url.scheme());
                if let Some(port) = parsed_url.port() {
                    next_origin.push_str(&format!(":{port}"));
                }
                if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                    editors
                        .target
                        .update(cx, |inp, cx| inp.set_value(next_origin, window, cx));
                }
            }
        }
        Ok((outcome.output, mutated))
    }
}
