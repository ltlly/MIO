use super::*;

const MAX_REPEATER_PATH_CHARS: usize = 28;
const MAX_WORKSPACE_TABS: usize = 512;

fn extract_concise_urlpath(url_or_target: &str) -> Option<String> {
    let trimmed = url_or_target.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path_part = if let Some((_, tail)) = trimmed.split_once("://") {
        match tail.find(['/', '?', '#']) {
            Some(idx) if tail.as_bytes()[idx] == b'/' => &tail[idx..],
            _ => "/",
        }
    } else if trimmed.starts_with('/') {
        trimmed
    } else {
        return None;
    };
    let without_query = path_part.split(['?', '#']).next().unwrap_or("/").trim();
    let normalized = if without_query.is_empty() {
        "/"
    } else {
        without_query
    };
    Some(truncate_path(normalized))
}

fn extract_path_from_raw(raw: &[u8]) -> Option<String> {
    let line_end = raw
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(raw.len());
    let line = std::str::from_utf8(&raw[..line_end]).ok()?.trim();
    let mut parts = line.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    if target == "*" {
        return Some("/".into());
    }
    extract_concise_urlpath(target).or_else(|| {
        let without_query = target.split(['?', '#']).next().unwrap_or("/").trim();
        if without_query.is_empty() {
            Some("/".into())
        } else if without_query.starts_with('/') {
            Some(truncate_path(without_query))
        } else {
            Some(truncate_path(&format!("/{without_query}")))
        }
    })
}

fn truncate_path(path: &str) -> String {
    if path.chars().count() <= MAX_REPEATER_PATH_CHARS {
        return path.to_owned();
    }
    let head: String = path.chars().take(MAX_REPEATER_PATH_CHARS - 1).collect();
    format!("{head}…")
}

fn tab_number_used(tabs: &[WorkTab], closed: &[WorkTab], n: usize) -> bool {
    let exact = n.to_string();
    let prefix = format!("{n}: ");
    tabs.iter()
        .chain(closed)
        .any(|tab| tab.title == exact || tab.title.starts_with(&prefix))
}

fn next_unused_tab_number(tabs: &[WorkTab], closed: &[WorkTab]) -> usize {
    let mut n = 1usize;
    while tab_number_used(tabs, closed, n) {
        n = n.saturating_add(1);
    }
    n
}

fn compute_repeater_tab_title(
    tabs: &[WorkTab],
    closed: &[WorkTab],
    request: Option<&mio_core::HttpRequest>,
    raw_override: Option<&[u8]>,
    target_fallback: Option<&str>,
) -> String {
    let n = next_unused_tab_number(tabs, closed);
    let path = request
        .and_then(|req| extract_concise_urlpath(&req.url))
        .or_else(|| raw_override.and_then(extract_path_from_raw))
        .or_else(|| target_fallback.and_then(extract_concise_urlpath))
        .unwrap_or_else(|| "/".into());
    format!("{n}: {path}")
}

pub(in crate::workbench) fn extract_repeater_urlpath(
    raw: Option<&[u8]>,
    target: Option<&str>,
) -> String {
    raw.and_then(extract_path_from_raw)
        .or_else(|| target.and_then(extract_concise_urlpath))
        .unwrap_or_else(|| "/".into())
}

fn workspace_total_tabs(ws: &Workspace) -> usize {
    ws.tabs.len()
        + ws.closed.len()
        + ws.intruder.tabs.len()
        + ws.intruder.closed.len()
        + ws.websocket_tabs.len()
}

impl Workbench {
    pub(in crate::workbench) fn next_repeater_tab_title(
        &self,
        request: Option<&mio_core::HttpRequest>,
        raw_override: Option<&[u8]>,
    ) -> String {
        compute_repeater_tab_title(
            &self.workspace.tabs,
            &self.workspace.closed,
            request,
            raw_override,
            None,
        )
    }

    pub(in crate::workbench) fn duplicate_repeater_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none()
            || !self.backend_available()
            || self.transition.is_some()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        if workspace_total_tabs(&self.workspace) >= MAX_WORKSPACE_TABS {
            self.fail("工作标签超过 512 个限额；未复制标签页。", cx);
            return;
        }
        let Some(index) = self.workspace.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let source = self.workspace.tabs[index].clone();
        let title = compute_repeater_tab_title(
            &self.workspace.tabs,
            &self.workspace.closed,
            None,
            Some(&source.draft.raw),
            Some(&source.draft.target),
        );
        let new_id = Uuid::new_v4();
        let duplicated = WorkTab {
            id: new_id,
            title,
            group: source.group,
            draft: Draft {
                revision: 1,
                target: source.draft.target,
                raw: source.draft.raw,
                version: source.draft.version,
            },
            last_flow: None,
            archive_response: None,
            history: None,
        };
        let prev_active = self.workspace.active;
        let prev_ws = self.workspace.active_websocket_tab;
        self.workspace.tabs.insert(index + 1, duplicated);
        self.workspace.active = Some(new_id);
        self.workspace.active_websocket_tab = None;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.tabs.remove(index + 1);
            self.workspace.active = prev_active;
            self.workspace.active_websocket_tab = prev_ws;
            self.fail(format!("未复制 Repeater 标签：{error}"), cx);
            return;
        }
        if let Some(group_id) = source.group
            && let Some(group) = self
                .workspace
                .groups
                .iter_mut()
                .find(|group| group.id == group_id)
        {
            group.collapsed = false;
        }
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        self.tool = Tool::Repeater;
        if let Some(editors) = self.tab_editors.get(&new_id) {
            editors
                .request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn duplicate_repeater_group(
        &mut self,
        group_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none()
            || !self.backend_available()
            || self.transition.is_some()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let Some(group_pos) = self
            .workspace
            .groups
            .iter()
            .position(|group| group.id == group_id)
        else {
            return;
        };
        let source_group = self.workspace.groups[group_pos].clone();
        let member_indices: Vec<usize> = self
            .workspace
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(idx, tab)| (tab.group == Some(group_id)).then_some(idx))
            .collect();
        if workspace_total_tabs(&self.workspace) + member_indices.len() > MAX_WORKSPACE_TABS {
            self.fail("工作标签超过 512 个限额；未复制分组。", cx);
            return;
        }
        let mut next_title = format!("{} 副本", source_group.title);
        let mut copy_num = 2usize;
        while self
            .workspace
            .groups
            .iter()
            .any(|existing| existing.title == next_title)
        {
            next_title = format!("{} 副本 {}", source_group.title, copy_num);
            copy_num = copy_num.saturating_add(1);
        }
        let new_group_id = Uuid::new_v4();
        let new_group = TabGroup {
            id: new_group_id,
            title: next_title,
            collapsed: false,
            color: source_group.color,
        };
        let mut candidate = self.workspace.clone();
        candidate.groups.insert(group_pos + 1, new_group);
        let insert_at = member_indices
            .last()
            .map_or(candidate.tabs.len(), |idx| idx + 1);
        let mut first_new_tab = None;
        let members: Vec<WorkTab> = member_indices
            .iter()
            .map(|&idx| self.workspace.tabs[idx].clone())
            .collect();
        for (offset, member) in members.into_iter().enumerate() {
            let title = compute_repeater_tab_title(
                &candidate.tabs,
                &candidate.closed,
                None,
                Some(&member.draft.raw),
                Some(&member.draft.target),
            );
            let new_tab_id = Uuid::new_v4();
            if first_new_tab.is_none() {
                first_new_tab = Some(new_tab_id);
            }
            candidate.tabs.insert(
                insert_at + offset,
                WorkTab {
                    id: new_tab_id,
                    title,
                    group: Some(new_group_id),
                    draft: Draft {
                        revision: 1,
                        target: member.draft.target,
                        raw: member.draft.raw,
                        version: member.draft.version,
                    },
                    last_flow: None,
                    archive_response: None,
                    history: None,
                },
            );
        }
        if let Some(active_id) = first_new_tab {
            candidate.active = Some(active_id);
            candidate.active_websocket_tab = None;
        }
        if let Err(error) = candidate.validate_structure() {
            self.fail(format!("未复制标签分组：{error}"), cx);
            return;
        }
        self.workspace = candidate;
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        self.tool = Tool::Repeater;
        if let Some(active_id) = first_new_tab
            && let Some(editors) = self.tab_editors.get(&active_id)
        {
            editors
                .request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn duplicate_ws_repeater_tab(
        &mut self,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || !self.backend_available() || self.transition.is_some() {
            return;
        }
        if self.workspace.websocket_tabs.len() >= mio_core::MAX_WEBSOCKET_REPEATER_TABS
            || workspace_total_tabs(&self.workspace) >= MAX_WORKSPACE_TABS
        {
            self.fail("WebSocket Repeater 标签已达上限；未复制标签页。", cx);
            return;
        }
        let Some(index) = self
            .workspace
            .websocket_tabs
            .iter()
            .position(|tab| !tab.closed && tab.id == id)
        else {
            return;
        };
        let source = self.workspace.websocket_tabs[index].clone();
        let mut next_num = 1usize;
        let title = loop {
            let candidate = format!("WS {next_num}");
            if !self
                .workspace
                .websocket_tabs
                .iter()
                .any(|tab| tab.title == candidate || tab.title == next_num.to_string())
            {
                break candidate;
            }
            next_num = next_num.saturating_add(1);
        };
        let new_id = Uuid::new_v4();
        let duplicated = mio_core::WebSocketRepeaterTab {
            id: new_id,
            title,
            group: source.group,
            url: source.url,
            handshake_headers: source.handshake_headers,
            source_flow_id: source.source_flow_id,
            source_message_id: source.source_message_id,
            direction: source.direction,
            draft_revision: 1,
            draft_payload: source.draft_payload,
            draft_kind: source.draft_kind,
            history: Vec::new(),
            closed: false,
        };
        self.workspace.websocket_tabs.insert(index + 1, duplicated);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.websocket_tabs.remove(index + 1);
            self.fail(format!("未复制 WebSocket Repeater 标签：{error}"), cx);
            return;
        }
        let prev_active = self.workspace.active;
        let prev_ws = self.workspace.active_websocket_tab;
        self.workspace.active = None;
        self.workspace.active_websocket_tab = Some(new_id);
        let entity = cx.entity();
        let owner_window = self.owner_window;
        cx.defer(move |cx| {
            let res = owner_window.update(cx, |_, window, cx| {
                entity.update(cx, |this, cx| {
                    if let Err(error) = this.capture_drafts(window, cx) {
                        this.workspace.websocket_tabs.retain(|tab| tab.id != new_id);
                        this.workspace.active = prev_active;
                        this.workspace.active_websocket_tab = prev_ws;
                        this.fail(error, cx);
                        return;
                    }
                    this.sync_websocket_repeater_editors(window, cx);
                    this.cancel_plugin_interactions(cx);
                    this.save_workspace(cx);
                    this.tool = Tool::Repeater;
                    if let Some(editors) = this.websocket_repeater.editors.get(&new_id) {
                        editors
                            .draft
                            .state()
                            .update(cx, |state, cx| state.focus(window, cx));
                    }
                    cx.notify();
                });
            });
            if res.is_err() {
                entity.update(cx, |this, cx| {
                    this.cancel_plugin_interactions(cx);
                    this.save_workspace(cx);
                    this.tool = Tool::Repeater;
                    cx.notify();
                });
            }
        });
        cx.notify();
    }
}
