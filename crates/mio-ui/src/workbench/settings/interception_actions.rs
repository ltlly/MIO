use gpui_kit::{Bounds, WindowOptions, size};
use mio_core::{
    ProxyInterceptOperator, ProxyInterceptRelation, ProxyInterceptRule, ProxyInterceptRuleSet,
    ProxyInterceptType, ProxyInterceptionSettings, Uuid,
};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn proxy_interception_workspace_acknowledged(
        &mut self,
        workspace: &Workspace,
        cx: &mut Context<Self>,
    ) -> bool {
        let snapshot_mismatch = self.pending_proxy_interception.as_ref().is_some_and(
            |(revision, settings, messages, ws_settings)| {
                *revision == workspace.revision
                    && (settings != &workspace.proxy_interception
                        || messages != &workspace.proxy_messages
                        || ws_settings != &workspace.proxy_websocket_interception)
            },
        );
        let editor = self
            .settings_window
            .and_then(|settings| {
                settings
                    .read::<gpui_kit::base::Root, _, _>(cx, |root, cx| {
                        root.read(cx)
                            .view()
                            .clone()
                            .downcast::<Settings>()
                            .ok()
                            .and_then(|settings| settings.read(cx).interception_dialog)
                    })
                    .ok()
                    .flatten()
            })
            .and_then(|dialog| {
                dialog
                    .read::<gpui_kit::base::Root, _, _>(cx, |root, cx| {
                        root.read(cx)
                            .view()
                            .clone()
                            .downcast::<InterceptionEditor>()
                            .ok()
                    })
                    .ok()
                    .flatten()
            });
        if snapshot_mismatch {
            if let Some(editor) = &editor {
                editor.update(cx, |editor, cx| {
                    if self.project == Some(editor.project)
                        && editor.pending == Some(workspace.revision)
                    {
                        editor.error = Some(SNAPSHOT_ERROR.into());
                        cx.notify();
                    }
                });
            }
            self.fail(SNAPSHOT_ERROR, cx);
            return false;
        }
        let valid = editor.is_none_or(|editor| {
            editor.update(cx, |editor, cx| {
                if self.project != Some(editor.project)
                    || editor.pending != Some(workspace.revision)
                {
                    return true;
                }
                if let Some((revision, settings)) = &editor.prior_save
                    && *revision == workspace.revision
                {
                    if settings == &workspace.proxy_interception {
                        editor.prior_save = None;
                        return true;
                    }
                    editor.error = Some(
                        "此前工作区保存回执的规则快照不匹配；输入保留，未推进当前候选。".into(),
                    );
                    cx.notify();
                    return false;
                }
                if editor.submitted.as_ref() == Some(&workspace.proxy_interception) {
                    editor.acknowledged = true;
                    return true;
                }
                editor.error =
                    Some("规则保存回执与提交的候选不匹配；输入仍保留，未确认保存。".into());
                cx.notify();
                false
            })
        });
        if !valid {
            self.fail(
                "规则保存回执与提交的候选不匹配；未推进保存状态，当前草稿仍保留。",
                cx,
            );
        } else if self.error.as_deref() == Some(SNAPSHOT_ERROR) {
            self.error = None;
        }
        valid
    }
}

impl InterceptionEditor {
    pub(in crate::workbench) fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.checking.is_some() {
            return;
        }
        let condition = match editing::input_snapshot(&self.condition, window, cx) {
            Ok(condition) => condition,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let mut rule = self.rule.clone();
        rule.condition = if rule.relationship.is_regex() {
            condition.to_string()
        } else {
            String::new()
        };
        let mut settings = self.base.clone();
        let set = intercept_set_mut(&mut settings, self.response);
        if self.new {
            set.rules.push(rule);
        } else if let Some(current) = set.rules.iter_mut().find(|current| current.id == rule.id) {
            *current = rule;
        } else {
            self.error = Some("编辑的规则已不存在；输入保留。".into());
            cx.notify();
            return;
        }
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        let base = self.base.clone();
        let project = self.project;
        let revision = self.revision;
        let operation = Uuid::new_v4();
        self.checking = Some(operation);
        self.submitted = Some(settings.clone());
        self.acknowledged = false;
        self.error = None;
        let validate = cx.background_executor().spawn(async move {
            settings.validate().map_err(|error| error.to_string())?;
            Ok::<_, String>(settings)
        });
        self.validation = Some(cx.spawn_in(window, async move |this, cx| {
            let result = validate.await;
            let active = this
                .update(cx, |this, _| this.checking == Some(operation))
                .unwrap_or(false);
            if !active {
                return;
            }
            let settings = match result {
                Ok(settings) => settings,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.checking = None;
                        this.error = Some(error);
                        cx.notify();
                    });
                    return;
                }
            };
            let result = owner_window.update(cx, |_, _, cx| {
                owner.update(cx, |view, cx| {
                    if view.project != Some(project)
                        || view.workspace.revision != revision
                        || view.workspace.proxy_interception != base
                    {
                        return Err(
                            "项目或规则 revision 已改变；输入保留，未覆盖最新配置。".to_owned()
                        );
                    }
                    let prior_save = view
                        .pending_proxy_interception
                        .as_ref()
                        .filter(|(revision, _, _, _)| Some(*revision) == view.pending_workspace)
                        .map(|(revision, rules, _, _)| (*revision, rules.clone()));
                    let unchanged = view.workspace.proxy_interception == settings;
                    if unchanged && !view.workspace_dirty && view.pending_workspace.is_none() {
                        return Ok((None, None));
                    }
                    if view.set_proxy_interception(Some(project), settings, cx)
                        && (!unchanged || !view.workspace_dirty || view.save_workspace(cx))
                    {
                        Ok((
                            Some(view.pending_workspace.unwrap_or(view.workspace.revision)),
                            prior_save,
                        ))
                    } else {
                        Err(view
                            .error
                            .clone()
                            .unwrap_or_else(|| "规则未保存；输入保留。".into()))
                    }
                })
            });
            let _ = this.update_in(cx, |this, window, cx| {
                this.checking = None;
                match result {
                    Ok(Ok(Ok((Some(revision), prior_save)))) => {
                        this.pending = Some(revision);
                        this.prior_save = prior_save;
                    }
                    Ok(Ok(Ok((None, _)))) => {
                        window.remove_window();
                        return;
                    }
                    Ok(Ok(Err(error))) => {
                        this.pending = None;
                        this.error = Some(error);
                    }
                    _ => {
                        this.pending = None;
                        this.error = Some("主工作台已关闭；规则未应用。".into());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Settings {
    pub(in crate::workbench) fn interception_available(&self) -> bool {
        self.state.enabled
            && !self.interception_opening
            && self.interception_dialog.is_none()
            && self.interception_restore.is_none()
    }

    pub(in crate::workbench) fn change_interception(
        &mut self,
        project: Option<ProjectKey>,
        response: bool,
        base: &ProxyInterceptionSettings,
        change: impl FnOnce(&mut ProxyInterceptRuleSet),
        cx: &mut Context<Self>,
    ) {
        if !self.interception_available() {
            return;
        }
        let mut settings = base.clone();
        change(intercept_set_mut(&mut settings, response));
        let result = self.owner.update(cx, |view, cx| {
            if view.project != project || view.workspace.proxy_interception != *base {
                return Err("项目或拦截规则已改变；未覆盖新配置，请重新核对。".to_owned());
            }
            if view.set_proxy_interception(project, settings, cx) {
                Ok(())
            } else {
                Err(view
                    .error
                    .clone()
                    .unwrap_or_else(|| "拦截规则未保存，当前输入仍保留。".into()))
            }
        });
        self.interception_error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(_) => Some("工作台已关闭；拦截规则未修改。".into()),
        };
        cx.notify();
    }

    pub(in crate::workbench) fn open_interception_editor(
        &mut self,
        response: bool,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.interception_dialog {
            if crate::chrome::activate_popup_window(dialog, cx) {
                return;
            }
            self.interception_dialog = None;
        }
        if !self.interception_available() {
            return;
        }
        let Some(project) = self.state.project else {
            return;
        };
        let base = self.state.proxy_interception.clone();
        let rule = if new {
            ProxyInterceptRule {
                id: Uuid::new_v4(),
                enabled: true,
                operator: ProxyInterceptOperator::And,
                match_type: ProxyInterceptType::DomainName,
                relationship: ProxyInterceptRelation::Matches,
                condition: String::new(),
            }
        } else {
            let Some(rule) = self.interception_selected[usize::from(response)]
                .and_then(|id| {
                    intercept_set(&base, response)
                        .rules
                        .iter()
                        .find(|rule| rule.id == id)
                })
                .cloned()
            else {
                return;
            };
            rule
        };
        let Some(display) = window.display(cx) else {
            self.interception_error =
                Some("无法确定设置窗口所在显示器；未打开规则编辑窗口。".into());
            cx.notify();
            return;
        };
        let visible = display.visible_bounds();
        let scale = theme::ui_scale();
        let content_w = if scale <= 1.0 {
            px(522.)
        } else {
            theme::scaled_px(522.)
        };
        let content_h = theme::scaled_px(276.).max(px(276.));
        let raw_h = if cfg!(test) {
            content_h
        } else {
            let bar_h = if scale <= 1.0 {
                px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
            } else {
                theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                    .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
            };
            content_h + bar_h
        };
        let dimensions = if scale <= 1.0 {
            size(content_w, raw_h)
        } else {
            size(
                content_w.min(visible.size.width),
                raw_h.min(visible.size.height),
            )
        };
        let mut bounds =
            Bounds::centered_at(window.window_bounds().get_bounds().center(), dimensions);
        bounds.origin.x = bounds.origin.x.clamp(
            visible.origin.x,
            (visible.right() - dimensions.width).max(visible.origin.x),
        );
        bounds.origin.y = bounds.origin.y.clamp(
            visible.origin.y,
            (visible.bottom() - dimensions.height).max(visible.origin.y),
        );
        let options = WindowOptions {
            display_id: Some(display.id()),
            ..crate::chrome::mio_window_options(bounds, false, false)
        };
        let owner = self.owner.clone();
        let settings = cx.weak_entity();
        let revision = self.state.revision;
        self.interception_opening = true;
        cx.defer(move |cx| {
            let Some(entity) = settings.upgrade() else {
                return;
            };
            if entity.read(cx).state.project != Some(project) {
                entity.update(cx, |this, cx| {
                    this.interception_opening = false;
                    cx.notify();
                });
                return;
            }
            let opened = crate::chrome::open_popup_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    InterceptionEditor::new(
                        (owner, settings.clone()),
                        (project, revision, base, rule, response, new),
                        window,
                        cx,
                    )
                })
            });
            entity.update(cx, |this, cx| {
                this.interception_opening = false;
                match opened {
                    Ok((window, _)) => this.interception_dialog = Some(window),
                    Err(error) => {
                        this.interception_error = Some(format!("无法打开拦截规则编辑窗口：{error}"))
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }
}
