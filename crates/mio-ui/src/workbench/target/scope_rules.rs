use super::*;

impl Workbench {
    pub(in crate::workbench) fn add_advanced_scope_rule(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let mut values: [String; 3] = std::array::from_fn(|_| String::new());
        let inputs = if include {
            &self.target_pane.advanced_scope_inputs.include
        } else {
            &self.target_pane.advanced_scope_inputs.exclude
        };
        for (index, input) in inputs.iter().enumerate() {
            match editing::input_snapshot(input, window, cx) {
                Ok(value) => values[index] = value.trim().to_owned(),
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            }
        }
        let rule = AdvancedScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            protocol: self.target_pane.scope_protocol,
            host: values[0].clone(),
            port: values[1].clone(),
            file: values[2].clone(),
        };
        let mut next = self.workspace.scope.clone();
        let previous = self.workspace.scope.clone();
        let side = usize::from(!include);
        let editing_id = self.target_pane.editing_advanced_scope[side];
        let rules = if include {
            &mut next.advanced_include
        } else {
            &mut next.advanced_exclude
        };
        let new_id = rule.id;
        let updated = if let Some(id) = editing_id
            && let Some(existing) = rules.iter_mut().find(|item| item.id == id)
        {
            existing.protocol = rule.protocol;
            existing.host = rule.host;
            existing.port = rule.port;
            existing.file = rule.file;
            true
        } else {
            rules.push(rule);
            false
        };
        if let Err(error) = next.validate() {
            self.fail(error.to_string(), cx);
            return;
        }
        self.workspace.scope = next;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope = previous;
            self.fail(format!("未添加高级 Scope 规则：{error}"), cx);
            return;
        }
        self.target_pane.editing_advanced_scope[side] = None;
        self.target_pane.scope_editor_open[side] = false;
        self.target_pane.selected_advanced_scope[side] = editing_id.or(Some(new_id));
        self.save_workspace(cx);
        let inputs = if include {
            &self.target_pane.advanced_scope_inputs.include
        } else {
            &self.target_pane.advanced_scope_inputs.exclude
        };
        for input in inputs {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.status = if updated {
            "已更新高级目标范围规则。".into()
        } else if include {
            "已添加高级目标范围规则。".into()
        } else {
            "已添加高级目标范围排除规则。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn remove_advanced_scope_rule(
        &mut self,
        include: bool,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let side = usize::from(!include);
        if self.target_pane.editing_advanced_scope[side] == Some(id) {
            self.target_pane.editing_advanced_scope[side] = None;
        }
        if self.target_pane.selected_advanced_scope[side] == Some(id) {
            self.target_pane.selected_advanced_scope[side] = None;
        }
        if include {
            self.workspace
                .scope
                .advanced_include
                .retain(|rule| rule.id != id);
        } else {
            self.workspace
                .scope
                .advanced_exclude
                .retain(|rule| rule.id != id);
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_advanced_scope_rule(
        &mut self,
        include: bool,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let rules = if include {
            &mut self.workspace.scope.advanced_include
        } else {
            &mut self.workspace.scope.advanced_exclude
        };
        if let Some(rule) = rules.iter_mut().find(|rule| rule.id == id) {
            rule.enabled = !rule.enabled;
            self.save_workspace(cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_target_advanced_mode(&mut self, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        self.workspace.scope.advanced_mode = !self.workspace.scope.advanced_mode;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope.advanced_mode = !self.workspace.scope.advanced_mode;
            self.fail(error.to_string(), cx);
            return;
        }
        self.target_pane.advanced_mode_input = self.workspace.scope.advanced_mode;
        self.target_pane.scope_editor_open = [false, false];
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn add_advanced_scope_rule_from_prefix(
        &mut self,
        prefix: String,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        if prefix.contains("://") {
            let (protocol, host, port, file) = split_advanced_scope_url(&prefix);
            let rule = AdvancedScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                protocol,
                host,
                port,
                file,
            };
            let mut next = self.workspace.scope.clone();
            if include {
                next.advanced_include.push(rule);
            } else {
                next.advanced_exclude.push(rule);
            }
            if let Err(error) = next.validate() {
                self.fail(error.to_string(), cx);
                return;
            }
            self.workspace.scope = next;
            self.save_workspace(cx);
            return;
        }
        self.add_scope_rule(self.project, include, prefix, cx);
    }

    pub(in crate::workbench) fn modify_selected_target_scope(
        &mut self,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            return;
        };
        let url = match row {
            HistoryRow::Live(summary) => summary.url.clone(),
            HistoryRow::ExtProxy(archive) => {
                let Some(request) = archive.request.as_ref().or(archive.edited_request.as_ref())
                else {
                    self.fail("归档条目缺少请求 URL，未修改目标范围。", cx);
                    return;
                };
                let target = request.request_target.clone().unwrap_or_default();
                if target.starts_with('/') {
                    format!("{}{target}", archive.origin.as_deref().unwrap_or_default())
                } else {
                    target
                }
            }
        };
        let prefix = target_origin_from_url(&url);
        if self.workspace.scope.advanced_mode {
            self.add_advanced_scope_rule_from_prefix(prefix, include, cx);
        } else {
            self.add_scope_rule(self.project, include, prefix, cx);
        }
    }

    pub(in crate::workbench) fn send_target_history_transfer(
        &mut self,
        intruder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.selected_target_rows_in_order();
        if rows.is_empty() {
            return;
        }
        let entries: Vec<HistoryTransferEntry> = rows
            .iter()
            .filter_map(|row| self.target_transfer_entry(row))
            .collect();
        if entries.is_empty() {
            self.fail("所选条目没有可转送的请求；未创建标签。", cx);
            return;
        }
        self.begin_history_transfer(None, entries, intruder, window, cx);
    }

    pub(in crate::workbench) fn split_advanced_scope_url(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .target_pane
            .advanced_scope_inputs
            .include
            .first()
            .cloned()
        else {
            return;
        };
        let url = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let (protocol, host, port, file) = split_advanced_scope_url(&url);
        if protocol != ScopeProtocol::Any {
            self.target_pane.scope_protocol = protocol;
        }
        for (input, value) in self
            .target_pane
            .advanced_scope_inputs
            .include
            .iter()
            .zip([host, port, file])
        {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        cx.notify();
    }
}

pub(in crate::workbench) fn split_advanced_scope_url(
    url: &str,
) -> (ScopeProtocol, String, String, String) {
    AdvancedScopeRule::split_url_patterns(url)
}

pub(in crate::workbench) fn target_origin_from_url(url: &str) -> String {
    let Some((scheme, tail)) = url.split_once("://") else {
        return url.to_owned();
    };
    let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
    format!("{scheme}://{}", &tail[..authority_end])
}
