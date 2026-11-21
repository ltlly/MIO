use super::*;

pub(in crate::workbench::tabs) fn sync_captured_draft_version(
    old_raw: &[u8],
    new_raw: &[u8],
    current_version: HttpVersion,
) -> HttpVersion {
    match mio_core::detect_request_line_version(new_raw) {
        Some(next) if mio_core::detect_request_line_version(old_raw) != Some(next) => next,
        _ => current_version,
    }
}

impl Workbench {
    pub(in crate::workbench) fn toggle_repeater_protocol(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none()
            || !self.backend_available()
            || self.transition.is_some()
            || self.sending.contains_key(&id)
            || self.pending_sends.contains_key(&id)
            || self.plugin_waiting(id)
            || self.extensions.active_messages.contains_key(&id)
        {
            return;
        }
        let Some(current_version) = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.draft.version)
        else {
            return;
        };
        let next_version = match current_version {
            HttpVersion::Http2 => HttpVersion::Http11,
            HttpVersion::Http10 | HttpVersion::Http11 => HttpVersion::Http2,
        };
        if let Err(error) = self.capture_drafts(window, cx) {
            self.fail(error, cx);
            return;
        }
        let Some(tab) = self.workspace.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        let Some(next_revision) = tab.draft.revision.checked_add(1) else {
            self.fail("草稿 revision 已耗尽；无法切换 HTTP 协议版本。", cx);
            return;
        };
        let next_raw = mio_core::rewrite_request_line_version(&tab.draft.raw, next_version);
        tab.draft.version = next_version;
        tab.draft.raw = next_raw.clone();
        tab.draft.revision = next_revision;
        if let Some(editors) = self.tab_editors.get_mut(&id) {
            let was_pretty = editors.request.pretty_active();
            let prev_view = editors.request.view();
            editors.request.load(&next_raw, window, cx);
            if was_pretty {
                let _ = editors.request.switch_pretty(window, cx);
            } else {
                let _ = editors.request.switch(prev_view, window, cx);
            }
            editors.loaded_revision = next_revision;
            editors.dirty = false;
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn begin_rename(
        &mut self,
        target: RenameTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let title = match target {
            RenameTarget::Tab(id) => self
                .workspace
                .tabs
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.title.clone()),
            RenameTarget::WebSocketTab(id) => self
                .workspace
                .websocket_tabs
                .iter()
                .find(|tab| !tab.closed && tab.id == id)
                .map(|tab| tab.title.clone()),
        };
        if let Some(title) = title {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
            input.update(cx, |state, cx| {
                state.select_all(window, cx);
                state.focus(window, cx);
            });
            self.rename = Some(Rename { target, input });
        }
        cx.notify();
    }

    pub(in crate::workbench) fn apply_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let Some(rename) = &self.rename else { return };
        let title = match editing::input_snapshot(&rename.input, window, cx) {
            Ok(title) => title.trim().to_owned(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if title.is_empty() || title.len() > 512 {
            self.fail("名称须为 1–512 字节的非空文本。", cx);
            return;
        }
        match rename.target {
            RenameTarget::Tab(id) => {
                if let Some(tab) = self.workspace.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.title = title;
                }
            }
            RenameTarget::WebSocketTab(id) => {
                if let Some(tab) = self
                    .workspace
                    .websocket_tabs
                    .iter_mut()
                    .find(|tab| !tab.closed && tab.id == id)
                {
                    tab.title = title;
                }
            }
        }
        self.rename = None;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_orientation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.flush_workspace(window, cx) {
            return;
        }
        let entering_vertical = self.workspace.orientation == Orientation::Horizontal;
        self.workspace.orientation = match self.workspace.orientation {
            Orientation::Horizontal => Orientation::Vertical,
            Orientation::Vertical => Orientation::Horizontal,
        };
        if entering_vertical && (self.tab_width - 180.).abs() < f32::EPSILON {
            self.tab_width = 220.;
        }
        self.tabs_scroll.set_offset(gpui_kit::Point::default());
        self.save_workspace(cx);
        cx.notify();
    }
}
