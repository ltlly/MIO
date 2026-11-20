use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_websocket_repeater_event(
        &mut self,
        event: WebSocketRepeaterEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            WebSocketRepeaterEvent::Opened {
                project,
                result,
                mode,
                ..
            } if self.project == Some(project) => match result {
                Ok(mut tab) => {
                    if matches!(mode, WebSocketRepeaterConnectionMode::Standalone { .. }) {
                        tab.direction = WebSocketDirection::ClientToServer;
                    }
                    let composition_error = self.capture_drafts(window, cx).err();
                    let tab_id = tab.id;
                    if let Some(existing) = self
                        .workspace
                        .websocket_tabs
                        .iter_mut()
                        .find(|existing| existing.id == tab_id)
                    {
                        *existing = tab;
                    } else {
                        self.workspace.websocket_tabs.push(tab);
                    }
                    if composition_error.is_none() {
                        self.workspace.active = None;
                        self.workspace.active_websocket_tab = Some(tab_id);
                        self.tool = Tool::Repeater;
                    }
                    self.sync_websocket_repeater_editors(window, cx);
                    if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
                        editors.mode = mode;
                    }
                    self.save_workspace(cx);
                    if let Some(error) = composition_error {
                        self.fail(error, cx);
                    }
                }
                Err(error) => self.fail(error, cx),
            },
            WebSocketRepeaterEvent::Connected {
                project,
                tab_id,
                mode,
                result,
                ..
            } if self.project == Some(project) => {
                if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
                    editors.connecting = None;
                    editors.disconnecting = false;
                    match result {
                        Ok(()) => {
                            editors.mode = mode;
                            editors.result = Some(mode_label(mode).into());
                            if matches!(mode, WebSocketRepeaterConnectionMode::Standalone { .. })
                                && let Some(tab) = self
                                    .workspace
                                    .websocket_tabs
                                    .iter_mut()
                                    .find(|tab| tab.id == tab_id)
                                && tab.direction != WebSocketDirection::ClientToServer
                            {
                                tab.direction = WebSocketDirection::ClientToServer;
                                self.save_workspace(cx);
                            }
                        }
                        Err(error) => {
                            editors.mode = mode;
                            editors.result = Some(format!("连接失败：{error}"));
                            self.fail(error, cx);
                        }
                    }
                }
            }
            WebSocketRepeaterEvent::Sent {
                project,
                tab_id,
                draft_revision,
                mode,
                result,
                ..
            } if self.project == Some(project) => {
                let matches_sending = self
                    .websocket_repeater
                    .editors
                    .get(&tab_id)
                    .and_then(|editors| editors.sending.as_ref())
                    .is_none_or(|(expected, _)| *expected <= draft_revision);
                if !matches_sending {
                    return;
                }
                match result {
                    Ok(item) => {
                        let item_id = item.id;
                        let item_bytes = item.payload.len();
                        let payload = item.payload.clone();
                        let had_no_selection = self
                            .websocket_repeater
                            .editors
                            .get(&tab_id)
                            .is_some_and(|editors| editors.selected_history.is_none());
                        if let Some(tab) = self
                            .workspace
                            .websocket_tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                        {
                            tab.push_history(item);
                        }
                        if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
                            editors.sending = None;
                            editors.mode = mode;
                            if had_no_selection || editors.auto_follow_latest {
                                editors.selected_history = Some(item_id);
                                editors.history.load(&payload, window, cx);
                            }
                            editors.result = Some(format!("已发送 · {item_bytes} 字节"));
                        }
                        self.save_workspace(cx);
                    }
                    Err(error) => {
                        if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
                            editors.sending = None;
                            editors.mode = mode;
                            editors.result = Some(format!("发送失败：{error}"));
                        }
                        self.fail(error, cx);
                    }
                }
            }
            WebSocketRepeaterEvent::Received {
                project,
                tab_id,
                item,
                ..
            } if self.project == Some(project) => {
                let item_id = item.id;
                let item_bytes = item.payload.len();
                let payload = item.payload.clone();
                let had_no_selection = self
                    .websocket_repeater
                    .editors
                    .get(&tab_id)
                    .is_some_and(|editors| editors.selected_history.is_none());
                if let Some(tab) = self
                    .workspace
                    .websocket_tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                {
                    tab.push_history(item);
                }
                if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
                    if had_no_selection
                        || (editors.auto_follow_latest && !editors.history.has_focus(window, cx))
                    {
                        editors.selected_history = Some(item_id);
                        editors.history.load(&payload, window, cx);
                    }
                    editors.result = Some(format!("收到服务端消息 · {item_bytes} 字节"));
                }
                self.save_workspace(cx);
            }
            WebSocketRepeaterEvent::SessionClosed {
                project,
                tab_id,
                mode,
                reason,
            } if self.project == Some(project) => {
                if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
                    editors.connecting = None;
                    editors.sending = None;
                    editors.disconnecting = false;
                    editors.mode = mode;
                    editors.result = Some(match reason {
                        Some(reason) if !reason.is_empty() => format!("连接已关闭：{reason}"),
                        _ => "连接已关闭".into(),
                    });
                }
            }
            WebSocketRepeaterEvent::Disconnected {
                project,
                tab_id,
                result,
                ..
            } if self.project == Some(project) => {
                if let Some(editors) = self.websocket_repeater.editors.get_mut(&tab_id) {
                    editors.connecting = None;
                    editors.disconnecting = false;
                    match result {
                        Ok(()) => {
                            editors.mode = WebSocketRepeaterConnectionMode::Disconnected;
                            editors.result = Some("已断开连接".into());
                        }
                        Err(error) => {
                            editors.result = Some(format!("断开失败：{error}"));
                            self.fail(error, cx);
                        }
                    }
                }
            }
            WebSocketRepeaterEvent::Closed { project, tab_id } if self.project == Some(project) => {
                if let Some(mut editors) = self.websocket_repeater.editors.remove(&tab_id) {
                    if let Some(cancel) = editors.connecting.take() {
                        cancel.cancel();
                    }
                    if let Some((_, cancel)) = editors.sending.take() {
                        cancel.cancel();
                    }
                }
            }
            _ => {}
        }
    }
}
