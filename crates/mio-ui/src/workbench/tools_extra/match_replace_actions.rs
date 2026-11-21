use mio_core::{
    MatchAndReplaceRule, MatchReplaceTarget, Uuid, WsMatchAndReplaceRule, WsReplaceDirection,
};

use super::*;

impl Workbench {
    pub(super) fn add_match_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let snapshots = (|| {
            Ok::<_, &'static str>((
                editing::input_snapshot(&self.extra_tools.mr_match, window, cx)?.to_string(),
                editing::input_snapshot(&self.extra_tools.mr_replace, window, cx)?.to_string(),
                editing::input_snapshot(&self.extra_tools.mr_comment, window, cx)?.to_string(),
            ))
        })();
        let (match_pattern, replace_with, comment) = match snapshots {
            Ok(values) => values,
            Err(error) => {
                self.extra_tools.mr_error = Some(error.into());
                self.fail(error, cx);
                return;
            }
        };
        let is_header = matches!(
            self.extra_tools.mr_target,
            MatchReplaceTarget::RequestHeader | MatchReplaceTarget::ResponseHeader
        );
        if match_pattern.is_empty() && !is_header {
            self.extra_tools.mr_error = Some("请填写 HTTP 匹配模式（不能为空）。".into());
            cx.notify();
            return;
        }
        let prev_rules = self.workspace.match_and_replace_rules.clone();
        if let Some(edit_id) = self.extra_tools.mr_editing
            && let Some(existing) = self
                .workspace
                .match_and_replace_rules
                .iter_mut()
                .find(|r| r.id == edit_id)
        {
            existing.item_type = self.extra_tools.mr_target;
            existing.match_pattern = match_pattern;
            existing.replace_with = replace_with;
            existing.regex = self.extra_tools.mr_regex;
            existing.comment = comment;
            existing.script_name.clear();
            existing.filter_script.clear();
        } else {
            self.workspace
                .match_and_replace_rules
                .push(MatchAndReplaceRule {
                    id: Uuid::new_v4(),
                    enabled: true,
                    item_type: self.extra_tools.mr_target,
                    match_pattern,
                    replace_with,
                    regex: self.extra_tools.mr_regex,
                    comment,
                    script_name: String::new(),
                    filter_script: String::new(),
                });
        }
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.match_and_replace_rules = prev_rules;
            let message = format!("未保存规则：{error}");
            self.extra_tools.mr_error = Some(message.clone());
            self.fail(message, cx);
            return;
        }
        if let Some(dlg) = self.extra_tools.mr_dialog.take() {
            let _ = dlg.update(cx, |_, window, _| window.remove_window());
        }
        self.extra_tools.mr_editing = None;
        self.extra_tools.mr_editor_open = false;
        self.extra_tools.mr_error = None;
        self.extra_tools.mr_regex = false;
        self.extra_tools.mr_target = MatchReplaceTarget::RequestHeader;
        self.extra_tools.mr_target_select.update(cx, |state, cx| {
            state.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
        });
        for input in [
            &self.extra_tools.mr_match,
            &self.extra_tools.mr_replace,
            &self.extra_tools.mr_comment,
        ] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.sync_mr_table_selection(cx);
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn sync_mr_table_selection(&mut self, cx: &mut Context<Self>) {
        let sel = self.extra_tools.mr_selected.and_then(|id| {
            self.workspace
                .match_and_replace_rules
                .iter()
                .position(|r| r.id == id)
        });
        if sel.is_none() {
            self.extra_tools.mr_selected = None;
        }
        self.extra_tools.mr_table.update(cx, |table, cx| {
            if let Some(row) = sel {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
            cx.notify();
        });
    }

    pub(in crate::workbench) fn load_http_match_replace_for_edit(
        &mut self,
        rule: &MatchAndReplaceRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let any_composing = editing::input_snapshot(&self.extra_tools.mr_match, window, cx).is_err()
            || editing::input_snapshot(&self.extra_tools.mr_replace, window, cx).is_err()
            || editing::input_snapshot(&self.extra_tools.mr_comment, window, cx).is_err();
        if any_composing {
            self.extra_tools.mr_error = Some(editing::COMPOSITION_PENDING.into());
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        self.extra_tools.mr_editing = Some(rule.id);
        self.extra_tools.mr_editor_open = true;
        self.extra_tools.mr_error = None;
        self.extra_tools.mr_target = rule.item_type;
        self.extra_tools.mr_target_select.update(cx, |state, cx| {
            state.set_selected_index(
                Some(gpui_kit::component::IndexPath::new(mr_target_index(rule.item_type))),
                window,
                cx,
            );
        });
        self.extra_tools.mr_regex = rule.regex;
        self.extra_tools
            .mr_match
            .update(cx, |s, cx| s.set_value(rule.match_pattern.clone(), window, cx));
        self.extra_tools
            .mr_replace
            .update(cx, |s, cx| s.set_value(rule.replace_with.clone(), window, cx));
        self.extra_tools
            .mr_comment
            .update(cx, |s, cx| s.set_value(rule.comment.clone(), window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn move_http_match_replace_rule(
        &mut self,
        id: Uuid,
        down: bool,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(pos) = self
            .workspace
            .match_and_replace_rules
            .iter()
            .position(|r| r.id == id)
        else {
            return;
        };
        let target_pos = if down {
            pos.checked_add(1)
                .filter(|&p| p < self.workspace.match_and_replace_rules.len())
        } else {
            pos.checked_sub(1)
        };
        if let Some(target_pos) = target_pos {
            self.workspace
                .match_and_replace_rules
                .swap(pos, target_pos);
            self.sync_mr_table_selection(cx);
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn toggle_http_match_replace_rule(
        &mut self,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        if let Some(rule) = self
            .workspace
            .match_and_replace_rules
            .iter_mut()
            .find(|r| r.id == id)
        {
            rule.enabled = !rule.enabled;
            self.sync_mr_table_selection(cx);
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn remove_http_match_replace_rule(
        &mut self,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self.extra_tools.mr_editing == Some(id) {
            self.extra_tools.mr_editing = None;
            self.extra_tools.mr_editor_open = false;
            self.extra_tools.mr_error = None;
        }
        self.workspace
            .match_and_replace_rules
            .retain(|rule| rule.id != id);
        self.sync_mr_table_selection(cx);
        self.save_workspace(cx);
        cx.notify();
    }

    pub(super) fn set_all_http_match_replace_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let mut changed = false;
        for rule in &mut self.workspace.match_and_replace_rules {
            if rule.enabled != enabled {
                rule.enabled = enabled;
                changed = true;
            }
        }
        if changed {
            self.sync_mr_table_selection(cx);
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(super) fn clear_http_match_replace_rules(&mut self, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        if !self.workspace.match_and_replace_rules.is_empty() {
            self.workspace.match_and_replace_rules.clear();
            if let Some(dlg) = self.extra_tools.mr_dialog.take() {
                let _ = dlg.update(cx, |_, window, _| window.remove_window());
            }
            self.extra_tools.mr_editing = None;
            self.extra_tools.mr_selected = None;
            self.extra_tools.mr_editor_open = false;
            self.extra_tools.mr_error = None;
            self.sync_mr_table_selection(cx);
            self.save_workspace(cx);
            cx.notify();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn upsert_http_match_replace_rule(
        &mut self,
        project: Option<ProjectKey>,
        editing_id: Option<Uuid>,
        item_type: MatchReplaceTarget,
        match_pattern: String,
        replace_with: String,
        regex: bool,
        comment: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return Err("当前项目尚未就绪或后台不可用；规则未修改。".into());
        }
        let prev = self.workspace.match_and_replace_rules.clone();
        if let Some(edit_id) = editing_id
            && let Some(existing) = self
                .workspace
                .match_and_replace_rules
                .iter_mut()
                .find(|r| r.id == edit_id)
        {
            existing.item_type = item_type;
            existing.match_pattern = match_pattern;
            existing.replace_with = replace_with;
            existing.regex = regex;
            existing.comment = comment;
        } else {
            self.workspace
                .match_and_replace_rules
                .push(MatchAndReplaceRule {
                    id: Uuid::new_v4(),
                    enabled: true,
                    item_type,
                    match_pattern,
                    replace_with,
                    regex,
                    comment,
                    script_name: String::new(),
                    filter_script: String::new(),
                });
        }
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.match_and_replace_rules = prev;
            return Err(format!("未保存规则：{error}"));
        }
        self.sync_mr_table_selection(cx);
        self.save_workspace(cx);
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn save_http_match_replace_draft(
        &mut self,
        project: ProjectKey,
        original: Option<&MatchAndReplaceRule>,
        rule: MatchAndReplaceRule,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.project != Some(project) || !self.backend_available() || self.transition.is_some() {
            return Err("当前项目尚未就绪或后台不可用；规则未保存。".into());
        }
        let prev = self.workspace.match_and_replace_rules.clone();
        if let Some(orig) = original {
            let Some(existing) = self
                .workspace
                .match_and_replace_rules
                .iter_mut()
                .find(|r| r.id == orig.id)
            else {
                return Err("待修改的规则已不存在。".into());
            };
            *existing = rule;
        } else {
            self.workspace.match_and_replace_rules.push(rule);
        }
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.match_and_replace_rules = prev;
            return Err(format!("未保存规则：{error}"));
        }
        self.extra_tools.mr_editor_open = false;
        self.extra_tools.mr_editing = None;
        self.extra_tools.mr_dialog = None;
        self.extra_tools.mr_error = None;
        self.sync_mr_table_selection(cx);
        self.save_workspace(cx);
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn sync_ws_mr_table_selection(&mut self, cx: &mut Context<Self>) {
        let sel = self
            .extra_tools
            .ws_mr_selected
            .and_then(|id| {
                self.workspace
                    .ws_match_and_replace_rules
                    .iter()
                    .position(|r| r.id == id)
            });
        if sel.is_none() {
            self.extra_tools.ws_mr_selected = None;
        }
        self.extra_tools.ws_mr_table.update(cx, |table, cx| {
            if let Some(row) = sel {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
            cx.notify();
        });
    }

    pub(super) fn submit_ws_match_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let snapshots = [
            &self.extra_tools.ws_mr_match,
            &self.extra_tools.ws_mr_replace,
            &self.extra_tools.ws_mr_comment,
        ]
        .map(|input| editing::input_snapshot(input, window, cx));
        let [Ok(match_raw), Ok(replace_raw), Ok(comment_raw)] = snapshots else {
            self.extra_tools.ws_mr_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        };
        if match_raw.is_empty() {
            self.extra_tools.ws_mr_error = Some("请填写 WebSocket 匹配模式（不能为空）。".into());
            cx.notify();
            return;
        }
        let match_pattern = match_raw.to_string();
        let replace_with = replace_raw.to_string();
        let comment = comment_raw.trim().to_owned();
        let direction = self.extra_tools.ws_mr_direction;
        let is_regex = self.extra_tools.ws_mr_regex;
        let editing_id = self.extra_tools.ws_mr_editing;
        let outcome = self.update_proxy_advanced(
            self.project,
            |_, ws, _| {
                if let Some(id) = editing_id
                    && let Some(existing) = ws.iter_mut().find(|r| r.id == id)
                {
                    existing.direction = direction;
                    existing.match_pattern = match_pattern;
                    existing.replace_with = replace_with;
                    existing.is_regex = is_regex;
                    existing.comment = comment;
                } else {
                    ws.push(WsMatchAndReplaceRule {
                        id: Uuid::new_v4(),
                        enabled: true,
                        direction,
                        match_pattern,
                        replace_with,
                        is_regex,
                        comment,
                    });
                }
            },
            cx,
        );
        match outcome {
            Ok(()) => {
                self.extra_tools.ws_mr_editing = None;
                self.extra_tools.ws_mr_editor_open = false;
                self.extra_tools.ws_mr_error = None;
                self.extra_tools.ws_mr_regex = false;
                self.extra_tools.ws_mr_direction = WsReplaceDirection::Both;
                self.extra_tools
                    .ws_mr_direction_select
                    .update(cx, |state, cx| {
                        state.set_selected_index(
                            Some(gpui_kit::component::IndexPath::new(0)),
                            window,
                            cx,
                        );
                    });
                for input in [
                    &self.extra_tools.ws_mr_match,
                    &self.extra_tools.ws_mr_replace,
                    &self.extra_tools.ws_mr_comment,
                ] {
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                }
                self.sync_ws_mr_table_selection(cx);
            }
            Err(err) => self.extra_tools.ws_mr_error = Some(err),
        }
        cx.notify();
    }

    pub(super) fn load_ws_match_replace_for_edit(
        &mut self,
        rule: &WsMatchAndReplaceRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let any_composing = editing::input_snapshot(&self.extra_tools.ws_mr_match, window, cx).is_err()
            || editing::input_snapshot(&self.extra_tools.ws_mr_replace, window, cx).is_err()
            || editing::input_snapshot(&self.extra_tools.ws_mr_comment, window, cx).is_err();
        if any_composing {
            self.extra_tools.ws_mr_error = Some(editing::COMPOSITION_PENDING.into());
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        self.extra_tools.ws_mr_editing = Some(rule.id);
        self.extra_tools.ws_mr_editor_open = true;
        self.extra_tools.ws_mr_error = None;
        self.extra_tools.ws_mr_direction = rule.direction;
        self.extra_tools
            .ws_mr_direction_select
            .update(cx, |state, cx| {
                state.set_selected_index(
                    Some(gpui_kit::component::IndexPath::new(ws_mr_direction_index(
                        rule.direction,
                    ))),
                    window,
                    cx,
                );
            });
        self.extra_tools.ws_mr_regex = rule.is_regex;
        self.extra_tools
            .ws_mr_match
            .update(cx, |s, cx| s.set_value(rule.match_pattern.clone(), window, cx));
        self.extra_tools
            .ws_mr_replace
            .update(cx, |s, cx| s.set_value(rule.replace_with.clone(), window, cx));
        self.extra_tools
            .ws_mr_comment
            .update(cx, |s, cx| s.set_value(rule.comment.clone(), window, cx));
        cx.notify();
    }

    pub(super) fn toggle_ws_match_replace_rule(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self
            .update_proxy_advanced(
                self.project,
                |_, ws, _| {
                    if let Some(rule) = ws.iter_mut().find(|r| r.id == id) {
                        rule.enabled = !rule.enabled;
                    }
                },
                cx,
            )
            .is_ok()
        {
            self.sync_ws_mr_table_selection(cx);
        }
    }

    pub(super) fn remove_ws_match_replace_rule(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.extra_tools.ws_mr_editing == Some(id) {
            self.extra_tools.ws_mr_editing = None;
            self.extra_tools.ws_mr_editor_open = false;
            self.extra_tools.ws_mr_error = None;
        }
        if self
            .update_proxy_advanced(self.project, |_, ws, _| ws.retain(|r| r.id != id), cx)
            .is_ok()
        {
            self.sync_ws_mr_table_selection(cx);
        }
    }

    pub(super) fn move_ws_match_replace_rule(&mut self, id: Uuid, down: bool, cx: &mut Context<Self>) {
        if self
            .update_proxy_advanced(
                self.project,
                |_, ws, _| {
                    let Some(pos) = ws.iter().position(|r| r.id == id) else {
                        return;
                    };
                    let target = if down {
                        pos.checked_add(1).filter(|&p| p < ws.len())
                    } else {
                        pos.checked_sub(1)
                    };
                    if let Some(target) = target {
                        ws.swap(pos, target);
                    }
                },
                cx,
            )
            .is_ok()
        {
            self.sync_ws_mr_table_selection(cx);
        }
    }

    pub(super) fn set_all_ws_match_replace_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self
            .update_proxy_advanced(
                self.project,
                |_, ws, _| {
                    for rule in ws.iter_mut() {
                        rule.enabled = enabled;
                    }
                },
                cx,
            )
            .is_ok()
        {
            self.sync_ws_mr_table_selection(cx);
        }
    }

    pub(super) fn clear_ws_match_replace_rules(&mut self, cx: &mut Context<Self>) {
        if self
            .update_proxy_advanced(self.project, |_, ws, _| ws.clear(), cx)
            .is_ok()
        {
            self.extra_tools.ws_mr_editing = None;
            self.extra_tools.ws_mr_selected = None;
            self.extra_tools.ws_mr_editor_open = false;
            self.extra_tools.ws_mr_error = None;
            self.sync_ws_mr_table_selection(cx);
        }
    }
}
