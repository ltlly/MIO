use super::*;

const MAX_SCOPE_LOAD_FILE_BYTES: usize = 256 * 1024;

impl Workbench {
    pub(in crate::workbench) fn validate_target_scope_ime(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        for input in self
            .target_pane
            .simple_inputs()
            .into_iter()
            .chain(&self.target_pane.advanced_scope_inputs.include)
            .chain(&self.target_pane.advanced_scope_inputs.exclude)
        {
            editing::input_snapshot(input, window, cx)?;
        }
        Ok(())
    }

    pub(in crate::workbench) fn open_simple_scope_add(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let side = usize::from(!include);
        let input = if include {
            self.target_pane.simple_scope_inputs.include.clone()
        } else {
            self.target_pane.simple_scope_inputs.exclude.clone()
        };
        self.target_pane.editing_simple_scope[side] = None;
        self.target_pane.simple_scope_include_subdomains[side] = false;
        self.target_pane.scope_editor_open[side] = true;
        input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn open_advanced_scope_add(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let side = usize::from(!include);
        let inputs = if include {
            self.target_pane.advanced_scope_inputs.include.clone()
        } else {
            self.target_pane.advanced_scope_inputs.exclude.clone()
        };
        self.target_pane.editing_advanced_scope[side] = None;
        self.target_pane.scope_editor_open[side] = true;
        self.target_pane.scope_protocol = ScopeProtocol::Any;
        for input in &inputs {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        inputs[0].update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn submit_target_simple_scope(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let input = if include {
            self.target_pane.simple_scope_inputs.include.clone()
        } else {
            self.target_pane.simple_scope_inputs.exclude.clone()
        };
        let prefix = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let side = usize::from(!include);
        let editing_id = self.target_pane.editing_simple_scope[side];
        let include_subdomains = self.target_pane.simple_scope_include_subdomains[side];
        if self.upsert_simple_scope_rule_with_subdomains(
            self.project,
            include,
            editing_id,
            prefix.clone(),
            Some(include_subdomains),
            cx,
        ) {
            self.target_pane.editing_simple_scope[side] = None;
            self.target_pane.simple_scope_include_subdomains[side] = false;
            self.target_pane.scope_editor_open[side] = false;
            let rules = if include {
                &self.workspace.scope.include
            } else {
                &self.workspace.scope.exclude
            };
            self.target_pane.selected_simple_scope[side] = editing_id
                .or_else(|| rules.iter().find(|r| r.prefix == prefix).map(|r| r.id));
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    pub(in crate::workbench) fn edit_simple_scope_rule(
        &mut self,
        include: bool,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let input = if include {
            self.target_pane.simple_scope_inputs.include.clone()
        } else {
            self.target_pane.simple_scope_inputs.exclude.clone()
        };
        if let Err(error) = editing::input_snapshot(&input, window, cx) {
            self.fail(error, cx);
            return;
        }
        let rules = if include {
            &self.workspace.scope.include
        } else {
            &self.workspace.scope.exclude
        };
        let Some(rule) = rules.iter().find(|r| r.id == id) else {
            return;
        };
        let side = usize::from(!include);
        let prefix = rule.prefix.clone();
        let include_subdomains = rule.include_subdomains;
        self.target_pane.editing_simple_scope[side] = Some(id);
        self.target_pane.simple_scope_include_subdomains[side] = include_subdomains;
        self.target_pane.selected_simple_scope[side] = Some(id);
        self.target_pane.scope_editor_open[side] = true;
        input.update(cx, |state, cx| {
            state.set_value(prefix, window, cx);
            state.focus(window, cx);
        });
        self.error = None;
        cx.notify();
    }

    pub(in crate::workbench) fn cancel_edit_simple_scope_rule(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = if include {
            self.target_pane.simple_scope_inputs.include.clone()
        } else {
            self.target_pane.simple_scope_inputs.exclude.clone()
        };
        if let Err(error) = editing::input_snapshot(&input, window, cx) {
            self.fail(error, cx);
            return;
        }
        let side = usize::from(!include);
        self.target_pane.editing_simple_scope[side] = None;
        self.target_pane.simple_scope_include_subdomains[side] = false;
        self.target_pane.scope_editor_open[side] = false;
        input.update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn edit_advanced_scope_rule(
        &mut self,
        include: bool,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let inputs = if include {
            self.target_pane.advanced_scope_inputs.include.clone()
        } else {
            self.target_pane.advanced_scope_inputs.exclude.clone()
        };
        for input in &inputs {
            if let Err(error) = editing::input_snapshot(input, window, cx) {
                self.fail(error, cx);
                return;
            }
        }
        let rules = if include {
            &self.workspace.scope.advanced_include
        } else {
            &self.workspace.scope.advanced_exclude
        };
        let Some(rule) = rules.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        let side = usize::from(!include);
        self.target_pane.editing_advanced_scope[side] = Some(id);
        self.target_pane.selected_advanced_scope[side] = Some(id);
        self.target_pane.scope_editor_open[side] = true;
        self.target_pane.scope_protocol = rule.protocol;
        for (input, val) in inputs.iter().zip([rule.host, rule.port, rule.file]) {
            input.update(cx, |state, cx| state.set_value(val, window, cx));
        }
        inputs[0].update(cx, |state, cx| state.focus(window, cx));
        self.error = None;
        cx.notify();
    }

    pub(in crate::workbench) fn cancel_edit_advanced_scope_rule(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inputs = if include {
            self.target_pane.advanced_scope_inputs.include.clone()
        } else {
            self.target_pane.advanced_scope_inputs.exclude.clone()
        };
        for input in &inputs {
            if let Err(error) = editing::input_snapshot(input, window, cx) {
                self.fail(error, cx);
                return;
            }
        }
        let side = usize::from(!include);
        self.target_pane.editing_advanced_scope[side] = None;
        self.target_pane.scope_editor_open[side] = false;
        for input in &inputs {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn paste_scope_url_from_clipboard(
        &mut self,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(raw) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.fail("剪贴板中没有可粘贴的 URL 文本。", cx);
            return;
        };
        let lines: Vec<String> = raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_owned)
            .collect();
        if lines.is_empty() {
            self.fail("剪贴板中没有有效的 URL 条目。", cx);
            return;
        }
        self.add_scope_url_lines(include, &lines, "已从剪贴板添加", cx);
    }

    pub(in crate::workbench) fn load_scope_urls_from_file(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() || self.file_picker_pending() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(
                    if include {
                        "加载包含目标范围 URL 列表"
                    } else {
                        "加载排除目标范围 URL 列表"
                    }
                    .into(),
                ),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let paths = paths.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                    return;
                }
                match paths {
                    Ok(Ok(Some(paths))) => {
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        match mio_core::ui_files::read_file_bytes(&path) {
                            Ok(bytes) if bytes.len() > MAX_SCOPE_LOAD_FILE_BYTES => {
                                view.fail("范围文件超过 256 KB 上限，未导入。", cx);
                            }
                            Ok(bytes) => match String::from_utf8(bytes) {
                                Ok(text) => {
                                    let lines: Vec<String> = text
                                        .lines()
                                        .map(str::trim)
                                        .filter(|line| !line.is_empty() && !line.starts_with('#'))
                                        .map(str::to_owned)
                                        .collect();
                                    if lines.is_empty() {
                                        view.fail("所选文件中没有有效的 URL 或规则行。", cx);
                                    } else {
                                        view.add_scope_url_lines(
                                            include,
                                            &lines,
                                            "已从文件加载",
                                            cx,
                                        );
                                    }
                                }
                                Err(_) => view.fail("所选范围文件不是有效的 UTF-8 文本。", cx),
                            },
                            Err(error) => view.fail(format!("读取范围文件失败：{error}"), cx),
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => view.fail(format!("选择范围文件失败：{error}"), cx),
                    Err(_) => view.fail("文件选择器已关闭。", cx),
                }
            });
        })
        .detach();
    }

    fn add_scope_url_lines(
        &mut self,
        include: bool,
        lines: &[String],
        verb: &str,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.workspace.scope.clone();
        let previous = self.workspace.scope.clone();
        let mut added = 0usize;
        for line in lines {
            if line.len() > 2048 || line.chars().any(char::is_control) {
                continue;
            }
            if next.advanced_mode {
                let (protocol, host, port, file) = split_advanced_scope_url(line);
                let rule = AdvancedScopeRule {
                    id: Uuid::new_v4(),
                    enabled: true,
                    protocol,
                    host,
                    port,
                    file,
                };
                if include {
                    next.advanced_include.push(rule);
                } else {
                    next.advanced_exclude.push(rule);
                }
                added += 1;
            } else {
                let rules = if include {
                    &mut next.include
                } else {
                    &mut next.exclude
                };
                if let Some(existing) = rules.iter_mut().find(|r| r.prefix == *line) {
                    existing.enabled = true;
                } else {
                    rules.push(ScopeRule {
                        id: Uuid::new_v4(),
                        enabled: true,
                        prefix: line.clone(),
                        include_subdomains: false,
                    });
                }
                added += 1;
            }
        }
        if added == 0 {
            self.fail("没有符合格式要求的 URL 规则可导入。", cx);
            return;
        }
        if let Err(error) = next.validate() {
            self.fail(error.to_string(), cx);
            return;
        }
        self.workspace.scope = next;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope = previous;
            self.fail(format!("未导入范围规则：{error}"), cx);
            return;
        }
        self.save_workspace(cx);
        self.error = None;
        self.status = format!("{verb} {added} 条目标范围规则。");
        cx.notify();
    }

    pub(in crate::workbench) fn copy_target_scope_json(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.validate_target_scope_ime(window, cx) {
            self.fail(error, cx);
            return;
        }
        match export_target_scope_json(&self.workspace.scope) {
            Ok(json) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(json));
                self.error = None;
                self.status = "已复制 Target Scope (JSON) 到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn paste_target_scope_json(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        if let Err(error) = self.validate_target_scope_ime(window, cx) {
            self.fail(error, cx);
            return;
        }
        let Some(raw) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.fail("剪贴板中没有可导入的 Scope JSON 文本。", cx);
            return;
        };
        let next = match parse_target_scope_json(&raw) {
            Ok(scope) => scope,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let previous = self.workspace.scope.clone();
        self.workspace.scope = next;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope = previous;
            self.fail(format!("未导入 Scope JSON：{error}"), cx);
            return;
        }
        self.target_pane.advanced_mode_input = self.workspace.scope.advanced_mode;
        self.target_pane.editing_simple_scope = [None, None];
        self.target_pane.simple_scope_include_subdomains = [false, false];
        self.target_pane.editing_advanced_scope = [None, None];
        self.target_pane.selected_simple_scope = [None, None];
        self.target_pane.selected_advanced_scope = [None, None];
        self.target_pane.scope_editor_open = [false, false];
        self.target_pane.settings_editing_scope = [None, None];
        for input in self
            .target_pane
            .simple_inputs()
            .into_iter()
            .chain(&self.target_pane.advanced_scope_inputs.include)
            .chain(&self.target_pane.advanced_scope_inputs.exclude)
        {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.save_workspace(cx);
        self.error = None;
        self.status = "已从剪贴板导入 Target Scope (JSON)。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_scope_rule_subdomains(
        &mut self,
        project: Option<ProjectKey>,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        if let Some(rule) = self
            .workspace
            .scope
            .include
            .iter_mut()
            .chain(&mut self.workspace.scope.exclude)
            .find(|rule| rule.id == id)
        {
            rule.include_subdomains = !rule.include_subdomains;
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn upsert_simple_scope_rule(
        &mut self,
        project: Option<ProjectKey>,
        include: bool,
        editing_id: Option<Uuid>,
        prefix: String,
        cx: &mut Context<Self>,
    ) -> bool {
        self.upsert_simple_scope_rule_with_subdomains(project, include, editing_id, prefix, None, cx)
    }

    pub(in crate::workbench) fn upsert_simple_scope_rule_with_subdomains(
        &mut self,
        project: Option<ProjectKey>,
        include: bool,
        editing_id: Option<Uuid>,
        prefix: String,
        include_subdomains: Option<bool>,
        cx: &mut Context<Self>,
    ) -> bool {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return false;
        }
        let prefix = prefix.trim().to_owned();
        if prefix.is_empty() || prefix.len() > 2048 || prefix.chars().any(char::is_control) {
            self.fail("Scope 规则前缀不能为空、包含控制字符或超过 2048 字节", cx);
            return false;
        }
        let mut next = self.workspace.scope.clone();
        let previous = self.workspace.scope.clone();
        let rules = if include {
            &mut next.include
        } else {
            &mut next.exclude
        };
        let updated = if let Some(id) = editing_id
            && let Some(existing) = rules.iter_mut().find(|rule| rule.id == id)
        {
            existing.prefix = prefix;
            if let Some(sub) = include_subdomains {
                existing.include_subdomains = sub;
            }
            true
        } else if let Some(existing) = rules.iter_mut().find(|rule| rule.prefix == prefix) {
            existing.enabled = true;
            if let Some(sub) = include_subdomains {
                existing.include_subdomains = sub;
            }
            false
        } else {
            rules.push(ScopeRule {
                id: Uuid::new_v4(),
                enabled: true,
                prefix,
                include_subdomains: include_subdomains.unwrap_or(false),
            });
            false
        };
        if let Err(error) = next.validate() {
            self.fail(error.to_string(), cx);
            return false;
        }
        self.workspace.scope = next;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope = previous;
            self.fail(error.to_string(), cx);
            return false;
        }
        self.save_workspace(cx);
        self.error = None;
        self.status = if updated {
            "已更新目标范围规则。".into()
        } else if include {
            "已添加到目标范围。".into()
        } else {
            "已从目标范围排除。".into()
        };
        cx.notify();
        true
    }

    pub(in crate::workbench) fn apply_imported_target_scope(
        &mut self,
        project: Option<ProjectKey>,
        next: TargetScope,
        cx: &mut Context<Self>,
    ) -> bool {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return false;
        }
        let previous = self.workspace.scope.clone();
        self.workspace.scope = next;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope = previous;
            self.fail(format!("未导入 Scope JSON：{error}"), cx);
            return false;
        }
        self.target_pane.advanced_mode_input = self.workspace.scope.advanced_mode;
        self.target_pane.editing_simple_scope = [None, None];
        self.target_pane.simple_scope_include_subdomains = [false, false];
        self.target_pane.editing_advanced_scope = [None, None];
        self.target_pane.selected_simple_scope = [None, None];
        self.target_pane.selected_advanced_scope = [None, None];
        self.target_pane.scope_editor_open = [false, false];
        self.target_pane.settings_editing_scope = [None, None];
        self.save_workspace(cx);
        self.error = None;
        self.status = "已从剪贴板导入 Target Scope (JSON)。".into();
        cx.notify();
        true
    }

    pub(in crate::workbench) fn render_advanced_scope(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editable =
            self.project.is_some() && self.transition.is_none() && self.backend_available();
        let include_section = self.render_advanced_scope_section(true, editable, cx);
        let exclude_section = self.render_advanced_scope_section(false, editable, cx);
        div()
            .w_full()
            .max_w(px(860.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(include_section)
            .child(exclude_section)
            .into_any_element()
    }

    fn render_advanced_scope_section(
        &self,
        include: bool,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let side = usize::from(!include);
        let rules = if include {
            &self.workspace.scope.advanced_include
        } else {
            &self.workspace.scope.advanced_exclude
        };
        let selected_id = self.target_pane.selected_advanced_scope[side]
            .filter(|id| rules.iter().any(|r| r.id == *id));
        let editor_open = self.target_pane.scope_editor_open[side];
        let editing = self.target_pane.editing_advanced_scope[side].is_some();
        let protocol = self.target_pane.scope_protocol;
        let inputs = if include {
            self.target_pane.advanced_scope_inputs.include.clone()
        } else {
            self.target_pane.advanced_scope_inputs.exclude.clone()
        };
        let fields = if include {
            ADVANCED_INCLUDE_FIELDS
        } else {
            ADVANCED_EXCLUDE_FIELDS
        };
        let (
            title,
            add_id,
            edit_btn_id,
            remove_btn_id,
            paste_btn_id,
            load_btn_id,
            save_id,
            cancel_id,
        ) = if include {
            (
                "包含在范围内",
                "target-scope-adv-include-add",
                "target-scope-adv-include-edit-btn",
                "target-scope-adv-include-remove-btn",
                "target-scope-adv-include-paste-url",
                "target-scope-adv-include-load",
                "target-scope-adv-include-save",
                "target-scope-adv-include-cancel",
            )
        } else {
            (
                "从范围中排除",
                "target-scope-adv-exclude-add",
                "target-scope-adv-exclude-edit-btn",
                "target-scope-adv-exclude-remove-btn",
                "target-scope-adv-exclude-paste-url",
                "target-scope-adv-exclude-load",
                "target-scope-adv-exclude-save",
                "target-scope-adv-exclude-cancel",
            )
        };
        let rows = self.render_advanced_rules(include, selected_id, cx);
        let header_bg = table_header_bg();
        let header_fg = table_header_fg();

        let side_buttons = div()
            .w(theme::scaled_px(102.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(
                mio_neutral_button(add_id, "添加", Some(102.))
                    .tooltip("添加新的高级正则范围规则")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.target_pane.scope_editor_open[side] {
                            this.add_advanced_scope_rule(include, window, cx);
                        } else {
                            this.open_advanced_scope_add(include, window, cx);
                        }
                    })),
            )
            .child(
                mio_neutral_button(edit_btn_id, "编辑", Some(102.))
                    .tooltip("编辑当前选中的高级正则范围规则")
                    .disabled(!editable || selected_id.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(id) = selected_id {
                            this.edit_advanced_scope_rule(include, id, window, cx);
                        }
                    })),
            )
            .child(
                mio_neutral_button(remove_btn_id, "移除", Some(102.))
                    .tooltip("移除当前选中的高级正则范围规则")
                    .disabled(!editable || selected_id.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(id) = selected_id {
                            this.remove_advanced_scope_rule(include, id, cx);
                        }
                    })),
            )
            .child(
                mio_neutral_button(paste_btn_id, "粘贴 URL", Some(102.))
                    .tooltip("从剪贴板粘贴 URL 并自动拆分为正则规则")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.paste_scope_url_from_clipboard(include, cx);
                    })),
            )
            .child(
                mio_neutral_button(load_btn_id, "加载…", Some(102.))
                    .tooltip("从本地文本文件批量加载 URL 并拆分为高级正则规则")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.load_scope_urls_from_file(include, window, cx);
                    })),
            );

        let table_header = div()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(header_bg)
            .text_color(header_fg)
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .w(theme::scaled_px(68.))
                    .h_full()
                    .flex_shrink_0()
                    .px(px(6.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("Enabled"),
            )
            .child(
                div()
                    .w(theme::scaled_px(72.))
                    .h_full()
                    .flex_shrink_0()
                    .px(px(6.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("Protocol"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(120.))
                    .h_full()
                    .px(px(6.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("Host / IP range"),
            )
            .child(
                div()
                    .w(theme::scaled_px(72.))
                    .h_full()
                    .flex_shrink_0()
                    .px(px(6.))
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .child("Port"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(100.))
                    .h_full()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .child("File"),
            );

        let table_box = div()
            .flex_1()
            .min_w_0()
            .h(theme::scaled_px(170.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(table_header)
            .child(rows);

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child(title),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(side_buttons)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .child(table_box)
                            .child(mio_scope_table_triangle()),
                    ),
            )
            .when(editor_open, |section| {
                section.child(
                    div().pl(theme::scaled_px(110.)).child(
                        div()
                            .w_full()
                            .p(px(8.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::toolbar_surface())
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("协议："),
                                    )
                                    .child(
                                        button(
                                            if include {
                                                "target-scope-protocol-any"
                                            } else {
                                                "target-scope-exclude-protocol-any"
                                            },
                                            "Any",
                                        )
                                        .selected(protocol == ScopeProtocol::Any)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.target_pane.scope_protocol = ScopeProtocol::Any;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        button(
                                            if include {
                                                "target-scope-protocol-http"
                                            } else {
                                                "target-scope-exclude-protocol-http"
                                            },
                                            "HTTP",
                                        )
                                        .selected(protocol == ScopeProtocol::Http)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.target_pane.scope_protocol = ScopeProtocol::Http;
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        button(
                                            if include {
                                                "target-scope-protocol-https"
                                            } else {
                                                "target-scope-exclude-protocol-https"
                                            },
                                            "HTTPS",
                                        )
                                        .selected(protocol == ScopeProtocol::Https)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.target_pane.scope_protocol = ScopeProtocol::Https;
                                            cx.notify();
                                        })),
                                    )
                                    .when(include, |row| {
                                        row.child(div().flex_1()).child(
                                            button("target-scope-advanced-paste", "按首框 URL 拆分")
                                                .icon(
                                                    Icon::new(IconName::ClipboardPaste)
                                                        .size(px(12.)),
                                                )
                                                .disabled(!editable)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.split_advanced_scope_url(window, cx);
                                                })),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .children(inputs.iter().zip(fields).map(
                                        |(input, (id, label))| {
                                            div().flex_1().min_w_0().child(
                                                Input::new(input)
                                                    .id(id)
                                                    .aria_label(label)
                                                    .small()
                                                    .disabled(!editable),
                                            )
                                        },
                                    ))
                                    .child(
                                        button(save_id, if editing { "保存" } else { "确定" })
                                            .primary()
                                            .disabled(!editable)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.add_advanced_scope_rule(include, window, cx);
                                            })),
                                    )
                                    .child(
                                        button(cancel_id, "取消")
                                            .disabled(!editable)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.cancel_edit_advanced_scope_rule(
                                                    include, window, cx,
                                                );
                                            })),
                                    ),
                            ),
                    ),
                )
            })
            .into_any_element()
    }
}
