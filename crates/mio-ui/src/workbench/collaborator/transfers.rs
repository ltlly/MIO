use super::*;

const COLLABORATOR_MAX_DECODER_BYTES: usize = 512 * 1024;
const COLLABORATOR_MAX_COMPARER_ITEMS: usize = 32;
const COLLABORATOR_MAX_COMPARER_ITEM_BYTES: usize = 256 * 1024;

impl Workbench {
    fn collaborator_interaction_by_id(
        &self,
        interaction_id: Option<Uuid>,
    ) -> Option<OastInteraction> {
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
        self.open_mio_ai_with_context(format!("Collaborator {proto} 交互"), context, cx);
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
        let short_id: String = interaction
            .id
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect();
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
                    Ok(Ok(Some(path))) => {
                        match mio_core::ui_files::write_export_file(&path, json.as_bytes()) {
                            Ok(()) => {
                                view.collaborator_pane.error = None;
                                let msg = format!(
                                    "已保存 Collaborator 交互详情到 {}。",
                                    path.display()
                                );
                                view.collaborator_pane.notice = Some(msg.clone());
                                view.status = msg;
                            }
                            Err(err) => {
                                view.fail_collaborator(format!("保存交互详情失败：{err}"), cx);
                            }
                        }
                    }
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
}
