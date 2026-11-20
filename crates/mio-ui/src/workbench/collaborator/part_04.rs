use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    input::{Copy, EditorState},
    native_menu::NativeMenu,
};

const COLLABORATOR_MAX_DECODER_BYTES: usize = 512 * 1024;
const COLLABORATOR_MAX_COMPARER_ITEMS: usize = 32;
const COLLABORATOR_MAX_COMPARER_ITEM_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
enum CollaboratorRawMenuAction {
    SendToRepeater,
    SendToIntruder,
    SendToComparer,
    SendToDecoder,
    CopyCurl,
    Save,
}

fn collaborator_raw_editor_context_menu(
    input: Entity<EditorState>,
    can_operate: bool,
    has_selected: bool,
    is_http: bool,
    raw_empty: bool,
    file_picker_pending: bool,
) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
    move |menu, window, cx| {
        input.update(cx, |state, cx| state.focus(window, cx));
        let selected = !input.read(cx).selected_range().is_empty();
        let mut menu = menu
            .menu_with_disabled(
                "发送到 Repeater",
                !can_operate || !is_http || raw_empty,
                Box::new(CollaboratorRawMenuAction::SendToRepeater),
            )
            .menu_with_disabled(
                "发送到 Intruder",
                !can_operate || !is_http || raw_empty,
                Box::new(CollaboratorRawMenuAction::SendToIntruder),
            )
            .menu_with_disabled(
                "发送到 Comparer",
                !has_selected || raw_empty,
                Box::new(CollaboratorRawMenuAction::SendToComparer),
            )
            .menu_with_disabled(
                "发送到 Decoder",
                !has_selected || raw_empty,
                Box::new(CollaboratorRawMenuAction::SendToDecoder),
            )
            .separator()
            .menu_with_disabled("复制", !selected, Box::new(Copy));
        if is_http {
            menu = menu.menu_with_disabled(
                "复制为 curl 命令 (bash)",
                raw_empty,
                Box::new(CollaboratorRawMenuAction::CopyCurl),
            );
        }
        menu.separator().menu_with_disabled(
            "保存…",
            !can_operate || !has_selected || file_picker_pending,
            Box::new(CollaboratorRawMenuAction::Save),
        )
    }
}

fn collaborator_view_tab(
    id: &'static str,
    label: &'static str,
    width: f32,
    selected: bool,
) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .rounded(ButtonRounded::None)
        .relative()
        .self_start()
        .w(px(width))
        .h(px(37.))
        .p_0()
        .bg(theme::surface())
        .text_color(if selected {
            if theme::is_dark() {
                theme::foreground()
            } else {
                rgb(0x000000).into()
            }
        } else if theme::is_dark() {
            theme::muted_foreground()
        } else {
            rgb(0x1f1f1f).into()
        })
        .selected(selected)
        .toggled(selected)
        .accessibility_label(label)
        .child(
            div()
                .text_size(px(13.))
                .font_weight(if selected {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .child(label),
        )
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(3.))
                    .bg(rgb(0xdb612f)),
            )
        })
}

impl Workbench {
    fn collaborator_interaction_by_id(&self, interaction_id: Option<Uuid>) -> Option<OastInteraction> {
        let target_id = interaction_id.or(self.collaborator_pane.selected_interaction)?;
        self.collaborator_pane
            .status
            .interactions
            .iter()
            .find(|item| item.id == target_id)
            .cloned()
    }

    fn collaborator_http_draft(&self, interaction: &OastInteraction) -> Draft {
        let raw_text = String::from_utf8_lossy(&interaction.raw_request);
        let host_header = raw_text
            .lines()
            .skip(1)
            .take_while(|line| !line.trim_end_matches('\r').is_empty())
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("host")
                    .then(|| v.trim().to_owned())
                    .filter(|h| !h.is_empty())
            });
        let target = if let Some(host) = host_header {
            if host.contains("://") {
                host
            } else {
                format!("http://{host}")
            }
        } else {
            self.collaborator_pane
                .status
                .http_addr
                .map(|addr| format!("http://{addr}"))
                .unwrap_or_else(|| "http://127.0.0.1:8080".to_owned())
        };
        Draft {
            revision: 1,
            target,
            raw: interaction.raw_request.clone(),
            version: HttpVersion::Http11,
        }
    }

    pub(in crate::workbench) fn send_collaborator_to_repeater(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_collaborator_interaction_to_repeater(None, window, cx);
    }

    pub(in crate::workbench) fn send_collaborator_interaction_to_repeater(
        &mut self,
        interaction_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() {
            return;
        }
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        let Some(interaction) = self
            .collaborator_interaction_by_id(interaction_id)
            .filter(|item| item.protocol == OastProtocol::Http && !item.raw_request.is_empty())
        else {
            return;
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        let draft = self.collaborator_http_draft(&interaction);
        let id = Uuid::new_v4();
        let previous = self.workspace.active;
        let title = self.next_repeater_tab_title(None, Some(&draft.raw));
        self.workspace.tabs.push(WorkTab {
            id,
            title,
            group: None,
            draft,
            last_flow: None,
            archive_response: None,
            history: None,
        });
        self.workspace.active = Some(id);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.tabs.pop();
            self.workspace.active = previous;
            self.fail_collaborator(format!("未创建 Repeater 标签：{error}"), cx);
            return;
        }
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        self.tool = Tool::Repeater;
        self.tab_editors[&id]
            .request
            .state()
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn send_collaborator_interaction_to_intruder(
        &mut self,
        interaction_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() {
            return;
        }
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        let Some(interaction) = self
            .collaborator_interaction_by_id(interaction_id)
            .filter(|item| item.protocol == OastProtocol::Http && !item.raw_request.is_empty())
        else {
            return;
        };
        let draft = self.collaborator_http_draft(&interaction);
        self.open_intruder_draft(draft, window, cx);
    }

    pub(in crate::workbench) fn send_collaborator_interaction_to_comparer(
        &mut self,
        interaction_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(interaction) = self
            .collaborator_interaction_by_id(interaction_id)
            .filter(|item| !item.raw_request.is_empty())
        else {
            return;
        };
        let bytes = interaction.raw_request;
        if bytes.len() > COLLABORATOR_MAX_COMPARER_ITEM_BYTES {
            self.fail_collaborator(
                format!(
                    "Comparer 单项超过 {COLLABORATOR_MAX_COMPARER_ITEM_BYTES} 字节上限，未添加。"
                ),
                cx,
            );
            return;
        }
        if self.extra_tools.comparer_items.len() >= COLLABORATOR_MAX_COMPARER_ITEMS {
            self.fail_collaborator(
                format!("Comparer 最多保存 {COLLABORATOR_MAX_COMPARER_ITEMS} 项，未添加。"),
                cx,
            );
            return;
        }
        let id = self
            .extra_tools
            .comparer_items
            .last()
            .map_or(1, |item| item.id + 1);
        let proto = match interaction.protocol {
            OastProtocol::Http => "HTTP",
            OastProtocol::Dns => "DNS",
        };
        let label = format!("{id}: Collaborator {proto} ({} bytes)", bytes.len());
        self.extra_tools
            .comparer_items
            .push(crate::workbench::tools_extra::ComparerItem { id, label, bytes });
        let n = self.extra_tools.comparer_items.len();
        self.extra_tools.comparer_selected = n - 1;
        if n >= 2 {
            (self.extra_tools.left_index, self.extra_tools.right_index) = (n - 2, n - 1);
        }
        self.refresh_comparer(window, cx);
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Comparer;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn send_collaborator_interaction_to_decoder(
        &mut self,
        interaction_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(interaction) = self
            .collaborator_interaction_by_id(interaction_id)
            .filter(|item| !item.raw_request.is_empty())
        else {
            return;
        };
        if let Err(err) = editing::snapshot(&self.extra_tools.decoder_input, window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        let bytes = interaction.raw_request;
        if bytes.len() > COLLABORATOR_MAX_DECODER_BYTES {
            self.fail_collaborator(
                format!(
                    "Decoder 输入超过 {COLLABORATOR_MAX_DECODER_BYTES} 字节上限，未覆盖原内容。"
                ),
                cx,
            );
            return;
        }
        let (disp, bak) = String::from_utf8(bytes.clone()).map_or_else(
            |_| (editing::hex_encode(&bytes), Some(bytes)),
            |t| (t, None),
        );
        self.extra_tools
            .decoder_input
            .update(cx, |s, cx| s.set_value(disp, window, cx));
        let rev = self.extra_tools.decoder_input.read(cx).document_revision();
        self.extra_tools.decoder_input_bytes = bak.map(|b| (rev, b));
        self.extra_tools
            .decoder_output
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.extra_tools.decoder_output_bytes = None;
        self.extra_tools.decoder_mode = "".into();
        self.extra_tools.steps.clear();
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Decoder;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn send_collaborator_interaction_to_mio_ai(
        &mut self,
        interaction_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) {
        let Some(interaction) = self.collaborator_interaction_by_id(interaction_id) else {
            return;
        };
        let proto = match interaction.protocol {
            OastProtocol::Http => "HTTP",
            OastProtocol::Dns => "DNS",
        };
        let token = interaction.token.as_deref().unwrap_or("未匹配 Token");
        let raw_repr = if interaction.protocol == OastProtocol::Dns {
            editing::hex_encode(&interaction.raw_request)
        } else {
            String::from_utf8_lossy(&interaction.raw_request).into_owned()
        };
        let context = format!(
            "Collaborator 交互 ({proto})\nToken: {token}\n来源: {}\n摘要: {}\n\n--- Raw ---\n{}",
            interaction.peer_addr,
            interaction.summary,
            raw_repr.chars().take(16 * 1024).collect::<String>(),
        );
        self.open_mio_ai_with_context(
            format!("Collaborator {proto} 交互"),
            context,
            cx,
        );
    }

    pub(in crate::workbench) fn copy_collaborator_interaction_payload(
        &mut self,
        interaction_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) {
        let Some(interaction) = self.collaborator_interaction_by_id(interaction_id) else {
            return;
        };
        let Some(token) = interaction.token.as_deref().filter(|t| !t.is_empty()) else {
            return;
        };
        let payload = self
            .collaborator_pane
            .status
            .issued_tokens
            .iter()
            .find(|issued| issued.token == token)
            .map(|issued| match interaction.protocol {
                OastProtocol::Http => issued.http_callback_url.clone(),
                OastProtocol::Dns => issued.dns_qname.clone(),
            })
            .unwrap_or_else(|| token.to_owned());
        cx.write_to_clipboard(ClipboardItem::new_string(payload));
        self.collaborator_pane.error = None;
        self.collaborator_pane.notice = Some("已复制 Collaborator Payload 到剪贴板。".into());
        self.status = "已复制 Collaborator Payload 到剪贴板。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn copy_collaborator_interaction_curl(
        &mut self,
        interaction_id: Option<Uuid>,
        cx: &mut Context<Self>,
    ) {
        let Some(interaction) = self
            .collaborator_interaction_by_id(interaction_id)
            .filter(|item| item.protocol == OastProtocol::Http && !item.raw_request.is_empty())
        else {
            return;
        };
        let draft = self.collaborator_http_draft(&interaction);
        let redact = theme::redact_sensitive_headers_in_copy_curl()
            || self
                .user_preferences
                .misc
                .redact_sensitive_headers_in_copy_curl;
        match mio_core::curl_command_for_request_with_options(
            &draft.raw,
            &draft.target,
            draft.version,
            redact,
        ) {
            Ok(cmd) => {
                cx.write_to_clipboard(ClipboardItem::new_string(cmd));
                self.collaborator_pane.error = None;
                self.collaborator_pane.notice = Some("已复制 curl 命令到剪贴板。".into());
                self.status = "已复制 curl 命令到剪贴板。".into();
                cx.notify();
            }
            Err(err) => self.fail_collaborator(err.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn delete_collaborator_interaction(
        &mut self,
        interaction_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.collaborator_pane.status.interactions.len();
        self.collaborator_pane
            .status
            .interactions
            .retain(|item| item.id != interaction_id);
        if self.collaborator_pane.status.interactions.len() == before {
            return;
        }
        if self.collaborator_pane.selected_interaction == Some(interaction_id) {
            self.collaborator_pane.selected_interaction = None;
            self.collaborator_pane.raw_editor.load(b"", window, cx);
        }
        self.collaborator_pane.error = None;
        self.collaborator_pane.notice = Some("已删除该 Collaborator 交互记录。".into());
        cx.notify();
    }

    pub(in crate::workbench) fn save_collaborator_interaction_details(
        &mut self,
        interaction_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() || self.file_picker_pending() {
            return;
        }
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        let Some(interaction) = self.collaborator_interaction_by_id(interaction_id) else {
            return;
        };
        let json = match mio_core::render_collaborator_interactions_json(std::slice::from_ref(
            &interaction,
        )) {
            Ok(json) => json,
            Err(err) => {
                self.fail_collaborator(err.to_string(), cx);
                return;
            }
        };
        let Some(project) = self.project else {
            return;
        };
        let directory = self.default_directory();
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let short_id: String = interaction.id.simple().to_string().chars().take(8).collect();
        let suggested = format!("collaborator-interaction-{short_id}.json");
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &directory,
            Some(&suggested),
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                    return;
                }
                match result {
                    Ok(Ok(Some(path))) => match std::fs::write(&path, json.as_bytes()) {
                        Ok(()) => {
                            view.collaborator_pane.error = None;
                            let msg = format!("已保存 Collaborator 交互详情到 {}。", path.display());
                            view.collaborator_pane.notice = Some(msg.clone());
                            view.status = msg;
                        }
                        Err(err) => {
                            view.fail_collaborator(format!("保存交互详情失败：{err}"), cx);
                        }
                    },
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => {
                        view.fail_collaborator(format!("无法选择保存路径：{err}"), cx);
                    }
                    Err(_) => view.fail_collaborator("文件选择器已关闭。", cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn render_collaborator_raw_viewer(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_operate = self.collaborator_can_operate();
        let dark = theme::is_dark();
        let pane = &self.collaborator_pane;
        let pretty = pane.raw_editor.pretty_active;
        let view = pane.raw_editor.view;
        let selected = pane
            .selected_interaction
            .and_then(|id| pane.status.interactions.iter().find(|item| item.id == id))
            .cloned();
        let has_selected = selected.is_some();
        let is_http = selected
            .as_ref()
            .is_some_and(|item| item.protocol == OastProtocol::Http);
        let raw_empty = selected
            .as_ref()
            .is_none_or(|item| item.raw_request.is_empty());
        let file_picker_pending = self.file_picker_pending();
        let can_send_repeater = can_operate && is_http && !raw_empty;
        let meta = selected.as_ref().map_or_else(
            || "选择上方交互记录以查看原始请求报文。".to_owned(),
            |interaction| {
                let token = interaction.token.as_deref().unwrap_or("未匹配 Token");
                let label = pane
                    .token_label(interaction.token.as_deref().unwrap_or_default())
                    .unwrap_or_default();
                let token_part = if label.is_empty() {
                    token.to_owned()
                } else {
                    format!("{token} ({label})")
                };
                let summary = if interaction.protocol == OastProtocol::Dns {
                    format!(
                        "DNS UDP 原始报文 · {} · {} B · 详见 Raw / Hex",
                        interaction.summary,
                        interaction.raw_request.len()
                    )
                } else {
                    format!(
                        "{} · {} B",
                        interaction.summary,
                        interaction.raw_request.len()
                    )
                };
                format!("{token_part}  ·  {summary}  ·  来源 {}", interaction.peer_addr)
            },
        );
        let error = pane.error.clone().unwrap_or_default();
        div()
            .id("collaborator-raw-viewer")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .on_action(cx.listener(|this, action: &CollaboratorRawMenuAction, window, cx| {
                cx.stop_propagation();
                match action {
                    CollaboratorRawMenuAction::SendToRepeater => {
                        this.send_collaborator_interaction_to_repeater(None, window, cx)
                    }
                    CollaboratorRawMenuAction::SendToIntruder => {
                        this.send_collaborator_interaction_to_intruder(None, window, cx)
                    }
                    CollaboratorRawMenuAction::SendToComparer => {
                        this.send_collaborator_interaction_to_comparer(None, window, cx)
                    }
                    CollaboratorRawMenuAction::SendToDecoder => {
                        this.send_collaborator_interaction_to_decoder(None, window, cx)
                    }
                    CollaboratorRawMenuAction::CopyCurl => {
                        this.copy_collaborator_interaction_curl(None, cx)
                    }
                    CollaboratorRawMenuAction::Save => {
                        this.save_collaborator_interaction_details(None, window, cx)
                    }
                }
            }))
            .child(
                div()
                    .w_full()
                    .h(px(38.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(theme::surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .pl(px(10.))
                            .pr(px(12.))
                            .text_size(theme::scaled_px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if dark {
                                theme::foreground()
                            } else {
                                rgb(0x000000).into()
                            })
                            .child("Request"),
                    )
                    .child(
                        collaborator_view_tab(
                            "collaborator-raw-editor-pretty",
                            "Pretty",
                            64.,
                            pretty,
                        )
                        .tooltip("格式化预览交互请求报文")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_collaborator_raw_pretty(window, cx);
                        })),
                    )
                    .child(
                        collaborator_view_tab(
                            "collaborator-raw-editor-raw",
                            "Raw",
                            56.,
                            !pretty && view == MessageView::Raw,
                        )
                        .tooltip("查看交互请求原始文本")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_collaborator_raw_view(MessageView::Raw, window, cx);
                        })),
                    )
                    .child(
                        collaborator_view_tab(
                            "collaborator-raw-editor-hex",
                            "Hex",
                            53.,
                            !pretty && view == MessageView::Hex,
                        )
                        .tooltip("以十六进制视图查看交互原始字节")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_collaborator_raw_view(MessageView::Hex, window, cx);
                        })),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .pr(px(10.))
                            .flex()
                            .items_center()
                            .child(
                                button("collaborator-to-repeater", "发送到 Repeater")
                                    .icon(IconName::Repeat)
                                    .tooltip("将当前选中的 HTTP 回调原始请求发送到 Repeater 重放分析")
                                    .disabled(!can_send_repeater)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_collaborator_to_repeater(window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("collaborator-interaction-meta")
                    .test_support()
                    .w_full()
                    .flex_shrink_0()
                    .px(px(10.))
                    .py(px(4.))
                    .truncate()
                    .text_size(theme::scaled_px(11.5))
                    .bg(theme::toolbar_surface())
                    .text_color(if has_selected {
                        theme::foreground()
                    } else {
                        theme::muted_foreground()
                    })
                    .border_b_1()
                    .border_color(theme::border())
                    .child(meta),
            )
            .when(!error.is_empty(), |section| {
                section.child(
                    div()
                        .id("collaborator-error-banner")
                        .test_support()
                        .px(px(10.))
                        .py(px(5.))
                        .text_size(theme::scaled_px(11.5))
                        .bg(theme::danger_bg())
                        .border_b_1()
                        .border_color(theme::danger_border())
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            })
            .child(
                div()
                    .id("collaborator-raw-editor")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .child(
                        Editor::new(pane.raw_editor.state())
                            .readonly(true)
                            .size_full()
                            .bordered(false)
                            .context_menu(collaborator_raw_editor_context_menu(
                                pane.raw_editor.state().clone(),
                                can_operate,
                                has_selected,
                                is_http,
                                raw_empty,
                                file_picker_pending,
                            ))
                            .aria_label("Collaborator 原始请求"),
                    ),
            )
            .child(pane.raw_editor.render_search_bar(
                "collaborator-raw-editor",
                cx,
                |this, action, window, cx| {
                    this.handle_collaborator_search_action(action, window, cx);
                },
            ))
            .into_any_element()
    }

    pub(in crate::workbench) fn render_collaborator_config_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.collaborator_pane;
        div()
            .id("collaborator-config-panel")
            .test_support()
            .px(px(8.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("自托管 OAST 轮询配置")
                    .child(
                        button(
                            "collaborator-self-hosted-enabled",
                            if pane.self_hosted_enabled {
                                "关闭外部轮询"
                            } else {
                                "启用外部轮询"
                            },
                        )
                        .tooltip("启用或关闭自定义自托管 OAST 服务器轮询")
                        .selected(pane.self_hosted_enabled)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_collaborator_self_hosted(window, cx);
                        })),
                    )
                    .child(div().flex_1())
                    .child(
                        button("collaborator-apply-config", "应用并重启监听器")
                            .primary()
                            .icon(IconName::Check)
                            .tooltip("应用自托管配置并重启 OAST 监听器")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_collaborator_server(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(11.))
                    .text_color(theme::muted_foreground())
                    .child(format!("{LOCAL_LOOPBACK_NOTICE}。{SELF_HOSTED_SECURITY_NOTE}")),
            )
            .when(pane.self_hosted_enabled, |section| {
                section
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .min_w(theme::scaled_px(120.))
                                    .flex_shrink_0()
                                    .child("轮询 URL："),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&pane.polling_url_input)
                                        .id("collaborator-polling-url")
                                        .aria_label("自托管轮询 URL")
                                        .small(),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .min_w(theme::scaled_px(120.))
                                    .flex_shrink_0()
                                    .child("服务域名："),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&pane.server_domain_input)
                                        .id("collaborator-server-domain")
                                        .aria_label("自托管服务域名")
                                        .small(),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .min_w(theme::scaled_px(120.))
                                    .flex_shrink_0()
                                    .child("认证请求头："),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&pane.auth_header_input)
                                        .id("collaborator-auth-header")
                                        .aria_label("自托管认证请求头")
                                        .small(),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .min_w(theme::scaled_px(120.))
                                    .flex_shrink_0()
                                    .child("轮询间隔 (ms)："),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(160.).max(px(160.)))
                                    .flex_shrink_0()
                                    .child(
                                        Input::new(&pane.poll_interval_input)
                                            .id("collaborator-poll-interval")
                                            .aria_label("轮询间隔 (ms)")
                                            .small(),
                                    ),
                            ),
                    )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AnyWindowHandle, TestAppContext};
    use mio_core::{OastInteraction, OastPayloadToken};

    use super::*;
    use crate::workbench::tests::{drain, fixture};

    fn click(window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn http_interaction(_project: mio_core::ProjectKey, token: &str) -> OastInteraction {
        OastInteraction {
            id: Uuid::new_v4(),
            token: Some(token.to_owned()),
            protocol: OastProtocol::Http,
            peer_addr: "127.0.0.1:64001".into(),
            summary: format!("GET /oast/{token} HTTP/1.1"),
            raw_request: format!("GET /oast/{token} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .into_bytes(),
            observed_unix_ms: 1,
        }
    }

    fn dns_interaction(_project: mio_core::ProjectKey, token: &str) -> OastInteraction {
        OastInteraction {
            id: Uuid::new_v4(),
            token: Some(token.to_owned()),
            protocol: OastProtocol::Dns,
            peer_addr: "127.0.0.1:64002".into(),
            summary: format!("A {token}.oast.local"),
            raw_request: vec![
                0x9a, 0xbc, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ],
            observed_unix_ms: 2,
        }
    }

    #[gpui_kit::test]
    fn copy_http_generates_payloads_and_writes_system_clipboard(cx: &mut TestAppContext) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);
        click(f.window, "tool-collaborator", cx);
        let drained = drain(&mut f.commands);
        let [query] = drained.as_slice() else {
            panic!("expected QueryStatus on first activation");
        };
        let Command::Collaborator(CollaboratorCommand::QueryStatus { project }) = query else {
            panic!("expected Collaborator QueryStatus");
        };
        assert_eq!(*project, f.project);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("collaborator-onboarding").is_some());
            window.click("collaborator-get-started", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let dismissed_cmds = drain(&mut f.commands);
        assert!(matches!(
            dismissed_cmds.as_slice(),
            [Command::SetUserWorkbenchPreferences { misc, .. }]
                if misc.collaborator_onboarding_dismissed
        ));

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("collaborator-count", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("3", cx);
            window.click("collaborator-label", cx);
            window.press(crate::kbd!("a"), cx);
            window.input(" SSRF probe ", cx);
            window.render_frame(cx);
            window.click("collaborator-copy-http", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let commands = drain(&mut f.commands);
        let [
            Command::Collaborator(CollaboratorCommand::GeneratePayloads {
                project,
                count,
                label,
            }),
        ] = commands.as_slice()
        else {
            panic!("expected GeneratePayloads, got {} commands", commands.len());
        };
        assert_eq!(*project, f.project);
        assert_eq!((*count, label.as_str()), (3, "SSRF probe"));

        let payloads: Vec<OastPayloadToken> = (0..3)
            .map(|index| OastPayloadToken {
                token: format!("{index:016x}"),
                label: "SSRF probe".into(),
                http_callback_url: format!("http://127.0.0.1:54321/oast/{index:016x}"),
                dns_qname: format!("{index:016x}.oast.local"),
                created_unix_ms: 3,
            })
            .collect();
        let expected_http: Vec<String> = payloads
            .iter()
            .map(|payload| payload.http_callback_url.clone())
            .collect();
        f.events
            .try_send(Event::Collaborator(CollaboratorEvent::PayloadsGenerated {
                project: f.project,
                payloads: payloads.clone(),
            }))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("collaborator-payload-preview").label().unwrap(),
                expected_http.join("\n")
            );
        })
        .unwrap();
        cx.read(|cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(expected_http.join("\n"))
            );
            let pane = &f.view.read(cx).collaborator_pane;
            assert_eq!(
                pane.latest_payload_preview.as_deref(),
                Some(expected_http.join("\n").as_str())
            );
            assert_eq!(pane.status.issued_tokens.len(), 3);
            assert!(pane.notice.as_deref().unwrap().contains("HTTP 回调 URL"));
        });
    }

    #[gpui_kit::test]
    fn interactions_filter_by_protocol_and_load_raw_packets(cx: &mut TestAppContext) {
        let mut f = fixture(cx, Vec::new(), 64);
        click(f.window, "tool-collaborator", cx);
        let _ = drain(&mut f.commands);
        let http = http_interaction(f.project, "1111111111111111");
        let dns = dns_interaction(f.project, "2222222222222222");
        let dns_id = dns.id;
        f.events
            .try_send(Event::Collaborator(
                CollaboratorEvent::InteractionObserved {
                    project: f.project,
                    interaction: Box::new(http.clone()),
                },
            ))
            .unwrap();
        f.events
            .try_send(Event::Collaborator(
                CollaboratorEvent::InteractionObserved {
                    project: f.project,
                    interaction: Box::new(dns.clone()),
                },
            ))
            .unwrap();
        cx.run_until_parked();

        click(f.window, "collaborator-filter-dns", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find(SharedString::from(format!(
                        "collaborator-interaction-{}",
                        http.id
                    )))
                    .is_none()
            );
            assert!(
                window
                    .try_find(SharedString::from(format!(
                        "collaborator-interaction-{dns_id}"
                    )))
                    .is_some()
            );
            window.click(
                SharedString::from(format!("collaborator-interaction-{dns_id}")),
                cx,
            );
        })
        .unwrap();
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let (selected, view) = {
                let pane = &f.view.read(cx).collaborator_pane;
                (pane.selected_interaction, pane.raw_editor.view)
            };
            assert_eq!(selected, Some(dns_id));
            assert_eq!(view, MessageView::Hex);
            let bytes = f.view.update(cx, |v, cx| {
                v.collaborator_pane.raw_editor.bytes(window, cx).unwrap()
            });
            assert_eq!(bytes, dns.raw_request);
        })
        .unwrap();

        click(f.window, "collaborator-filter-all", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(
                SharedString::from(format!("collaborator-interaction-{}", http.id)),
                cx,
            );
            window.render_frame(cx);
            let (selected, view, raw) = {
                let pane = &f.view.read(cx).collaborator_pane;
                (
                    pane.selected_interaction,
                    pane.raw_editor.view,
                    pane.raw_editor.state().read(cx).value().to_string(),
                )
            };
            assert_eq!(selected, Some(http.id));
            assert_eq!(view, MessageView::Raw);
            let bytes = f.view.update(cx, |v, cx| {
                v.collaborator_pane.raw_editor.bytes(window, cx).unwrap()
            });
            assert_eq!(bytes, http.raw_request);
            assert!(raw.contains("GET /oast/1111111111111111"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn config_submits_poll_interval_and_copies_interactions_json(cx: &mut TestAppContext) {
        let mut f = fixture(cx, Vec::new(), 64);
        click(f.window, "tool-collaborator", cx);
        let _ = drain(&mut f.commands);

        let http = http_interaction(f.project, "abcd1234ef567890");
        f.events
            .try_send(Event::Collaborator(
                CollaboratorEvent::InteractionObserved {
                    project: f.project,
                    interaction: Box::new(http),
                },
            ))
            .unwrap();
        cx.run_until_parked();

        click(f.window, "collaborator-copy-json", cx);
        cx.read(|cx| {
            let clip = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .expect("JSON copied");
            assert!(clip.contains("abcd1234ef567890"));
        });

        click(f.window, "collaborator-toggle-config", cx);
        click(f.window, "collaborator-self-hosted-enabled", cx);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("collaborator-polling-url", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("http://127.0.0.1:19999/poll", cx);
            window.click("collaborator-poll-interval", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("750", cx);
            window.render_frame(cx);
            window.click("collaborator-apply-config", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let cmds = drain(&mut f.commands);
        assert!(matches!(
            cmds.as_slice(),
            [Command::Collaborator(CollaboratorCommand::Start { config, .. })]
                if config.enabled && config.poll_interval_ms == 750
        ));
    }

    #[gpui_kit::test]
    fn onboarding_renders_ext_welcome_without_inspector_and_dismisses_to_workspace(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);
        click(f.window, "tool-collaborator", cx);
        let _ = drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("collaborator-onboarding").is_some());
            assert!(window.try_find("collaborator-onboarding-radar").is_some());
            assert!(window.try_find("collaborator-onboarding-title").is_some());
            assert!(window.try_find("collaborator-learn-more").is_some());
            assert!(window.try_find("collaborator-get-started").is_some());
            assert!(window.try_find("collaborator-dont-show-again").is_some());
            assert!(window.try_find("collaborator-dismiss-onboarding").is_some());
            assert!(window.try_find("inspector-toggle").is_none());
            window.click("collaborator-dismiss-onboarding", cx);
        })
        .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("collaborator-onboarding").is_none());
            assert!(window.try_find("collaborator-table-header").is_some());
            assert!(window.try_find("collaborator-copy-http").is_some());
            assert!(window.try_find("collaborator-raw-editor-pretty").is_some());
            assert!(window.try_find("inspector-toggle").is_none());
        })
        .unwrap();
    }
}
