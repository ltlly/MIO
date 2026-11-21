use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    checkbox::Checkbox,
    input::Textarea,
    menu::{DropdownMenu, PopupMenuItem},
};
use mio_core::intruder::AttackType;

use super::{core_actions::AddIntruderPosition, *};

mod actions;
pub(super) mod onboarding;
mod settings;

const ATTACK_TYPES: [(AttackType, &str); 4] = [
    (AttackType::Sniper, "Sniper attack"),
    (AttackType::BatteringRam, "Battering ram attack"),
    (AttackType::Pitchfork, "Pitchfork attack"),
    (AttackType::ClusterBomb, "Cluster bomb attack"),
];

pub(in crate::workbench::intruder) fn attack_label(kind: AttackType) -> &'static str {
    ATTACK_TYPES
        .iter()
        .find(|(value, _)| *value == kind)
        .expect("all attack types have a label")
        .1
}

fn format_target_summary(raw: &str) -> Option<(bool, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = url::Url::parse(trimmed)
        && let Some(host) = parsed.host_str()
    {
        let https = parsed.scheme().eq_ignore_ascii_case("https");
        let port = parsed
            .port_or_known_default()
            .unwrap_or(if https { 443 } else { 80 });
        return Some((https, format!("{host}:{port}")));
    }
    Some((
        trimmed.starts_with("https://"),
        trimmed
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or(trimmed)
            .to_owned(),
    ))
}

pub(super) fn hex_length(text: &str) -> Option<usize> {
    let mut digits = 0_usize;
    for byte in text.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if !byte.is_ascii_hexdigit() {
            return None;
        }
        digits += 1;
    }
    digits.is_multiple_of(2).then_some(digits / 2)
}

impl Workbench {
    fn toggle_intruder_settings_sidebar(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(currently_open) = self
            .intruder
            .editors
            .get(&id)
            .map(|editors| editors.settings_open)
        else {
            return;
        };
        let other_sidebar_open = self
            .intruder
            .editors
            .get(&id)
            .is_some_and(|editors| editors.payloads_open || editors.resource_pool_open);
        if !currently_open
            && self.inspector.open
            && !other_sidebar_open
            && !self.dismiss_inspector_for_current_selection(window, cx)
        {
            return;
        }
        self.unfocus_intruder_sidebars(id, window, cx);
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.settings_open = !currently_open;
            if !currently_open {
                editors.payloads_open = false;
                editors.resource_pool_open = false;
            }
        }
        cx.notify();
    }

    fn unfocus_intruder_sidebars(&self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let payload_encode_focused = cx
            .try_global::<payloads::PayloadEncodingInputs>()
            .and_then(|inputs| inputs.0.get(&id))
            .is_some_and(|input| input.focus_handle(cx).is_focused(window));
        let sidebar_focused = self.intruder.editors.get(&id).is_some_and(|editors| {
            [
                &editors.payload_input,
                &editors.numbers_from,
                &editors.numbers_to,
                &editors.numbers_step,
                &editors.numbers_min_digits,
                &editors.grep_match_input,
                &editors.grep_extract_start,
                &editors.grep_extract_end,
            ]
            .into_iter()
            .any(|input| input.focus_handle(cx).is_focused(window))
                || editors
                    .pool_form
                    .inputs()
                    .iter()
                    .any(|input| input.focus_handle(cx).is_focused(window))
        });
        if payload_encode_focused || sidebar_focused {
            self.focus.focus(window, cx);
        }
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_intruder(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.workspace.intruder.active.and_then(|id| {
            let tab = self
                .workspace
                .intruder
                .tabs
                .iter()
                .find(|tab| tab.id == id)?;
            Some((id, &tab.spec, self.intruder.editors.get(&id)?))
        });
        let body = match active {
            Some((id, spec, editors)) => self.render_intruder_attack(id, spec, editors, cx),
            None => {
                if !self.intruder_onboarding_dismissed() {
                    self.render_intruder_onboarding(window, cx)
                } else {
                    self.render_intruder_dismissed_empty(cx)
                }
            }
        };
        self.render_intruder_tabs_workspace(body, window, cx)
    }

    #[inline(never)]
    fn render_intruder_attack(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editable = self.transition.is_none();
        let soft_wrap = editors.request.soft_wrap;
        let is_http2 = spec.draft.version == HttpVersion::Http2;
        let has_markers = editors.request.has_markers(cx);
        let positions_count = editors.request.display_count(cx);

        let top_action_bar = self.render_intruder_top_action_bar(id, spec, editors, editable, cx);
        let target_bar = self.render_intruder_target_bar(id, spec, editors, editable, cx);
        let positions_rail = self.render_intruder_positions_rail(id, editable, has_markers, cx);
        let editor_toolbar = self.render_intruder_editor_toolbar(
            id,
            editors,
            editable,
            is_http2,
            soft_wrap,
            positions_count.clone(),
            cx,
        );
        let search_bar = self.render_intruder_search_bar(id, editors, positions_count, cx);

        let request = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(top_action_bar)
            .child(target_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(theme::surface())
                    .flex()
                    .flex_col()
                    .child(editor_toolbar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_row()
                            .child(
                                div().flex_1().min_w_0().h_full().child(
                                    Textarea::new(editors.request.state())
                                        .size_full()
                                        .bordered(false)
                                        .readonly(!editable)
                                        .token(|_, _, _| {
                                            div()
                                                .px(px(2.))
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(theme::accent().opacity(0.45))
                                                .bg(theme::accent().opacity(0.14))
                                                .text_color(theme::accent())
                                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                .child("§")
                                        })
                                        .context_menu(self.intruder_positions_context_menu(
                                            editors.request.state().clone(),
                                            !editable,
                                        ))
                                        .aria_label("Intruder Positions 消息编辑器"),
                                ),
                            )
                            .child(positions_rail),
                    ),
            )
            .on_action(
                cx.listener(move |this, _: &AddIntruderPosition, window, cx| {
                    cx.stop_propagation();
                    this.add_intruder_position(id, window, cx);
                }),
            )
            .on_action(
                cx.listener(move |this, _: &crate::ClearIntruderPositions, window, cx| {
                    cx.stop_propagation();
                    if has_markers {
                        this.clear_intruder_positions(id, window, cx);
                    }
                }),
            )
            .child(search_bar)
            .when_some(editors.request.position_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .px(px(10.))
                        .py(px(4.))
                        .bg(theme::danger_bg())
                        .border_t_1()
                        .border_color(theme::danger_border())
                        .text_color(theme::danger_fg())
                        .child(error.clone()),
                )
            })
            .into_any_element();

        self.render_intruder_side_panel_and_rail(id, spec, editors, request, editable, cx)
    }

    #[inline(never)]
    fn render_intruder_top_action_bar(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.weak_entity();
        let selected = spec.attack_type;
        let attack_type = button("intruder-attack-type", attack_label(selected))
            .min_w(theme::scaled_px(180.))
            .dropdown_caret(true)
            .tooltip(
                "选择 Intruder 载荷组合模式 (Sniper / Battering ram / Pitchfork / Cluster bomb)",
            )
            .disabled(!editable)
            .dropdown_menu(move |menu, _, _| {
                ATTACK_TYPES.into_iter().fold(menu, |menu, (kind, label)| {
                    let weak = weak.clone();
                    menu.item(
                        PopupMenuItem::new(label)
                            .checked(kind == selected)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |view, cx| {
                                    view.change_intruder_spec(
                                        id,
                                        |spec| spec.attack_type = kind,
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    )
                })
            });
        let target_text = editors.target.read(cx).value();
        let target_summary = format_target_summary(target_text.as_ref());
        div()
            .w_full()
            .min_h(theme::scaled_px(42.).max(px(40.)))
            .px(px(12.))
            .py(px(6.))
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                Button::new("intruder-start")
                    .primary()
                    .small()
                    .h(theme::control_height())
                    .px(theme::scaled_px(12.))
                    .rounded(ButtonRounded::Size(px(3.)))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .icon(IconName::Play)
                    .label("开始攻击")
                    .tooltip("按当前攻击类型、标记位与载荷集启动 Intruder 攻击")
                    .disabled(
                        !editable
                            || self.backend_disconnected
                            || self.intruder.starts.contains_key(&id)
                            || self.pending_records > 0,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.start_intruder(window, cx))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::muted_foreground())
                            .child("Attack type"),
                    )
                    .child(attack_type),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .when_some(target_summary, |bar, (https, host_port)| {
                        bar.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .px(px(8.))
                                .h(theme::control_height())
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .bg(theme::surface())
                                .max_w(px(240.))
                                .child(
                                    gpui_kit::component::Icon::new(if https {
                                        IconName::ShieldCheck
                                    } else {
                                        IconName::Globe
                                    })
                                    .size(theme::scaled_px(12.))
                                    .text_color(if https {
                                        rgb(0x2e7d32).into()
                                    } else {
                                        theme::muted_foreground()
                                    }),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(theme::scaled_px(12.))
                                        .text_color(theme::foreground())
                                        .child(host_port),
                                ),
                        )
                    })
                    .child(
                        Button::new("intruder-header-settings")
                            .ghost()
                            .small()
                            .compact()
                            .w(theme::control_height())
                            .h(theme::control_height())
                            .p_0()
                            .rounded(ButtonRounded::Size(px(3.)))
                            .icon(
                                gpui_kit::component::Icon::new(IconName::Settings)
                                    .size(theme::scaled_px(14.)),
                            )
                            .selected(editors.settings_open)
                            .toggled(editors.settings_open)
                            .accessibility_label("Intruder 设置")
                            .tooltip("展开或收起 Intruder 攻击设置与 Grep 配置面板")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_intruder_settings_sidebar(id, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_target_bar(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w_full()
            .min_h(theme::scaled_px(36.).max(px(34.)))
            .px(px(12.))
            .py(px(5.))
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(theme::muted_foreground())
                    .child("目标"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.))
                    .child(Input::new(&editors.target).small().disabled(!editable)),
            )
            .child(
                Checkbox::new("intruder-update-host")
                    .label("更新 Host header 以匹配目标")
                    .tooltip("根据连接目标自动同步更新请求报文中的 Host 头字段")
                    .checked(spec.update_host_header)
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, value, window, cx| {
                        this.change_intruder_spec(
                            id,
                            |spec| spec.update_host_header = *value,
                            window,
                            cx,
                        );
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_positions_rail(
        &self,
        id: TabId,
        editable: bool,
        has_markers: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w(theme::scaled_px(108.).max(px(96.)))
            .flex_shrink_0()
            .h_full()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::toolbar_surface())
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .px(px(2.))
                    .pb(px(2.))
                    .text_size(theme::scaled_px(11.5))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(theme::muted_foreground())
                    .child("Positions"),
            )
            .child(
                button("intruder-position-add", "Add §")
                    .w_full()
                    .justify_start()
                    .icon(IconName::Plus)
                    .tooltip("在选中文本两侧插入 § 载荷标记（无选中时插入空标记对 §§）")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_intruder_position(id, window, cx)
                    })),
            )
            .child(
                button("intruder-position-clear", "Clear §")
                    .w_full()
                    .justify_start()
                    .icon(IconName::Eraser)
                    .tooltip("清除请求模板中的所有 § 载荷标记")
                    .disabled(!editable || !has_markers)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.clear_intruder_positions(id, window, cx)
                    })),
            )
            .child(
                button("intruder-position-auto", "Auto §")
                    .w_full()
                    .justify_start()
                    .icon(IconName::WandSparkles)
                    .tooltip("自动识别 URL 查询参数、表单字段与 Cookie 值并插入 § 标记")
                    .disabled(
                        !editable
                            || self.backend_disconnected
                            || self.intruder.positions_request.is_some(),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.auto_intruder_positions(id, window, cx)
                    })),
            )
            .child(div().my(px(4.)).h(px(1.)).bg(theme::border()))
            .child(
                div()
                    .px(px(2.))
                    .text_size(theme::scaled_px(11.))
                    .line_height(theme::scaled_px(16.))
                    .text_color(theme::muted_foreground())
                    .child("使用 § 标记需要替换的载荷位置"),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn render_intruder_editor_toolbar(
        &self,
        id: TabId,
        editors: &IntruderEditors,
        editable: bool,
        is_http2: bool,
        soft_wrap: bool,
        positions_count: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        toolbar()
            .children(
                [
                    (
                        MessageView::Raw,
                        "Raw",
                        IconName::Code,
                        "查看或编辑包含 § 载荷标记的原始请求模板文本",
                    ),
                    (
                        MessageView::Hex,
                        "Hex",
                        IconName::Binary,
                        "以十六进制视图查看或编辑请求字节与 § 载荷标记",
                    ),
                ]
                .into_iter()
                .map(|(view, label, icon, tip)| {
                    button(
                        SharedString::from(format!(
                            "intruder-view-{}",
                            label.to_ascii_lowercase()
                        )),
                        label,
                    )
                    .icon(icon)
                    .tooltip(tip)
                    .selected(editors.request.view == view)
                    .disabled(!editable)
                    .on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.switch_intruder_view(id, view, window, cx)
                        },
                    ))
                }),
            )
            .child(div().w(px(1.)).h(px(16.)).bg(theme::border()))
            .child(
                button(
                    "intruder-protocol-toggle",
                    if is_http2 { "HTTP/2" } else { "HTTP/1.1" },
                )
                .tooltip("切换当前攻击草稿发包使用的 HTTP/1.1 与 HTTP/2 协议版本")
                .selected(is_http2)
                .disabled(!editable)
                .on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.toggle_intruder_protocol(id, window, cx)
                    },
                )),
            )
            .child(
                Button::new("intruder-soft-wrap")
                    .small()
                    .h(theme::control_height())
                    .px(theme::scaled_px(7.))
                    .icon(IconName::TextWrap)
                    .accessibility_label("自动换行")
                    .selected(soft_wrap)
                    .toggled(soft_wrap)
                    .tooltip(if soft_wrap {
                        "自动换行已开启（点击切换为单行水平滚动）"
                    } else {
                        "自动换行已关闭（点击开启长行自动折行显示）"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_intruder_soft_wrap(id, window, cx)
                    })),
            )
            .child(
                div()
                    .ml_auto()
                    .px(px(7.))
                    .py(px(1.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::accent().opacity(0.35))
                    .bg(theme::accent().opacity(0.08))
                    .text_size(theme::scaled_px(11.5))
                    .text_color(theme::accent())
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(positions_count),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_search_bar(
        &self,
        id: TabId,
        editors: &IntruderEditors,
        positions_count: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor_state = editors.request.state().read(cx);
        let length = match editors.request.view {
            MessageView::Raw => editors.request.byte_length(cx).unwrap_or(0).to_string(),
            MessageView::Hex => editors
                .hex_length
                .map_or_else(|| "Hex 未完整".into(), |bytes| bytes.to_string()),
        };
        let matches = editor_state.search_session().matcher.len();
        let search_case_sensitive = editors.request.search_case_sensitive;
        let search_regex = editors.request.search_regex;
        let search_has_error = editors.request.search_error.is_some();
        div()
            .min_h(theme::scaled_px(34.).max(px(34.)))
            .py(px(4.))
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::toolbar_surface())
            .child(
                button("intruder-search-previous", "‹")
                    .icon(IconName::ChevronUp)
                    .tooltip("定位到上一个搜索匹配项")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search_intruder(id, Some(false), window, cx)
                    })),
            )
            .child(
                button("intruder-search-next", "›")
                    .icon(IconName::ChevronDown)
                    .tooltip("定位到下一个搜索匹配项")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search_intruder(id, Some(true), window, cx)
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&editors.search).small()),
            )
            .child(
                button("intruder-search-case", "Aa")
                    .tooltip("切换搜索是否区分大小写 (Case sensitive)")
                    .selected(search_case_sensitive)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_intruder_search_case(id, window, cx)
                    })),
            )
            .child(
                button("intruder-search-regex", ".*")
                    .tooltip("切换搜索是否启用正则表达式匹配 (Regex)")
                    .selected(search_regex)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_intruder_search_regex(id, window, cx)
                    })),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .text_color(if search_has_error {
                        theme::danger_fg()
                    } else {
                        theme::muted_foreground()
                    })
                    .child(if search_has_error {
                        "正则无效".to_owned()
                    } else {
                        format!("{matches} 个匹配")
                    }),
            )
            .child(
                div()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::accent())
                    .child(positions_count),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child(format!("长度：{length}")),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_side_panel_and_rail(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        request: AnyElement,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let any_intruder_sidebar =
            editors.payloads_open || editors.resource_pool_open || editors.settings_open;
        let show_inspector = self.inspector.open && !any_intruder_sidebar;
        let side_panel_left = self.intruder_user_misc().intruder_side_panel_left;
        let side_drawer: Option<AnyElement> = if show_inspector {
            Some(self.render_inspector_sidebar(cx))
        } else if editors.payloads_open {
            Some(
                div()
                    .w(px(420.))
                    .min_w(px(280.))
                    .max_w(px(480.))
                    .flex_shrink(1.0)
                    .h_full()
                    .bg(theme::surface())
                    .when(side_panel_left, |d| d.border_r_1())
                    .when(!side_panel_left, |d| d.border_l_1())
                    .border_color(theme::border())
                    .child(self.render_intruder_payloads(id, spec, editors, cx))
                    .into_any_element(),
            )
        } else if editors.resource_pool_open {
            Some(self.render_intruder_resource_pools(id, editors, cx))
        } else if editors.settings_open {
            Some(self.render_intruder_config_settings(id, spec, editors, editable, cx))
        } else {
            None
        };
        let side_rail = crate::workbench::wb_render_chrome::sidebar_rail_with_dock(side_panel_left)
            .child(
                sidebar_rail_tab(
                    "intruder-sidebar-payloads",
                    "载荷",
                    "载荷",
                    IconName::List,
                    editors.payloads_open,
                    false,
                    "展开或收起 Payloads 载荷集、生成器与处理规则面板",
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let Some(currently_open) = this
                        .intruder
                        .editors
                        .get(&id)
                        .map(|editors| editors.payloads_open)
                    else {
                        return;
                    };
                    let other_sidebar_open =
                        this.intruder.editors.get(&id).is_some_and(|editors| {
                            editors.resource_pool_open || editors.settings_open
                        });
                    if !currently_open
                        && this.inspector.open
                        && !other_sidebar_open
                        && !this.dismiss_inspector_for_current_selection(window, cx)
                    {
                        return;
                    }
                    this.unfocus_intruder_sidebars(id, window, cx);
                    if let Some(editors) = this.intruder.editors.get_mut(&id) {
                        editors.payloads_open = !currently_open;
                        if !currently_open {
                            editors.settings_open = false;
                            editors.resource_pool_open = false;
                        }
                    }
                    cx.notify();
                })),
            )
            .child(
                sidebar_rail_tab(
                    "intruder-sidebar-resource-pool",
                    "资源池",
                    "资源池",
                    IconName::SlidersHorizontal,
                    editors.resource_pool_open,
                    false,
                    "展开或收起 Resource pool 并发速率与自动节流配置面板",
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let Some(currently_open) = this
                        .intruder
                        .editors
                        .get(&id)
                        .map(|editors| editors.resource_pool_open)
                    else {
                        return;
                    };
                    let other_sidebar_open = this
                        .intruder
                        .editors
                        .get(&id)
                        .is_some_and(|editors| editors.payloads_open || editors.settings_open);
                    if !currently_open
                        && this.inspector.open
                        && !other_sidebar_open
                        && !this.dismiss_inspector_for_current_selection(window, cx)
                    {
                        return;
                    }
                    this.unfocus_intruder_sidebars(id, window, cx);
                    let mut opened = false;
                    if let Some(editors) = this.intruder.editors.get_mut(&id) {
                        editors.resource_pool_open = !currently_open;
                        if !currently_open {
                            editors.payloads_open = false;
                            editors.settings_open = false;
                            opened = true;
                        }
                    }
                    if opened {
                        this.query_intruder_resource_pools(cx);
                    }
                    cx.notify();
                })),
            )
            .child(
                sidebar_rail_tab(
                    "intruder-sidebar-settings",
                    "设置",
                    "设置",
                    IconName::Settings,
                    editors.settings_open,
                    false,
                    "展开或收起攻击保存策略、基准请求与 Grep 匹配提取面板",
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_intruder_settings_sidebar(id, window, cx);
                })),
            )
            .child(
                sidebar_rail_tab(
                    "inspector-toggle",
                    "Inspector",
                    "Inspector",
                    IconName::SlidersHorizontal,
                    show_inspector,
                    false,
                    if show_inspector {
                        "收起请求模板与选区编解码 Inspector 检视面板"
                    } else {
                        "展开请求模板与选区编解码 Inspector 检视面板"
                    },
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let any_sidebar_open = this.intruder.editors.get(&id).is_some_and(|editors| {
                        editors.payloads_open || editors.resource_pool_open || editors.settings_open
                    });
                    if any_sidebar_open {
                        if !this.open_inspector(window, cx) {
                            return;
                        }
                        this.unfocus_intruder_sidebars(id, window, cx);
                        if let Some(editors) = this.intruder.editors.get_mut(&id) {
                            editors.payloads_open = false;
                            editors.resource_pool_open = false;
                            editors.settings_open = false;
                        }
                        cx.notify();
                        return;
                    }
                    if this.inspector.open {
                        if !this.dismiss_inspector_for_current_selection(window, cx) {
                            return;
                        }
                    } else if !this.open_inspector(window, cx) {
                        return;
                    }
                    cx.notify();
                })),
            );
        if side_panel_left {
            div()
                .size_full()
                .flex()
                .child(side_rail)
                .children(side_drawer)
                .child(request)
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .child(request)
                .children(side_drawer)
                .child(side_rail)
                .into_any_element()
        }
    }
}
