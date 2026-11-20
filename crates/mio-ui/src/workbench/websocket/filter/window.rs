use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

impl FilterWindow {
    pub(super) fn new(
        owner: Entity<Workbench>,
        owner_window: AnyWindowHandle,
        project: ProjectKey,
        applied: WebSocketHistoryFilter,
        in_scope_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let frame = window.bounds().size;
        let viewport = window.viewport_size();
        let scale = theme::ui_scale();
        let mut target_w = if scale > 1.0 {
            theme::scaled_px(FILTER_WINDOW_WIDTH)
        } else {
            px(FILTER_WINDOW_WIDTH)
        };
        let mut target_h = if scale > 1.0 {
            theme::scaled_px(FILTER_WINDOW_HEIGHT)
        } else {
            px(FILTER_WINDOW_HEIGHT)
        };
        if let Some(display) = window.display(cx) {
            let visible = display.visible_bounds().size;
            target_w = target_w.min(visible.width);
            target_h = target_h.min(visible.height);
        }
        let w = target_w - (frame.width - viewport.width);
        let h = target_h - (frame.height - viewport.height);
        if w > px(0.) && h > px(0.) {
            window.resize(size(w, h));
        }
        let port = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                applied
                    .listener_port
                    .map(|p| p.to_string())
                    .unwrap_or_default(),
            )
        });
        let term = cx.new(|cx| InputState::new(window, cx).default_value(applied.term.clone()));
        let script_mode = !applied.filter_script.is_empty();
        let initial_script = applied.filter_script.clone();
        let filter_script_editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language("javascript")
                .line_number(true)
                .soft_wrap(false)
                .auto_close(false)
                .autocorrect(false);
            if !initial_script.is_empty() {
                editor.set_value(initial_script, window, cx);
            }
            editor
        });
        let subscription = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let view = owner.read(cx);
            if view.project != Some(this.project) {
                window.remove_window();
                return;
            }
            this.applied = view.websocket.applied_filter.clone();
            this.applied_in_scope_only = view.websocket.in_scope_only;
            if let Some((revision, close)) = this.pending {
                let delegate = view.websocket.table.read(cx).delegate();
                if let Some((_, outcome)) = view
                    .websocket
                    .filter_outcome
                    .as_ref()
                    .filter(|(actual, _)| *actual == revision)
                {
                    this.pending = None;
                    this.error = outcome.clone().err();
                    if close && this.error.is_none() {
                        window.remove_window();
                        return;
                    }
                } else if revision != view.websocket.query_revision {
                    this.pending = None;
                    this.error = Some("筛选查询已被较新的操作替代；当前草稿仍保留。".into());
                } else if !delegate.loading {
                    this.pending = None;
                    this.error = delegate.error.clone();
                    if close && this.error.is_none() {
                        window.remove_window();
                        return;
                    }
                }
            }
            cx.notify();
        });
        let released = owner.downgrade();
        let handle = window.window_handle();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released.update(cx, |view, cx| {
                    if view.websocket.filter_window == Some(handle) {
                        view.websocket.filter_window = None;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        window.set_window_title("WebSockets 历史记录筛选");
        Self {
            owner: owner.downgrade(),
            owner_window,
            project,
            draft: applied.clone(),
            applied,
            applied_in_scope_only: in_scope_only,
            draft_in_scope_only: in_scope_only,
            script_mode,
            port,
            term,
            filter_script_editor,
            pending: None,
            submitting: false,
            error: None,
            focus,
            _subscription: subscription,
        }
    }

    fn restore(
        &mut self,
        filter: WebSocketHistoryFilter,
        in_scope_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.draft = filter;
        self.draft_in_scope_only = in_scope_only;
        self.script_mode = !self.draft.filter_script.is_empty();
        self.port.update(cx, |input, cx| {
            input.set_value(
                self.draft
                    .listener_port
                    .map(|port| port.to_string())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        self.term.update(cx, |input, cx| {
            input.set_value(self.draft.term.clone(), window, cx)
        });
        self.filter_script_editor.update(cx, |editor, cx| {
            editor.set_value(self.draft.filter_script.clone(), window, cx)
        });
        self.error = None;
        cx.notify();
    }

    fn snapshot_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let port = editing::input_snapshot(&self.port, window, cx).map_err(str::to_owned)?;
        let term = editing::input_snapshot(&self.term, window, cx).map_err(str::to_owned)?;
        self.draft.listener_port = if port.trim().is_empty() {
            None
        } else {
            match port.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => {
                    return Err("监听端口须为 1–65535 的整数，留空显示全部端口。".into());
                }
            }
        };
        self.draft.term = term.to_string();
        Ok(())
    }

    fn convert_to_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.snapshot_settings(window, cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        if let Err(error) = editing::snapshot(&self.filter_script_editor, window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        let script = mio_core::convert_ws_filter_to_script(&self.draft, self.draft_in_scope_only);
        self.filter_script_editor.update(cx, |editor, cx| editor.set_value(script.clone(), window, cx));
        self.draft.filter_script = script;
        self.script_mode = true;
        self.error = None;
        cx.notify();
    }

    fn load_script(&mut self, script: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = editing::snapshot(&self.filter_script_editor, window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        self.filter_script_editor.update(cx, |editor, cx| editor.set_value(script.to_owned(), window, cx));
        self.error = None;
        cx.notify();
    }

    fn apply(&mut self, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        if self.script_mode {
            let script = match editing::snapshot(&self.filter_script_editor, window, cx) {
                Ok(script) => script.to_string(),
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                    return;
                }
            };
            if !script.trim().is_empty() && let Err(error) = mio_core::validate_filter_script(&script) {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
            self.draft.filter_script = script;
        } else {
            if let Err(error) = self.snapshot_settings(window, cx) {
                self.error = Some(error);
                cx.notify();
                return;
            }
            self.draft.filter_script.clear();
        }
        let draft = self.draft.clone();
        let draft_in_scope = self.draft_in_scope_only;
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        let project = self.project;
        let dialog = cx.weak_entity();
        let dialog_window = window.window_handle();
        self.submitting = true;
        cx.notify();
        cx.defer(move |cx| {
            if !cx.windows().contains(&dialog_window) || dialog.upgrade().is_none() {
                return;
            }
            let result = owner_window.update(cx, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if view.project != Some(project) {
                        return Err("项目已改变；未应用旧筛选。".to_owned());
                    }
                    if !view.leave_websocket_notes(window, cx) {
                        return Err(view
                            .error
                            .clone()
                            .unwrap_or_else(|| "备注仍在编辑，请先完成输入。".into()));
                    }
                    view.query_websocket_filtered_with_scope(
                        view.websocket.filter,
                        0,
                        draft,
                        draft_in_scope,
                        view.websocket.filter_enabled,
                        window,
                        cx,
                    )
                    .ok_or_else(|| {
                        view.error
                            .clone()
                            .unwrap_or_else(|| "筛选查询未发送。".into())
                    })
                })
            });
            let _ = dialog.update(cx, |this, cx| {
                this.submitting = false;
                match result {
                    Ok(Ok(Ok(revision))) => {
                        this.pending = Some((revision, close));
                        this.error = None;
                    }
                    Ok(Ok(Err(error))) => this.error = Some(error),
                    _ => this.error = Some("项目窗口已关闭；未应用筛选。".into()),
                }
                cx.notify();
            });
        });
    }
}

impl Render for FilterWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = crate::chrome::render_title_bar(
            "mio-ws-filter",
            "WebSockets 历史记录筛选",
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let enabled = self.pending.is_none() && !self.submitting;
        let script_mode = self.script_mode;
        let outgoing = cx.weak_entity();
        let incoming = cx.weak_entity();
        let in_scope = cx.weak_entity();
        let annotated = cx.weak_entity();
        let regex = cx.weak_entity();
        let case_sensitive = cx.weak_entity();
        let negative = cx.weak_entity();
        let mode_bar = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("WebSockets 历史记录筛选"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        button("ws-filter-mode-settings", "Settings mode")
                            .selected(!script_mode)
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.script_mode = false;
                                this.error = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("ws-filter-mode-script", "Script mode (MioScript)")
                            .selected(script_mode)
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.script_mode = true;
                                this.error = None;
                                cx.notify();
                            })),
                    ),
            );
        let body = if script_mode {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child("// 入参: message (url, direction, payload, length, edited, notes, listener_port), in_scope -> 返回 bool"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    button("ws-filter-script-template", "Load template")
                                        .disabled(!enabled)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.load_script(
                                                mio_core::DEFAULT_WS_FILTER_SCRIPT_TEMPLATE,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(
                                    button("ws-filter-script-clear", "Clear script")
                                        .disabled(!enabled)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.load_script("", window, cx)
                                        })),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("ws-filter-script-editor")
                        .test_support()
                        .flex_1()
                        .min_h_0()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(4.))
                        .bg(theme::surface())
                        .child(
                            Editor::new(&self.filter_script_editor)
                                .size_full()
                                .readonly(!enabled)
                                .aria_label("WebSocket MioScript Script"),
                        ),
                )
        } else {
            div()
                .flex()
                .flex_wrap()
                .min_h_0()
                .gap(px(10.))
                .child(
                    div()
                        .w(theme::scaled_px(155.))
                        .flex_shrink_0()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(4.))
                        .bg(theme::surface())
                        .p(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child("按请求类型筛选")
                        .child(
                            intruder::resource_pools::pool_checkbox(
                                "websocket-hide-outgoing",
                                "隐藏传出消息",
                                self.draft.hide_outgoing,
                                enabled,
                            )
                            .on_change(
                                move |state, _, _, cx| {
                                    let _ = outgoing.update(cx, |this, cx| {
                                        this.draft.hide_outgoing =
                                            state == gpui_kit::base::CheckboxState::Checked;
                                        cx.notify();
                                    });
                                },
                            ),
                        )
                        .child(
                            intruder::resource_pools::pool_checkbox(
                                "websocket-hide-incoming",
                                "隐藏传入消息",
                                self.draft.hide_incoming,
                                enabled,
                            )
                            .on_change(
                                move |state, _, _, cx| {
                                    let _ = incoming.update(cx, |this, cx| {
                                        this.draft.hide_incoming =
                                            state == gpui_kit::base::CheckboxState::Checked;
                                        cx.notify();
                                    });
                                },
                            ),
                        )
                        .child(
                            intruder::resource_pools::pool_checkbox(
                                "websocket-in-scope-only",
                                "仅显示范围内条目 (In-scope only)",
                                self.draft_in_scope_only,
                                enabled,
                            )
                            .on_change(
                                move |state, _, _, cx| {
                                    let _ = in_scope.update(cx, |this, cx| {
                                        this.draft_in_scope_only =
                                            state == gpui_kit::base::CheckboxState::Checked;
                                        cx.notify();
                                    });
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(theme::scaled_px(220.))
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(4.))
                        .bg(theme::surface())
                        .p(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child("按搜索词筛选")
                        .child(Input::new(&self.term).small().disabled(!enabled))
                        .child(
                            intruder::resource_pools::pool_checkbox(
                                "websocket-filter-regex",
                                "Regex",
                                self.draft.regex,
                                enabled,
                            )
                            .on_change(
                                move |state, _, _, cx| {
                                    let _ = regex.update(cx, |this, cx| {
                                        this.draft.regex =
                                            state == gpui_kit::base::CheckboxState::Checked;
                                        cx.notify();
                                    });
                                },
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(10.))
                                .child(
                                    intruder::resource_pools::pool_checkbox(
                                        "websocket-filter-case",
                                        "区分大小写",
                                        self.draft.case_sensitive,
                                        enabled,
                                    )
                                    .on_change(
                                        move |state, _, _, cx| {
                                            let _ = case_sensitive.update(cx, |this, cx| {
                                                this.draft.case_sensitive = state
                                                    == gpui_kit::base::CheckboxState::Checked;
                                                cx.notify();
                                            });
                                        },
                                    ),
                                )
                                .child(
                                    intruder::resource_pools::pool_checkbox(
                                        "websocket-filter-negative",
                                        "反向搜索",
                                        self.draft.negative,
                                        enabled,
                                    )
                                    .on_change(
                                        move |state, _, _, cx| {
                                            let _ = negative.update(cx, |this, cx| {
                                                this.draft.negative = state
                                                    == gpui_kit::base::CheckboxState::Checked;
                                                cx.notify();
                                            });
                                        },
                                    ),
                                ),
                        ),
                )
                .child(
                    div()
                        .w(theme::scaled_px(180.))
                        .flex_shrink_0()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(4.))
                        .bg(theme::surface())
                        .p(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child("按注解筛选")
                        .child(
                            intruder::resource_pools::pool_checkbox(
                                "websocket-filter-notes",
                                "仅显示有备注的消息",
                                self.draft.only_with_notes,
                                enabled,
                            )
                            .on_change(
                                move |state, _, _, cx| {
                                    let _ = annotated.update(cx, |this, cx| {
                                        this.draft.only_with_notes =
                                            state == gpui_kit::base::CheckboxState::Checked;
                                        cx.notify();
                                    });
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .w(theme::scaled_px(115.))
                        .flex_shrink_0()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(4.))
                        .bg(theme::surface())
                        .p(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child("按监听器筛选")
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child("端口")
                                .child(Input::new(&self.port).small().disabled(!enabled)),
                        ),
                )
        };
        let content = div()
            .id("websocket-filter-dialog")
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(mode_bar)
            .child(body)
            .when_some(self.error.clone(), |this, error| {
                let summary = if error.contains('\n') {
                    format!(
                        "{}；{}",
                        error.lines().last().unwrap_or_default(),
                        error.lines().next().unwrap_or_default()
                    )
                } else {
                    error.clone()
                };
                this.child(
                    div()
                        .id("websocket-filter-error")
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Alert)
                        .aria_label(error.clone())
                        .min_h(theme::scaled_px(36.))
                        .max_h(px(72.))
                        .flex_shrink_0()
                        .line_height(gpui_kit::relative(1.3))
                        .overflow_y_scroll()
                        .text_color(theme::danger_fg())
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(error.clone())
                                .max_w(px(720.))
                                .build(window, cx)
                        })
                        .child(summary),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .flex_shrink_0()
                    .gap(px(8.))
                    .when(!script_mode, |row| {
                        row.child(
                            button("ws-filter-convert-to-script", "Convert to script")
                                .tooltip("将当前勾选的 WebSocket 筛选条件一键转换为 脚本并切换到 Script mode")
                                .disabled(!enabled)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.convert_to_script(window, cx)
                                })),
                        )
                    })
                    .child(
                        button("websocket-filter-all", "显示全部")
                            .icon(IconName::Eye)
                            .tooltip("重置所有筛选条件以显示全部 WebSocket 消息记录")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore(WebSocketHistoryFilter::default(), false, window, cx)
                            })),
                    )
                    .child(
                        button("websocket-filter-revert", "还原更改")
                            .icon(IconName::Undo2)
                            .tooltip("放弃当前窗口中未应用的草稿改动，恢复至上次已生效的筛选条件")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore(
                                    this.applied.clone(),
                                    this.applied_in_scope_only,
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        button("websocket-filter-cancel", "取消")
                            .icon(IconName::Close)
                            .tooltip("关闭筛选窗口并丢弃未应用的改动")
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        button("websocket-filter-apply", "应用")
                            .icon(IconName::Check)
                            .tooltip("应用当前筛选条件并立即刷新 WebSocket 历史列表")
                            .disabled(!enabled)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.apply(false, window, cx)),
                            ),
                    )
                    .child(
                        button("websocket-filter-apply-close", "应用并关闭")
                            .primary()
                            .icon(IconName::Check)
                            .tooltip("应用当前筛选条件刷新列表并关闭此窗口")
                            .disabled(!enabled)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.apply(true, window, cx)),
                            ),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(theme::ui_font_size())
            .bg(theme::background())
            .text_color(theme::foreground())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(content))
    }
}
