use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator};
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::popover::Popover;

pub(super) fn search_bar_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
) -> Button {
    Button::new(id)
        .label(label)
        .small()
        .h(crate::theme::control_height())
        .px(crate::theme::scaled_px(8.))
}

fn search_option_checkbox(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    checked: bool,
    width: gpui_kit::Pixels,
    top: gpui_kit::Pixels,
) -> Checkbox {
    Checkbox::new(id)
        .accessibility_label(label)
        .checked(checked)
        .tab_stop(false)
        .absolute()
        .left(px(5.))
        .top(top)
        .w(width)
        .h(px(21.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(crate::theme::ui_font_size())
        .text_color(crate::theme::foreground())
        .child(
            CheckboxIndicator::new()
                .checked(checked)
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(if checked {
                    rgb(0x1558b0)
                } else {
                    rgb(if crate::theme::is_dark() {
                        0x5a5d62
                    } else {
                        0xb0afaf
                    })
                })
                .bg(if checked {
                    rgb(0x1558b0).into()
                } else {
                    crate::theme::surface()
                })
                .rounded(px(2.))
                .when(checked, |this| {
                    this.child(
                        gpui_kit::component::Icon::new(IconName::Check)
                            .size(px(11.))
                            .text_color(rgb(0xffffff)),
                    )
                }),
        )
        .child(div().whitespace_nowrap().child(label))
}

impl MessageEditor {
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        let state = self.state().clone();
        if state.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) || self.search.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) || self.replace.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            return Err(COMPOSITION_PENDING.into());
        }
        let selected = if state.focus_handle(cx).is_focused(window) {
            state.read(cx).selected_value()
        } else {
            Default::default()
        };
        self.search.update(cx, |input, cx| {
            if !selected.is_empty() && !selected.contains(['\r', '\n']) {
                input.set_value(selected, window, cx);
            }
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.update_search(None, window, cx)
    }

    pub fn update_search(
        &mut self,
        next: Option<bool>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let query = input_snapshot(&self.search, window, cx).map_err(str::to_owned)?;
        let (header_preview, non_printable, case_sensitive, regex, auto_scroll_to_match) = {
            let s = self.shared.borrow();
            (
                s.header_preview.clone(),
                s.non_printable.clone(),
                s.case_sensitive,
                s.regex,
                s.auto_scroll_to_match,
            )
        };
        let hex_grid = self.hex_grid();
        let active = self.state();
        for candidate in [
            header_preview.as_ref(),
            non_printable.as_ref(),
            Some(&self.raw),
            Some(&self.hex),
        ]
        .into_iter()
        .flatten()
        {
            if candidate.entity_id() != active.entity_id() {
                candidate.update(cx, |state, cx| state.close_search(cx));
            }
        }
        let case_insensitive = !case_sensitive;
        let result = active.update(cx, |state, cx| {
            if query.is_empty() {
                let _ = state.set_search_regex("", case_insensitive, regex, cx);
                state.close_search(cx);
                return Ok(None);
            }
            state
                .set_search_regex(query.to_string(), case_insensitive, regex, cx)
                .map_err(|error| format!("搜索正则表达式无效：{error}"))?;
            let revealed_range = match next {
                Some(true) => state.next_search_match(cx),
                Some(false) => state.previous_search_match(cx),
                None if auto_scroll_to_match => state.reveal_current_search_match(cx),
                None => None,
            };
            Ok::<_, String>(revealed_range)
        });
        match result {
            Ok(revealed_range) => {
                self.shared.borrow_mut().search_error = None;
                if active.entity_id() == self.hex.entity_id()
                    && let Some(grid) = hex_grid
                {
                    if let Some(match_range) = revealed_range {
                        grid.update(cx, |g, cx| g.scroll_to_text_range(match_range, cx));
                    } else if query.is_empty()
                        || active.read(cx).search_session().matcher.is_empty()
                    {
                        grid.update(cx, |g, cx| g.clear_search_match(cx));
                    }
                }
                Ok(())
            }
            Err(error) => {
                self.shared.borrow_mut().search_error = Some(error.clone());
                if active.entity_id() == self.hex.entity_id()
                    && let Some(grid) = hex_grid
                {
                    grid.update(cx, |g, cx| g.clear_search_match(cx));
                }
                Err(error)
            }
        }
    }

    pub fn toggle_case_sensitive(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let next = !self.case_sensitive();
        self.shared.borrow_mut().case_sensitive = next;
        self.pending_replace_all = None;
        if let Err(error) = self.update_search(None, window, cx) {
            if error == COMPOSITION_PENDING {
                self.shared.borrow_mut().case_sensitive = !next;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn toggle_regex(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        let next = !self.regex();
        self.shared.borrow_mut().regex = next;
        self.pending_replace_all = None;
        if let Err(error) = self.update_search(None, window, cx) {
            if error == COMPOSITION_PENDING {
                self.shared.borrow_mut().regex = !next;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn toggle_auto_scroll_to_match(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if self.has_any_composition(window, cx) {
            return Err(COMPOSITION_PENDING.into());
        }
        let next = !self.auto_scroll_to_match();
        self.shared.borrow_mut().auto_scroll_to_match = next;
        if next && let Err(error) = self.update_search(None, window, cx) {
            if error == COMPOSITION_PENDING {
                self.shared.borrow_mut().auto_scroll_to_match = !next;
            }
            return Err(error);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn search_match_count(&self, cx: &App) -> usize {
        self.state().read(cx).search_session().matcher.len()
    }

    pub fn search_status_label(&self, cx: &App) -> String {
        if self.has_search_error() {
            return "正则无效".into();
        }
        let active = self.state();
        let session = active.read(cx).search_session();
        if session.query.is_empty() {
            return "0 个匹配".into();
        }
        match session.matcher.current() {
            Some(ix) => format!("{}/{} 个匹配", ix + 1, session.matcher.len()),
            None => "0 个匹配".into(),
        }
    }

    pub fn handle_search_bar_action(
        &mut self,
        action: SearchBarAction,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        self.handle_search_bar_action_editable(action, true, window, cx)
    }

    pub fn handle_search_bar_action_editable(
        &mut self,
        action: SearchBarAction,
        readonly: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        match action {
            SearchBarAction::Previous => self.update_search(Some(false), window, cx),
            SearchBarAction::Next => self.update_search(Some(true), window, cx),
            SearchBarAction::ToggleCaseSensitive => self.toggle_case_sensitive(window, cx),
            SearchBarAction::ToggleRegex => self.toggle_regex(window, cx),
            SearchBarAction::ToggleAutoScroll => self.toggle_auto_scroll_to_match(window, cx),
            SearchBarAction::ToggleReplace => self.toggle_replace(readonly, window, cx),
            SearchBarAction::Replace => self.replace_current(readonly, window, cx),
            SearchBarAction::RequestReplaceAll => self.request_replace_all(readonly, window, cx),
            SearchBarAction::ConfirmReplaceAll => self.confirm_replace_all(readonly, window, cx),
            SearchBarAction::CancelReplaceAll => {
                self.cancel_replace_all();
                Ok(())
            }
        }
    }

    pub fn render_search_bar<V: 'static>(
        &self,
        id: &'static str,
        cx: &mut gpui_kit::Context<V>,
        on_action: impl Fn(&mut V, SearchBarAction, &mut Window, &mut gpui_kit::Context<V>)
        + Clone
        + 'static,
    ) -> AnyElement {
        self.render_search_bar_editable(id, true, cx, on_action)
    }

    #[inline(never)]
    fn render_search_help_popover(&self, id: &'static str) -> AnyElement {
        div()
            .mt(px(11.))
            .w(px(22.))
            .h(px(22.))
            .child(
                Popover::new(SharedString::from(format!("{id}-search-help-popover")))
                    .anchor(gpui_kit::Anchor::BottomLeft)
                    .trigger(
                        Button::new(format!("{id}-search-help"))
                            .ghost()
                            .small()
                            .compact()
                            .rounded(ButtonRounded::None)
                            .w(px(22.))
                            .h(px(22.))
                            .p_0()
                            .border_0()
                            .bg(gpui_kit::transparent_black())
                            .text_color(crate::theme::foreground())
                            .child(
                                gpui_kit::component::Icon::new(IconName::CircleQuestionMark)
                                    .size(px(17.))
                                    .text_color(crate::theme::foreground()),
                            )
                            .accessibility_label("搜索帮助")
                            .tooltip("查看消息编辑器搜索与替换使用说明"),
                    )
                    .content(move |_, _, _| {
                        div()
                            .id(SharedString::from(format!("{id}-search-help-content")))
                            .test_support()
                            .w(px(264.))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .text_size(px(12.))
                            .text_color(crate::theme::foreground())
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .child("消息搜索与替换帮助"),
                            )
                            .child("• Enter / →：跳转到下一个匹配项")
                            .child("• Shift+Enter / ←：跳转到上一个匹配项")
                            .child("• 齿轮菜单：开启区分大小写、正则表达式或展开查找替换栏")
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_search_options_popover<V: 'static>(
        &self,
        id: &'static str,
        can_show_replace: bool,
        show_replace_row: bool,
        field_bg: gpui_kit::Hsla,
        cx: &mut gpui_kit::Context<V>,
        on_action: impl Fn(&mut V, SearchBarAction, &mut Window, &mut gpui_kit::Context<V>)
        + Clone
        + 'static,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        let (case_sensitive, regex_active, auto_scroll) = {
            let s = self.shared.borrow();
            (s.case_sensitive, s.regex, s.auto_scroll_to_match)
        };
        let replace_open = self.replace_open;
        let case_action = on_action.clone();
        let regex_action = on_action.clone();
        let auto_scroll_action = on_action.clone();
        let toggle_replace_action = on_action;
        div()
            .ml(px(5.))
            .mt(px(11.))
            .w(px(22.))
            .h(px(22.))
            .child(
                Popover::new(SharedString::from(format!("{id}-search-options-popover")))
                    .anchor(gpui_kit::Anchor::BottomLeft)
                    .offset(px(0.))
                    .p_0()
                    .bg(field_bg)
                    .trigger(
                        Button::new(format!("{id}-search-options"))
                            .ghost()
                            .small()
                            .compact()
                            .rounded(ButtonRounded::None)
                            .w(px(22.))
                            .h(px(22.))
                            .p_0()
                            .border_0()
                            .bg(gpui_kit::transparent_black())
                            .text_color(crate::theme::foreground())
                            .child(
                                gpui_kit::component::Icon::new(IconName::Settings)
                                    .size(px(17.))
                                    .text_color(crate::theme::foreground()),
                            )
                            .accessibility_label("搜索选项")
                            .tooltip("搜索与替换选项（区分大小写、正则表达式、展开替换）")
                            .selected(
                                case_sensitive
                                    || regex_active
                                    || auto_scroll
                                    || show_replace_row,
                            ),
                    )
                    .content(move |_, _, _| {
                        let owner_case = owner.clone();
                        let owner_regex = owner.clone();
                        let owner_autoscroll = owner.clone();
                        let owner_replace = owner.clone();
                        let case_action = case_action.clone();
                        let regex_action = regex_action.clone();
                        let auto_scroll_action = auto_scroll_action.clone();
                        let toggle_replace_action = toggle_replace_action.clone();
                        div()
                            .id(SharedString::from(format!("{id}-search-options-menu")))
                            .test_support()
                            .w(px(272.))
                            .p_0()
                            .bg(field_bg)
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .relative()
                                    .w(px(270.))
                                    .h(px(83.))
                                    .ml(px(1.))
                                    .mt(px(4.))
                                    .mb(px(4.))
                                    .child(
                                        search_option_checkbox(
                                            format!("{id}-search-case"),
                                            "Case sensitive",
                                            case_sensitive,
                                            px(115.),
                                            px(5.),
                                        )
                                        .on_change(move |_, _, window, cx| {
                                            let _ = owner_case.update(cx, |this, cx| {
                                                case_action(
                                                    this,
                                                    SearchBarAction::ToggleCaseSensitive,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .child(
                                        search_option_checkbox(
                                            format!("{id}-search-regex"),
                                            "Regex",
                                            regex_active,
                                            px(66.),
                                            px(31.),
                                        )
                                        .on_change(move |_, _, window, cx| {
                                            let _ = owner_regex.update(cx, |this, cx| {
                                                regex_action(
                                                    this,
                                                    SearchBarAction::ToggleRegex,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                    )
                                    .child(
                                        search_option_checkbox(
                                            format!("{id}-search-autoscroll"),
                                            "Auto-scroll to match when text changes",
                                            auto_scroll,
                                            px(260.),
                                            px(57.),
                                        )
                                        .on_change(move |_, _, window, cx| {
                                            let _ = owner_autoscroll.update(cx, |this, cx| {
                                                auto_scroll_action(
                                                    this,
                                                    SearchBarAction::ToggleAutoScroll,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                    ),
                            )
                            .when(can_show_replace, |menu| {
                                menu.child(
                                    div()
                                        .border_t_1()
                                        .border_color(crate::theme::border())
                                        .px(px(5.))
                                        .py(px(3.))
                                        .child(
                                            Popover::new(SharedString::from(format!(
                                                "{id}-search-replace-submenu"
                                            )))
                                            .anchor(gpui_kit::Anchor::RightCenter)
                                            .default_open(true)
                                            .overlay_closable(false)
                                            .p(px(5.))
                                            .bg(field_bg)
                                            .trigger(
                                                Button::new(format!(
                                                    "{id}-search-replace-submenu-trigger"
                                                ))
                                                .ghost()
                                                .small()
                                                .w_full()
                                                .h(px(21.))
                                                .px(px(4.))
                                                .icon(IconName::Replace)
                                                .label("Replacement ▸")
                                                .selected(replace_open),
                                            )
                                            .content(move |_, _, _| {
                                                let owner_replace = owner_replace.clone();
                                                let toggle_replace_action =
                                                    toggle_replace_action.clone();
                                                div()
                                                    .w(px(180.))
                                                    .flex()
                                                    .flex_col()
                                                    .child(
                                                        search_option_checkbox(
                                                            format!(
                                                                "{id}-search-replace-toggle"
                                                            ),
                                                            "Show replace bar",
                                                            replace_open,
                                                            px(170.),
                                                            px(0.),
                                                        )
                                                        .relative()
                                                        .left(px(0.))
                                                        .top(px(0.))
                                                        .on_change(move |_, _, window, cx| {
                                                            let _ = owner_replace.update(
                                                                cx,
                                                                |this, cx| {
                                                                    toggle_replace_action(
                                                                        this,
                                                                        SearchBarAction::ToggleReplace,
                                                                        window,
                                                                        cx,
                                                                    );
                                                                },
                                                            );
                                                        }),
                                                    )
                                            }),
                                        ),
                                )
                            })
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_search_nav_button<V: 'static>(
        button_id: String,
        icon: IconName,
        a11y_label: &'static str,
        tip: &'static str,
        action: SearchBarAction,
        nav_bg: gpui_kit::Hsla,
        nav_border: gpui_kit::Hsla,
        cx: &mut gpui_kit::Context<V>,
        on_action: impl Fn(&mut V, SearchBarAction, &mut Window, &mut gpui_kit::Context<V>) + 'static,
    ) -> AnyElement {
        Button::new(button_id)
            .ghost()
            .small()
            .compact()
            .rounded(ButtonRounded::Size(px(3.)))
            .ml(px(5.))
            .mt(px(10.))
            .w(px(24.))
            .h(px(24.))
            .p_0()
            .bg(nav_bg)
            .border_1()
            .border_color(nav_border)
            .text_color(crate::theme::foreground())
            .child(
                gpui_kit::component::Icon::new(icon)
                    .size(px(14.))
                    .text_color(crate::theme::foreground()),
            )
            .accessibility_label(a11y_label)
            .tooltip(tip)
            .on_click(cx.listener(move |this, _, window, cx| {
                on_action(this, action, window, cx);
            }))
            .into_any_element()
    }

    #[inline(never)]
    fn render_search_input_box(
        &self,
        id: &'static str,
        field_bg: gpui_kit::Hsla,
        field_border: gpui_kit::Hsla,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("{id}-search-input")))
            .test_support()
            .ml(px(10.))
            .mt(px(11.))
            .flex_1()
            .min_w(crate::theme::scaled_px(80.))
            .h(px(23.))
            .rounded(px(3.))
            .border_1()
            .border_color(field_border)
            .bg(field_bg)
            .overflow_hidden()
            .child(
                Input::new(&self.search)
                    .small()
                    .appearance(false)
                    .h(px(21.))
                    .px(px(6.))
                    .bg(field_bg)
                    .suffix(
                        gpui_kit::component::Icon::new(IconName::Search)
                            .size(px(14.))
                            .text_color(crate::theme::foreground()),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_search_status_label(
        &self,
        id: &'static str,
        status: String,
        has_error: bool,
    ) -> AnyElement {
        let status: SharedString = status.into();
        div()
            .id(SharedString::from(format!("{id}-search-matches")))
            .test_support()
            .ml(px(15.))
            .mt(px(9.))
            .w(px(80.))
            .flex_shrink_0()
            .h(px(27.))
            .flex()
            .items_center()
            .justify_start()
            .text_left()
            .role(gpui_kit::accesskit::Role::Status)
            .aria_label(status.clone())
            .text_color(if has_error {
                crate::theme::danger_fg()
            } else {
                crate::theme::foreground()
            })
            .child(status)
            .into_any_element()
    }

    #[inline(never)]
    pub fn render_search_bar_editable<V: 'static>(
        &self,
        id: &'static str,
        readonly: bool,
        cx: &mut gpui_kit::Context<V>,
        on_action: impl Fn(&mut V, SearchBarAction, &mut Window, &mut gpui_kit::Context<V>)
        + Clone
        + 'static,
    ) -> AnyElement {
        let prev_action = on_action.clone();
        let next_action = on_action.clone();
        let has_error = self.has_search_error();
        let status = self.search_status_label(cx);
        let selected_len = Some(self.selected_byte_len(cx)).filter(|&n| n > 0);
        let can_show_replace = !readonly && !self.is_readonly_preview();
        let show_replace_row = can_show_replace && self.replace_open;
        let dark = crate::theme::is_dark();
        let nav_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xeeeeee }).into();
        let nav_border: gpui_kit::Hsla = rgb(if dark { 0x4b4f56 } else { 0xd4d4d4 }).into();
        let field_bg: gpui_kit::Hsla =
            rgb(if dark { 0x1e1f22 } else { 0xffffff }).into();
        let field_border: gpui_kit::Hsla =
            rgb(if dark { 0x4e5157 } else { 0xc5c5c5 }).into();
        let search_row = div()
            .relative()
            .h(px(46.))
            .flex_shrink_0()
            .flex()
            .pl(px(5.))
            .pr(px(5.))
            .child(self.render_search_help_popover(id))
            .child(self.render_search_options_popover(
                id,
                can_show_replace,
                show_replace_row,
                field_bg,
                cx,
                on_action.clone(),
            ))
            .child(Self::render_search_nav_button(
                format!("{id}-search-prev"),
                IconName::ArrowLeft,
                "上一匹配",
                "跳转到上一个搜索匹配项（Shift+Enter）",
                SearchBarAction::Previous,
                nav_bg,
                nav_border,
                cx,
                prev_action,
            ))
            .child(Self::render_search_nav_button(
                format!("{id}-search-next"),
                IconName::ArrowRight,
                "下一匹配",
                "跳转到下一个搜索匹配项（Enter）",
                SearchBarAction::Next,
                nav_bg,
                nav_border,
                cx,
                next_action,
            ))
            .child(self.render_search_input_box(id, field_bg, field_border))
            .child(self.render_search_status_label(id, status, has_error))
            .when_some(selected_len, |row, n| {
                let selection_label = format!("Selection: {n} (0x{n:x})");
                row.child(
                    div()
                        .ml(px(6.))
                        .mt(px(14.))
                        .w(px(1.))
                        .h(px(17.))
                        .flex_shrink_0()
                        .bg(crate::theme::border()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-search-selection")))
                        .test_support()
                        .ml(px(8.))
                        .mt(px(9.))
                        .h(px(27.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .whitespace_nowrap()
                        .text_size(px(13.))
                        .text_color(crate::theme::foreground())
                        .aria_label(selection_label.clone())
                        .child(selection_label),
                )
            });

        div()
            .id(SharedString::from(format!("{id}-search-bar")))
            .test_support()
            .min_h(px(47.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(if crate::theme::is_dark() {
                crate::theme::border()
            } else {
                rgb(0xefefef).into()
            })
            .bg(crate::theme::surface())
            .child(search_row)
            .when(show_replace_row, |container| {
                container.child(self.render_replace_row(id, cx, on_action))
            })
            .into_any_element()
    }
}
