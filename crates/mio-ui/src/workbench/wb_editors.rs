use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use mio_core::RepeaterLayoutMode;

struct CachedRenderedResponse {
    status_line: String,
    status_code: Option<u16>,
    content_type: Option<String>,
    location: Option<String>,
    body_len: usize,
    summary: editing::RenderedBodySummary,
    body_for_browser: std::sync::Arc<Vec<u8>>,
}

type ProvenanceMenuCacheKey = (
    gpui_kit::EntityId,
    TabId,
    FlowId,
    u64,
    bool,
    usize,
    bool,
    crate::workbench::inspector::MessageEditorRevKey,
);

type ProvenanceMenuCacheVal = Option<(std::sync::Arc<Vec<(String, Vec<u8>)>>, usize)>;

std::thread_local! {
    static RENDERED_RESPONSE_CACHE: std::cell::RefCell<
        Vec<(
            crate::workbench::inspector::MessageEditorRevKey,
            Option<std::rc::Rc<CachedRenderedResponse>>,
        )>,
    > = const { std::cell::RefCell::new(Vec::new()) };
    static PROVENANCE_MENU_CACHE: std::cell::RefCell<
        Vec<(ProvenanceMenuCacheKey, ProvenanceMenuCacheVal)>,
    > = const { std::cell::RefCell::new(Vec::new()) };
}

fn owner_for_editor_id(id: &'static str) -> Option<&'static str> {
    match id {
        "repeater-request" | "repeater-response" => Some("repeater"),
        "history-request" | "history-response" => Some("history"),
        "target-request" | "target-response" => Some("target"),
        "logger-request" | "logger-response" => Some("logger"),
        "organiser-request" | "organiser-response" => Some("organiser"),
        "dashboard-issue-request" | "dashboard-issue-response" => Some("dashboard"),
        _ => None,
    }
}

fn is_request_editor_id(id: &str) -> bool {
    matches!(
        id,
        "repeater-request"
            | "history-request"
            | "target-request"
            | "logger-request"
            | "organiser-request"
            | "dashboard-issue-request"
    )
}

fn is_response_editor_id(id: &str) -> bool {
    matches!(
        id,
        "repeater-response"
            | "history-response"
            | "target-response"
            | "logger-response"
            | "organiser-response"
            | "dashboard-issue-response"
    )
}

fn message_view_tab(id: SharedString, label: &'static str, width: f32, selected: bool) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .rounded(ButtonRounded::None)
        .relative()
        .self_start()
        .w(px(width))
        .h(px(37.))
        .p_0()
        .bg(theme::surface())
        .text_color(if selected {
            if theme::is_dark() { theme::foreground() } else { rgb(0x000000).into() }
        } else if theme::is_dark() {
            theme::muted_foreground()
        } else {
            rgb(0x1f1f1f).into()
        })
        .selected(selected)
        .toggled(selected)
        .accessibility_label(label)
        .child(
            div()
                .text_size(px(13.))
                .font_weight(if selected {
                    gpui_kit::FontWeight::BOLD
                } else {
                    gpui_kit::FontWeight::NORMAL
                })
                .child(label),
        )
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(3.))
                    .bg(rgb(0xdb612f)),
            )
        })
}

pub(in crate::workbench) fn message_version_trigger(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    tooltip: &'static str,
) -> Button {
    let label: SharedString = label.into();
    let title_fg: gpui_kit::Hsla = if theme::is_dark() {
        theme::foreground()
    } else {
        rgb(0x000000).into()
    };
    Button::new(id)
        .ghost()
        .small()
        .compact()
        .rounded(ButtonRounded::None)
        .h(px(23.))
        .px_0()
        .py_0()
        .border_0()
        .bg(gpui_kit::transparent_black())
        .text_color(title_fg)
        .accessibility_label(label.clone())
        .tooltip(tooltip)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(
                    div()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(15.))
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(title_fg)
                        .child(label),
                )
                .child(
                    gpui_kit::component::Icon::default()
                        .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 12 12\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.9\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.2 4.1L6 7.9L9.8 4.1\"/></svg>")
                        .size(px(11.))
                        .text_color(title_fg),
                ),
        )
}

fn combined_message_tab(id: SharedString, label: &'static str, width: f32, selected: bool) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .rounded(ButtonRounded::None)
        .relative()
        .self_start()
        .w(px(width))
        .h(px(39.))
        .p_0()
        .bg(theme::surface())
        .text_color(if selected {
            if theme::is_dark() { theme::foreground() } else { rgb(0x000000).into() }
        } else if theme::is_dark() {
            theme::muted_foreground()
        } else {
            rgb(0x2b2b2b).into()
        })
        .selected(selected)
        .toggled(selected)
        .tab_stop(false)
        .accessibility_label(label)
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui_kit::FontWeight::BOLD)
                .child(label),
        )
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(3.))
                    .bg(rgb(0xdb612f)),
            )
        })
}

impl Workbench {
    pub(in crate::workbench) fn message_pair_layout_for_owner(
        &self,
        owner: &'static str,
    ) -> RepeaterLayoutMode {
        let key = if owner == "imported-history" {
            "history"
        } else {
            owner
        };
        self.message_pair_layouts
            .get(key)
            .copied()
            .unwrap_or(RepeaterLayoutMode::SideBySide)
    }

    pub(in crate::workbench) fn set_message_pair_layout_for_owner(
        &mut self,
        owner: &'static str,
        mode: RepeaterLayoutMode,
    ) {
        let key = if owner == "imported-history" {
            "history"
        } else {
            owner
        };
        self.message_pair_layouts.insert(key, mode);
    }

    fn combined_pane_for_owner(&self, owner: &'static str) -> super::tabs::RepeaterCombinedPane {
        let key = if owner == "imported-history" {
            "history"
        } else {
            owner
        };
        self.message_pair_combined_panes
            .get(key)
            .copied()
            .unwrap_or(super::tabs::RepeaterCombinedPane::Request)
    }

    fn set_combined_pane_for_owner(
        &mut self,
        owner: &'static str,
        pane: super::tabs::RepeaterCombinedPane,
    ) {
        let key = if owner == "imported-history" {
            "history"
        } else {
            owner
        };
        self.message_pair_combined_panes.insert(key, pane);
    }

    pub(in crate::workbench) fn select_combined_pane_for_owner(
        &mut self,
        owner: &'static str,
        pane: super::tabs::RepeaterCombinedPane,
        cx: &mut Context<Self>,
    ) {
        if owner == "repeater" {
            if let Some(tab_id) = self.workspace.active
                && let Some(ed) = self.tab_editors.get(&tab_id)
            {
                ed.combined_pane.set(pane);
            }
        } else {
            self.set_combined_pane_for_owner(owner, pane);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn message_header(
        &self,
        id: &'static str,
        title: &'static str,
        view: MessageView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pretty = self.is_message_pretty(id);
        let hide_headers = self.is_message_hide_headers(id);
        let non_printable = self.is_message_non_printable(id);
        let render_active = self.is_message_render(id);
        let soft_wrap = self.is_message_soft_wrap(id);
        let raw_selected = !pretty && !render_active && view == MessageView::Raw;
        let hex_selected = !pretty && !render_active && view == MessageView::Hex;
        let provenance = self
            .render_message_provenance_menu(id, cx)
            .or_else(|| self.render_repeater_provenance_menu(id, cx))
            .or_else(|| self.render_target_provenance_menu(id, cx));
        let title = if is_response_editor_id(id) {
            "Response"
        } else if is_request_editor_id(id) {
            "Request"
        } else {
            title
        };
        let editor_owner = owner_for_editor_id(id);
        let current_layout = editor_owner.map(|owner| {
            if owner == "repeater" {
                self.workspace.repeater_settings.layout
            } else {
                self.message_pair_layout_for_owner(owner)
            }
        });
        let is_combined = current_layout == Some(RepeaterLayoutMode::Combined);
        let is_top_bottom = current_layout == Some(RepeaterLayoutMode::TopBottom);
        let layout_owner = editor_owner.filter(|_| {
            if is_combined {
                true
            } else if is_top_bottom {
                is_request_editor_id(id)
            } else {
                is_response_editor_id(id)
            }
        });
        let is_target_editor = id == "target-request" || id == "target-response";
        let compact_header = is_target_editor && !is_combined;
        let narrow_view_tabs = is_target_editor && self.inspector.open && !is_combined && !is_top_bottom;
        let row1_h = if compact_header { px(30.) } else { px(39.) };
        let row2_h = if compact_header { px(30.) } else { px(38.) };
        let header_h = if compact_header { px(60.) } else { px(77.) };
        let tab_h = if compact_header { px(29.) } else { px(37.) };
        let overflow_active = hex_selected || (is_response_editor_id(id) && render_active);
        let overflow_menu_owner = cx.weak_entity();
        div()
            .w_full()
            .h(header_h)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                div()
                    .w_full()
                    .h(row1_h)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .when(!is_combined, |row| row.pl(px(15.)).gap(px(8.)))
                    .pr(px(10.))
                    .border_b_1()
                    .border_color(if is_combined {
                        theme::border()
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .map(|row| {
                        if let Some(owner) = editor_owner.filter(|_| is_combined) {
                            let active_pane = if owner == "repeater" {
                                self.workspace
                                    .active
                                    .and_then(|tab_id| self.tab_editors.get(&tab_id))
                                    .map_or(super::tabs::RepeaterCombinedPane::Request, |ed| {
                                        ed.combined_pane.get()
                                    })
                            } else {
                                self.combined_pane_for_owner(owner)
                            };
                            row.child(
                                combined_message_tab(
                                    SharedString::from(format!("{owner}-combined-req")),
                                    "Request",
                                    90.,
                                    active_pane == super::tabs::RepeaterCombinedPane::Request,
                                )
                                .tooltip("切换显示 HTTP 请求报文视图")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_combined_pane_for_owner(
                                        owner,
                                        super::tabs::RepeaterCombinedPane::Request,
                                        cx,
                                    );
                                })),
                            )
                            .child(
                                combined_message_tab(
                                    SharedString::from(format!("{owner}-combined-resp")),
                                    "Response",
                                    102.,
                                    active_pane == super::tabs::RepeaterCombinedPane::Response,
                                )
                                .tooltip("切换显示 HTTP 响应报文视图")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_combined_pane_for_owner(
                                        owner,
                                        super::tabs::RepeaterCombinedPane::Response,
                                        cx,
                                    );
                                })),
                            )
                            .when_some(provenance, |row, menu| {
                                row.child(div().ml(px(8.)).child(menu))
                            })
                        } else if let Some(menu) = provenance {
                            row.child(menu)
                        } else {
                            row.child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_color(if theme::is_dark() {
                                        theme::foreground()
                                    } else {
                                        rgb(0x000000).into()
                                    })
                                    .child(title),
                            )
                        }
                    })
                    .when_some(layout_owner, |row, owner| {
                        row.child(
                            div()
                                .ml_auto()
                                .child(self.render_message_pair_layout_controls(owner, cx)),
                        )
                    }),
            )
            .child(
                div()
                    .w_full()
                    .h(row2_h)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        message_view_tab(
                            format!("{id}-pretty").into(),
                            "Pretty",
                            if narrow_view_tabs { 56. } else { 64. },
                            pretty && !render_active,
                        )
                        .h(tab_h)
                        .tooltip("格式化视图（自动美化 JSON / XML / HTML / 表单，可编辑模式下支持实时编辑）")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.switch_pretty_view(id, window, cx)
                        })),
                    )
                    .child(
                        message_view_tab(
                            format!("{id}-raw").into(),
                            "Raw",
                            if narrow_view_tabs { 48. } else { 56. },
                            raw_selected,
                        )
                        .h(tab_h)
                        .tooltip("原始字节流文本视图（按字节展示完整报文，可编辑模式下支持直接修改）")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.switch_message_view(id, MessageView::Raw, window, cx)
                        })),
                    )
                    .when(!narrow_view_tabs, |group| {
                        group
                            .child(
                                message_view_tab(
                                    format!("{id}-hex").into(),
                                    "Hex",
                                    53.,
                                    hex_selected,
                                )
                                .h(tab_h)
                                .tooltip("十六进制字节编辑器视图（按精确字节偏移查看与修改载荷）")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.switch_message_view(id, MessageView::Hex, window, cx)
                                })),
                            )
                            .when(is_response_editor_id(id), |group| {
                                group.child(
                                    message_view_tab(
                                        format!("{id}-render").into(),
                                        "Render",
                                        72.,
                                        render_active,
                                    )
                                    .h(tab_h)
                                    .tooltip("安全离线页面渲染预览（提取 HTML 结构、超链接、表单控件与图片元数据，绝不发起外部请求）")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.switch_render_view(id, window, cx)
                                    })),
                                )
                            })
                    })
                    .when(narrow_view_tabs, |group| {
                        let is_resp = is_response_editor_id(id);
                        group.child(
                            Button::new(SharedString::from(format!("{id}-view-overflow")))
                                .ghost()
                                .small()
                                .rounded(ButtonRounded::None)
                                .relative()
                                .self_start()
                                .w(px(26.))
                                .h(tab_h)
                                .p_0()
                                .bg(theme::surface())
                                .text_color(theme::foreground())
                                .accessibility_label("更多视图")
                                .tooltip("切换 Hex / Render 视图")
                                .child(
                                    gpui_kit::component::Icon::new(IconName::ChevronDown)
                                        .size(px(12.))
                                        .text_color(theme::foreground()),
                                )
                                .when(overflow_active, |tab| {
                                    tab.child(
                                        div()
                                            .absolute()
                                            .bottom_0()
                                            .left_0()
                                            .right_0()
                                            .h(px(3.))
                                            .bg(rgb(0xdb612f)),
                                    )
                                })
                                .dropdown_menu(move |menu: PopupMenu, _, _| {
                                    let owner_hex = overflow_menu_owner.clone();
                                    let owner_render = overflow_menu_owner.clone();
                                    menu.min_w(px(110.))
                                        .item(
                                            PopupMenuItem::new("Hex")
                                                .checked(hex_selected)
                                                .on_click(move |_, window, cx| {
                                                    let _ = owner_hex.update(cx, |this, cx| {
                                                        this.switch_message_view(
                                                            id,
                                                            MessageView::Hex,
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        )
                                        .when(is_resp, |m| {
                                            m.item(
                                                PopupMenuItem::new("Render")
                                                    .checked(render_active)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = owner_render.update(cx, |this, cx| {
                                                            this.switch_render_view(id, window, cx);
                                                        });
                                                    }),
                                            )
                                        })
                                }),
                        )
                    })
                    .when(id != "ext-message", |row| {
                        let unavailable = self.project.is_none()
                            || self.transition.is_some()
                            || self.backend_disconnected;
                        let menu_owner = cx.weak_entity();
                        row.child(
                            div()
                                .ml_auto()
                                .pr(px(10.))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(
                                    Button::new(SharedString::from(format!("{id}-hide-headers")))
                                        .ghost()
                                        .small()
                                        .rounded(ButtonRounded::None)
                                        .w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .bg(if hide_headers { rgb(0x26649d).into() } else { gpui_kit::transparent_black() })
                                        .text_color(if hide_headers { rgb(0xffffff).into() } else { theme::foreground() })
                                        .child(
                                            gpui_kit::component::Icon::default()
                                                .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.45\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.2 2.2l11.6 11.6\"/><path d=\"M6.6 6.7a2 2 0 0 0 2.7 2.7\"/><path d=\"M4.1 4.4C2.5 5.4 1.5 7 1.2 8c.9 2.4 3.5 4.3 6.8 4.3 1.2 0 2.3-.3 3.3-.8\"/><path d=\"M6.8 3.8c.4-.1.8-.1 1.2-.1 3.3 0 5.9 1.9 6.8 4.3-.4 1-.9 1.9-1.7 2.6\"/></svg>")
                                                .size(px(16.))
                                                .text_color(if hide_headers { rgb(0xffffff).into() } else { theme::foreground() }),
                                        )
                                        .accessibility_label("Hide specified headers")
                                        .selected(hide_headers)
                                        .toggled(hide_headers)
                                        .tooltip(if hide_headers {
                                            "Show specified headers"
                                        } else {
                                            "Hide specified headers"
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.toggle_message_hide_headers(id, window, cx)
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("{id}-wrap")))
                                        .ghost()
                                        .small()
                                        .rounded(ButtonRounded::None)
                                        .w(px(19.))
                                        .h(px(19.))
                                        .p_0()
                                        .bg(if soft_wrap { rgb(0x26649d).into() } else { gpui_kit::transparent_black() })
                                        .text_color(if soft_wrap { rgb(0xffffff).into() } else { theme::foreground() })
                                        .child(
                                            gpui_kit::component::Icon::new(IconName::TextWrap)
                                                .size(px(15.))
                                                .text_color(if soft_wrap { rgb(0xffffff).into() } else { theme::foreground() }),
                                        )
                                        .accessibility_label("自动换行")
                                        .selected(soft_wrap)
                                        .toggled(soft_wrap)
                                        .tooltip(if soft_wrap { "自动换行已开启（点击切换为单行水平滚动）" } else { "自动换行已关闭（点击开启长行自动折行显示）" })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.toggle_message_soft_wrap(id, window, cx)
                                        })),
                                )
                                .child(
                    Button::new(SharedString::from(format!("{id}-nonprintable")))
                        .ghost()
                        .small()
                        .rounded(ButtonRounded::None)
                        .w(px(19.))
                        .h(px(19.))
                        .p_0()
                        .bg(if non_printable { rgb(0x26649d).into() } else { gpui_kit::transparent_black() })
                        .text_color(if non_printable { rgb(0xffffff).into() } else { theme::foreground() })
                        .child({
                            let icon_color: gpui_kit::Hsla = if non_printable {
                                rgb(0xffffff).into()
                            } else {
                                theme::foreground()
                            };
                            gpui_kit::canvas(
                                |_, _, _| {},
                                move |b, _, window, _| {
                                    let mut builder = gpui_kit::PathBuilder::stroke(px(1.5));
                                    builder.move_to(gpui_kit::point(
                                        b.left() + px(2.0),
                                        b.top() + px(2.5),
                                    ));
                                    builder.line_to(gpui_kit::point(
                                        b.left() + px(6.0),
                                        b.top() + px(11.5),
                                    ));
                                    builder.move_to(gpui_kit::point(
                                        b.left() + px(7.8),
                                        b.top() + px(5.2),
                                    ));
                                    builder.line_to(gpui_kit::point(
                                        b.left() + px(7.8),
                                        b.top() + px(11.5),
                                    ));
                                    builder.move_to(gpui_kit::point(
                                        b.left() + px(7.8),
                                        b.top() + px(7.0),
                                    ));
                                    builder.curve_to(
                                        gpui_kit::point(b.left() + px(11.8), b.top() + px(7.0)),
                                        gpui_kit::point(b.left() + px(9.8), b.top() + px(4.6)),
                                    );
                                    builder.line_to(gpui_kit::point(
                                        b.left() + px(11.8),
                                        b.top() + px(11.5),
                                    ));
                                    if let Ok(path) = builder.build() {
                                        window.paint_path(path, icon_color);
                                    }
                                },
                            )
                            .w(px(14.))
                            .h(px(14.))
                        })
                        .accessibility_label("显示非打印字符")
                        .selected(non_printable)
                        .toggled(non_printable)
                        .tooltip(if non_printable {
                            "显示非打印字符已开启（可视化呈现 \\r\\n、制表符与控制字符，点击恢复原始视图）"
                        } else {
                            "显示非打印字符（Show non-printable characters：可视化呈现 \\r\\n、制表符与控制字符）"
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_message_non_printable(id, window, cx)
                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("{id}-message-actions")))
                                        .ghost()
                                        .small()
                                        .rounded(ButtonRounded::None)
                                        .w(px(17.))
                                        .h(px(17.))
                                        .p_0()
                                        .icon(
                                            gpui_kit::component::Icon::default()
                                                .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\"><path d=\"M2.5 4h11M2.5 8h11M2.5 12h11\"/></svg>")
                                                .size(px(14.))
                                                .text_color(theme::foreground()),
                                        )
                                        .accessibility_label("Message actions")
                                        .tooltip("Message actions")
                                        .dropdown_menu(move |menu, window, cx| {
                                            menu_owner.update(cx, |this, cx| {
                                                this.focus_message_editor_by_id(id, window, cx);
                                            }).expect("消息菜单所属工作台必须存在");
                                            let o1 = menu_owner.clone();
                                            let o2 = menu_owner.clone();
                                            let o3 = menu_owner.clone();
                                            let o6 = menu_owner.clone();
                                            let o7 = menu_owner.clone();
                                            let o8 = menu_owner.clone();
                                            let o9 = menu_owner.clone();
                                            let o10 = menu_owner.clone();
                                            let o11 = menu_owner.clone();
                                            let o12 = menu_owner.clone();
                                            let o_scan = menu_owner.clone();
                                            let o_pscan = menu_owner.clone();
                                            let o_top_pscan = menu_owner.clone();
                                            let o_top_ascan = menu_owner.clone();
                                            let o_seq = menu_owner.clone();
                                            let o_org = menu_owner.clone();
                                            let o_ai = menu_owner.clone();
                                            let o_issue = menu_owner.clone();
                                            let o_attach_issue = menu_owner.clone();
                                            let o_find_refs = menu_owner.clone();
                                            let o_discover = menu_owner.clone();
                                            let o_sched = menu_owner.clone();
                                            let o_csrf = menu_owner.clone();
                                            let existing_findings = menu_owner
                                                .upgrade()
                                                .map(|entity| entity.read(cx).existing_audit_finding_ids())
                                                .unwrap_or_default();
                                            let o_site_map = menu_owner.clone();
                                            let o_cresp = menu_owner.clone();
                                            let o_sbody = menu_owner.clone();
                                            let o_links = menu_owner.clone();
                                            let o_xml = menu_owner.clone();
                                            let o_inscope = menu_owner.clone();
                                            let o_exscope = menu_owner.clone();
                                            let launcher_owner = menu_owner.clone();
                                            let browser_owner = menu_owner.clone();
                                            let req_browser_owner = menu_owner.clone();
                                            let copy_owner = menu_owner.clone();
                                            let attach_existing_submenu = PopupMenu::build(
                                                window,
                                                cx,
                                                move |sub, _, _| {
                                                    if existing_findings.is_empty() {
                                                        sub.item(
                                                            PopupMenuItem::new("暂无现有问题")
                                                                .disabled(true),
                                                        )
                                                    } else {
                                                        existing_findings.into_iter().fold(
                                                            sub.scrollable(true)
                                                                .max_h(theme::scaled_px(320.)),
                                                            |sub, (finding_id, title)| {
                                                                let owner = o_attach_issue.clone();
                                                                sub.item(
                                                                    PopupMenuItem::new(title)
                                                                        .disabled(unavailable)
                                                                        .on_click(move |_, _, cx| {
                                                                            let _ = owner.update(
                                                                                cx,
                                                                                |this, cx| {
                                                                                    this.attach_active_context_to_issue(
                                                                                        finding_id,
                                                                                        cx,
                                                                                    );
                                                                                },
                                                                            );
                                                                        }),
                                                                )
                                                            },
                                                        )
                                                    }
                                                },
                                            );
                                            let record_issue_submenu = PopupMenu::build(
                                                window,
                                                cx,
                                                move |sub, _, _| {
                                                    sub.item(
                                                        PopupMenuItem::new(
                                                            "创建新问题 (Create a new issue)",
                                                        )
                                                        .disabled(unavailable)
                                                        .on_click(move |_, _, cx| {
                                                            let _ = o_issue.update(cx, |this, cx| {
                                                                this.record_manual_issue_for_active_context(cx);
                                                            });
                                                        }),
                                                    )
                                                    .item(PopupMenuItem::submenu(
                                                        "添加到现有问题 (Add to existing issue)",
                                                        attach_existing_submenu,
                                                    ))
                                                },
                                            );
                                            let is_resp_side = id.ends_with("response") || id == "response";
                                            let req_browser_orig_owner = req_browser_owner.clone();
                                            let req_browser_curr_owner = req_browser_owner.clone();
                                            let req_browser_submenu = PopupMenu::build(
                                                window,
                                                cx,
                                                move |sub, _, _| {
                                                    sub.item(
                                                        PopupMenuItem::new(
                                                            "在原始会话中 (In original session)",
                                                        )
                                                            .disabled(unavailable)
                                                            .on_click(move |_, window, cx| {
                                                                let _ = req_browser_orig_owner.update(cx, |this, cx| {
                                                                    this.open_current_request_in_browser_with_session(
                                                                        true, window, cx,
                                                                    );
                                                                });
                                                            }),
                                                    )
                                                    .item(
                                                        PopupMenuItem::new(
                                                            "在当前浏览器会话中 (In current session)",
                                                        )
                                                            .action(Box::new(
                                                                crate::OpenConfiguredBrowser,
                                                            ))
                                                            .disabled(unavailable)
                                                            .on_click(move |_, window, cx| {
                                                                let _ = req_browser_curr_owner.update(cx, |this, cx| {
                                                                    this.open_current_request_in_browser_with_session(
                                                                        false, window, cx,
                                                                    );
                                                                });
                                                            }),
                                                    )
                                                },
                                            );
                                            let engagement_submenu = PopupMenu::build(
                                                window,
                                                cx,
                                                move |sub, _, _| {
                                                    sub.item(
                                                            PopupMenuItem::new("查找引用 (Find references)")
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_find_refs.update(cx, |this, cx| {
                                                                        this.engagement_find_references(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new("发现内容 (Discover content)")
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_discover.update(cx, |this, cx| {
                                                                        this.engagement_discover_content(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new("计划任务 (Schedule task)")
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_sched.update(cx, |this, cx| {
                                                                        this.engagement_schedule_task(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new("生成 CSRF PoC (Generate CSRF PoC)")
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_csrf.update(cx, |this, cx| {
                                                                        this.engagement_generate_csrf_poc(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .separator()
                                                        .item(
                                                            PopupMenuItem::new("发起主动扫描 (Active scan)")
                                                                .action(Box::new(crate::SendToScanner))
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_scan.update(cx, |this, cx| {
                                                                        this.send_to_scanner(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new("发起被动扫描 (Passive scan)")
                                                                .action(Box::new(crate::SendToPassiveScanner))
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_pscan.update(cx, |this, cx| {
                                                                        this.send_to_passive_scanner(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .separator()
                                                        .item(
                                                            PopupMenuItem::new("添加到范围")
                                                                .action(Box::new(crate::AddToScope))
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_inscope.update(cx, |this, cx| {
                                                                        this.add_to_scope(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new("从范围排除")
                                                                .action(Box::new(crate::ExcludeFromScope))
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_exscope.update(cx, |this, cx| {
                                                                        this.exclude_from_scope(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .separator()
                                                        .item(
                                                            PopupMenuItem::new("提取响应链接并复制 (Copy links in response)")
                                                                .action(Box::new(crate::CopyLinksInResponse))
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_links.update(cx, |this, cx| {
                                                                        this.copy_links_in_response(window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new("保存响应体到文件…")
                                                                .action(Box::new(crate::CopyResponseBodyToFile))
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o_sbody.update(cx, |this, cx| {
                                                                        this.copy_to_file(true, window, cx);
                                                                    });
                                                                }),
                                                        )
                                                        .separator()
                                                        .item(
                                                            PopupMenuItem::new("转换选区：URL 解码并复制")
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o10.update(cx, |this, cx| {
                                                                        this.focus_message_editor_by_id(id, window, cx);
                                                                        this.copy_converted_selection_action(
                                                                            &editing::CopyConvertedSelection(
                                                                                editing::ConvertKind::UrlDecode,
                                                                            ),
                                                                            window,
                                                                            cx,
                                                                        );
                                                                    });
                                                                }),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new("转换选区：Base64 解码并复制")
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o11.update(cx, |this, cx| {
                                                                        this.focus_message_editor_by_id(id, window, cx);
                                                                        this.copy_converted_selection_action(
                                                                            &editing::CopyConvertedSelection(
                                                                                editing::ConvertKind::Base64Decode,
                                                                            ),
                                                                            window,
                                                                            cx,
                                                                        );
                                                                    });
                                                                }),
                                                        )
                                                        .item(
                                                            PopupMenuItem::new("在 CyberChef 中打开选区")
                                                                .disabled(unavailable)
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = o12.update(cx, |this, cx| {
                                                                        this.focus_message_editor_by_id(id, window, cx);
                                                                        this.open_selection_in_cyberchef(
                                                                            window, cx,
                                                                        );
                                                                    });
                                                                }),
                                                        )
                                                },
                                            );
                                            menu.item(
                                                PopupMenuItem::new("打开扫描启动器 (Scan)")
                                                    .action(Box::new(crate::OpenScanLauncher))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        launcher_owner.update(cx, |this, cx| {
                                                            this.open_scan_launcher(window, cx);
                                                        }).expect("消息菜单所属工作台必须存在");
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new(
                                                    "被动扫描当前消息 (Passive scan selected message)",
                                                )
                                                .action(Box::new(crate::SendToPassiveScanner))
                                                .disabled(unavailable)
                                                .on_click(move |_, window, cx| {
                                                    let _ = o_top_pscan.update(cx, |this, cx| {
                                                        this.send_to_passive_scanner(window, cx);
                                                    });
                                                }),
                                            )
                                            .item(
                                                PopupMenuItem::new(
                                                    "主动扫描当前消息 (Active scan selected message)",
                                                )
                                                .action(Box::new(crate::SendToScanner))
                                                .disabled(unavailable)
                                                .on_click(move |_, window, cx| {
                                                    let _ = o_top_ascan.update(cx, |this, cx| {
                                                        this.send_to_scanner(window, cx);
                                                    });
                                                }),
                                            )
                                            .separator()
                                            .item(
                                                PopupMenuItem::new("发送到 Intruder")
                                                    .action(Box::new(OpenInIntruder))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o2.update(cx, |this, cx| {
                                                            this.open_in_intruder(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::element(move |window, _| {
                                                    let key = gpui_kit::component::kbd::Kbd::binding_for_action(
                                                        &OpenInRepeater,
                                                        Some("MioWorkspace"),
                                                        window,
                                                    )
                                                    .map(|kbd| {
                                                        kbd.p_0()
                                                            .flex_nowrap()
                                                            .border_0()
                                                            .bg(gpui_kit::transparent_black())
                                                    });
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "{id}-to-repeater"
                                                        )))
                                                        .test_support()
                                                        .role(gpui_kit::accesskit::Role::MenuItem)
                                                        .aria_label("发送到 Repeater")
                                                        .w_full()
                                                        .h(theme::scaled_px(23.))
                                                        .min_h(theme::scaled_px(23.))
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap(px(12.))
                                                        .child("发送到 Repeater")
                                                        .children(key)
                                                })
                                                    .action(Box::new(OpenInRepeater))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o1.update(cx, |this, cx| {
                                                            this.open_in_repeater(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new("发送到 Sequencer")
                                                    .action(Box::new(crate::SendToSequencer))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_seq.update(cx, |this, cx| {
                                                            this.send_to_sequencer(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item({
                                                let o_cmp = menu_owner.clone();
                                                PopupMenuItem::new("发送到 Comparer")
                                                    .action(if is_resp_side {
                                                        Box::new(crate::SendToComparerResponse)
                                                    } else {
                                                        Box::new(crate::SendToComparerRequest)
                                                    })
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_cmp.update(cx, |this, cx| {
                                                            this.send_to_comparer(
                                                                is_resp_side,
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    })
                                            })
                                            .item(
                                                PopupMenuItem::new("发送到 Decoder")
                                                    .action(Box::new(crate::SendToDecoder))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o3.update(cx, |this, cx| {
                                                            this.send_to_decoder(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::element(move |_, _| {
                                                    let key = gpui_kit::Keystroke::parse(
                                                        if cfg!(target_os = "macos") {
                                                            "cmd-o"
                                                        } else {
                                                            "ctrl-o"
                                                        },
                                                    )
                                                    .ok()
                                                    .map(|ks| {
                                                        gpui_kit::component::kbd::Kbd::new(ks)
                                                            .p_0()
                                                            .flex_nowrap()
                                                            .border_0()
                                                            .bg(gpui_kit::transparent_black())
                                                    });
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "{id}-to-organiser"
                                                        )))
                                                        .test_support()
                                                        .role(gpui_kit::accesskit::Role::MenuItem)
                                                        .aria_label("发送到 Organiser")
                                                        .w_full()
                                                        .h(theme::scaled_px(23.))
                                                        .min_h(theme::scaled_px(23.))
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap(px(12.))
                                                        .child("发送到 Organiser")
                                                        .children(key)
                                                })
                                                    .action(Box::new(crate::SendToOrganiser))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_org.update(cx, |this, cx| {
                                                            this.send_selected_to_organiser(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::element(move |_, _| {
                                                    let key = gpui_kit::Keystroke::parse(
                                                        if cfg!(target_os = "macos") {
                                                            "cmd-g"
                                                        } else {
                                                            "ctrl-g"
                                                        },
                                                    )
                                                    .ok()
                                                    .map(|ks| {
                                                        gpui_kit::component::kbd::Kbd::new(ks)
                                                            .p_0()
                                                            .flex_nowrap()
                                                            .border_0()
                                                            .bg(gpui_kit::transparent_black())
                                                    });
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "{id}-to-mio-at"
                                                        )))
                                                        .test_support()
                                                        .role(gpui_kit::accesskit::Role::MenuItem)
                                                        .aria_label("发送到 MioAT")
                                                        .w_full()
                                                        .h(theme::scaled_px(23.))
                                                        .min_h(theme::scaled_px(23.))
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap(px(12.))
                                                        .child("发送到 MioAT")
                                                        .children(key)
                                                })
                                                .icon(IconName::Zap)
                                                .disabled(unavailable)
                                                .on_click(move |_, _, cx| {
                                                    let _ = o_ai.update(cx, |this, cx| {
                                                        this.send_selected_to_mio_at(cx);
                                                    });
                                                }),
                                            )
                                            .separator()
                                            .item(PopupMenuItem::submenu(
                                                "记录问题 (Record an issue)",
                                                record_issue_submenu,
                                            ))
                                            .separator()
                                            .item(
                                                PopupMenuItem::new("在浏览器中打开响应 (Open response in browser)")
                                                    .action(Box::new(crate::OpenResponseInBrowser))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        browser_owner.update(cx, |this, cx| {
                                                            this.open_current_response_in_browser(window, cx);
                                                        }).expect("消息菜单所属工作台必须存在");
                                                    }),
                                            )
                                            .item(PopupMenuItem::submenu(
                                                "在浏览器中发起请求 (Request in browser)",
                                                req_browser_submenu,
                                            ))
                                            .item(PopupMenuItem::submenu(
                                                "辅助测试工具 (Engagement tools)",
                                                engagement_submenu,
                                            ))
                                            .separator()
                                            .item(
                                                PopupMenuItem::new("复制 (Copy)")
                                                    .icon(IconName::Copy)
                                                    .action(Box::new(gpui_kit::component::input::Copy))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        copy_owner.update(cx, |this, cx| {
                                                            this.copy_selected_editor_text(id, window, cx);
                                                        }).expect("消息菜单所属工作台必须存在");
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new("复制美化报文 (Copy prettified)")
                                                    .icon(IconName::Copy)
                                                    .action(Box::new(CopyPrettified))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o8.update(cx, |this, cx| {
                                                            this.focus_message_editor_by_id(id, window, cx);
                                                            this.copy_prettified(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new("保存选中文本到文件… (Save selected text to file)")
                                                    .icon(IconName::Save)
                                                    .action(Box::new(crate::SaveSelectedTextToFile))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o9.update(cx, |this, cx| {
                                                            this.save_selected_text_to_file(id, window, cx);
                                                        });
                                                    }),
                                            )
                                            .separator()
                                            .item(
                                                PopupMenuItem::new("复制 URL")
                                                    .icon(IconName::Copy)
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o6.update(cx, |this, cx| {
                                                            this.copy_url(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new("复制为 curl 命令")
                                                    .icon(IconName::Copy)
                                                    .action(Box::new(CopyCurlCommand))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o7.update(cx, |this, cx| {
                                                            this.copy_curl_command(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new("提取页面链接 (Copy links in response)")
                                                    .icon(IconName::Copy)
                                                    .action(Box::new(crate::CopyLinksInResponse))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_cresp.update(cx, |this, cx| {
                                                            this.copy_links_in_response(window, cx);
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new("导出选中记录为 XML… (Save item)")
                                                    .icon(IconName::Save)
                                                    .action(Box::new(crate::ExportSelectedHistoryXml))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_xml.update(cx, |this, cx| {
                                                            this.export_selected_history_xml(window, cx);
                                                        });
                                                    }),
                                            )
                                            .separator()
                                            .item(
                                                PopupMenuItem::new("添加到站点地图 (Add to site map)")
                                                    .action(Box::new(crate::AddToSiteMap))
                                                    .disabled(unavailable)
                                                    .on_click(move |_, window, cx| {
                                                        o_site_map.update(cx, |this, cx| this.add_to_site_map(window, cx))
                                                            .expect("消息菜单所属工作台必须存在");
                                                    }),
                                            )
                                            .compact()
                                            .max_h(theme::scaled_px(412.))
                                            .scrollable(true)
                                        }),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn message_pair(
        &self,
        owner: &'static str,
        request: &MessageEditor,
        response: &MessageEditor,
        readonly: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (request_id, response_id) = match owner {
            "history" | "imported-history" => ("history-request", "history-response"),
            "logger" => ("logger-request", "logger-response"),
            "target" => ("target-request", "target-response"),
            "organiser" => ("organiser-request", "organiser-response"),
            "dashboard" => ("dashboard-issue-request", "dashboard-issue-response"),
            _ => ("repeater-request", "repeater-response"),
        };
        let split_id: SharedString = if owner == "target" {
            "target-messages-hsplit".into()
        } else {
            format!("{owner}-hsplit").into()
        };
        let replace_readonly = readonly || self.is_message_replace_readonly(request_id);
        let req_readonly = readonly || request.is_readonly_preview();
        editing::set_url_encode_as_you_type(self.url_encode_as_you_type_enabled());
        let request_pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.message_header(request_id, "Request", request.view, cx))
            .child(
                div().flex_1().min_h_0().child(
                    editing::message_editor(request.state())
                        .readonly(req_readonly)
                        .size_full()
                        .context_menu(self.request_context_menu(
                            request.state().clone(),
                            readonly,
                        ))
                        .aria_label("请求消息编辑器"),
                ),
            )
            .child(request.render_search_bar_editable(
                request_id,
                replace_readonly || request.show_non_printable,
                cx,
                move |this, action, window, cx| {
                    this.handle_message_search_action(request_id, action, window, cx);
                },
            ));
        let response_pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.message_header(response_id, "Response", response.view, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_response_editor_body(response_id, response, cx)),
            )
            .child(
                response.render_search_bar(response_id, cx, move |this, action, window, cx| {
                    this.handle_message_search_action(response_id, action, window, cx);
                }),
            );
        let layout_mode = if owner == "repeater" {
            self.workspace.repeater_settings.layout
        } else {
            self.message_pair_layout_for_owner(owner)
        };
        if owner != "repeater" {
            match layout_mode {
                RepeaterLayoutMode::TopBottom => {
                    let vsplit_id: SharedString = format!("{owner}-vsplit").into();
                    return div()
                        .size_full()
                        .flex()
                        .child(
                            div().flex_1().min_w_0().min_h_0().child(
                                v_resizable(vsplit_id)
                                    .child(
                                        resizable_panel()
                                            .size_range(px(120.)..px(10000.))
                                            .child(request_pane),
                                    )
                                    .child(
                                        resizable_panel()
                                            .size_range(px(120.)..px(10000.))
                                            .child(response_pane),
                                    ),
                            ),
                        )
                        .into_any_element();
                }
                RepeaterLayoutMode::Combined => {
                    let active_pane = self.combined_pane_for_owner(owner);
                    let body = match active_pane {
                        super::tabs::RepeaterCombinedPane::Request => request_pane,
                        super::tabs::RepeaterCombinedPane::Response => response_pane,
                    };
                    return div()
                        .id(SharedString::from(format!("{owner}-combined-container")))
                        .test_support()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(div().flex_1().min_w_0().min_h_0().child(body))
                        .into_any_element();
                }
                RepeaterLayoutMode::SideBySide => {}
            }
        }
        div()
            .size_full()
            .flex()
            .child(
                div().flex_1().min_w_0().child(
                    h_resizable(split_id)
                        .child(
                            resizable_panel()
                                .size_range(px(160.)..px(10000.))
                                .child(request_pane),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(160.)..px(10000.))
                                .child(response_pane),
                        ),
                ),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn is_message_pretty(&self, id: &'static str) -> bool {
        match id {
            "history-request" => self.history_request.pretty_active,
            "history-response" => self.history_response.pretty_active,
            "logger-request" => self.extra_tools.logger_request.pretty_active,
            "logger-response" => self.extra_tools.logger_response.pretty_active,
            "target-request" => self.target_pane.request.pretty_active,
            "target-response" => self.target_pane.response.pretty_active,
            "organiser-request" => self.organiser.request.pretty_active,
            "organiser-response" => self.organiser.response.pretty_active,
            "dashboard-issue-request" => self.audit_dashboard.issue_request_editor.pretty_active,
            "dashboard-issue-response" => self.audit_dashboard.issue_response_editor.pretty_active,
            "websocket-payload" => self.websocket.payload.pretty_active,
            "ext-message" => self.ext_import.is_message_pretty(),
            "intercept" => self
                .intercepts
                .iter()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .is_some_and(|pending| pending.editor.pretty_active),
            "repeater-request" => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .is_some_and(|editors| editors.request.pretty_active),
            "repeater-response" => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .is_some_and(|editors| editors.response.pretty_active),
            "ws-repeater-draft" => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get(&id))
                .is_some_and(|editors| editors.draft.pretty_active),
            "ws-repeater-history" => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get(&id))
                .is_some_and(|editors| editors.history.pretty_active),
            _ => false,
        }
    }

    pub(in crate::workbench) fn is_message_non_printable(&self, id: &'static str) -> bool {
        self.message_editor_ref(id)
            .is_some_and(|editor| editor.show_non_printable)
    }

    pub(in crate::workbench) fn is_message_hide_headers(&self, id: &'static str) -> bool {
        self.message_editor_ref(id)
            .is_some_and(|editor| editor.hide_headers)
    }

    pub(in crate::workbench) fn is_message_render(&self, id: &'static str) -> bool {
        self.message_editor_ref(id)
            .is_some_and(|editor| editor.render_active)
    }

    pub(in crate::workbench) fn message_editor_ref(
        &self,
        id: &'static str,
    ) -> Option<&MessageEditor> {
        match id {
            "history-request" => Some(&self.history_request),
            "history-response" => Some(&self.history_response),
            "logger-request" => Some(&self.extra_tools.logger_request),
            "logger-response" => Some(&self.extra_tools.logger_response),
            "target-request" => Some(&self.target_pane.request),
            "target-response" => Some(&self.target_pane.response),
            "organiser-request" => Some(&self.organiser.request),
            "organiser-response" => Some(&self.organiser.response),
            "dashboard-issue-request" => Some(&self.audit_dashboard.issue_request_editor),
            "dashboard-issue-response" => Some(&self.audit_dashboard.issue_response_editor),
            "websocket-payload" => Some(&self.websocket.payload),
            "intercept" => self
                .intercepts
                .iter()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .map(|pending| &pending.editor),
            "repeater-request" => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .map(|editors| &editors.request),
            "repeater-response" => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .map(|editors| &editors.response),
            "ws-repeater-draft" => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get(&id))
                .map(|editors| &editors.draft),
            "ws-repeater-history" => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get(&id))
                .map(|editors| &editors.history),
            _ => None,
        }
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_repeater_provenance_menu(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let is_resp = match id {
            "repeater-request" => false,
            "repeater-response" => true,
            _ => return None,
        };
        let tab_id = self.workspace.active?;
        let editors = self.tab_editors.get(&tab_id)?;
        let flow = editors.loaded_flow.as_ref()?;
        let btn_id = if is_resp {
            "repeater-response-version"
        } else {
            "repeater-request-version"
        };
        let steps_len = if is_resp {
            editors.loaded_original.provenance.response_steps.len()
        } else {
            editors.loaded_original.provenance.request_steps.len()
        };
        let has_orig = if is_resp {
            editors.loaded_original.response.is_some() && flow.response.is_some()
        } else {
            editors.loaded_original.request.is_some()
        };
        if steps_len <= 1 && !has_orig {
            return None;
        }
        let editor = if is_resp {
            &editors.response
        } else {
            &editors.request
        };
        let cache_key: ProvenanceMenuCacheKey = (
            cx.entity_id(),
            tab_id,
            flow.summary.id,
            flow.summary.revision,
            is_resp,
            steps_len,
            has_orig,
            crate::workbench::inspector::message_editor_rev_key(editor, cx),
        );
        let cached = PROVENANCE_MENU_CACHE.with(|cache| {
            cache
                .borrow()
                .iter()
                .find(|(k, _)| *k == cache_key)
                .map(|(_, val)| val.clone())
        });
        let resolved = if let Some(hit) = cached {
            hit
        } else {
            let steps: Vec<(String, Vec<u8>)> = if is_resp {
                let s = &editors.loaded_original.provenance.response_steps;
                if s.len() > 1 {
                    s.iter()
                        .enumerate()
                        .map(|(i, st)| {
                            (format!("{i}. {}", st.label), response_to_raw(&st.response))
                        })
                        .collect()
                } else if let (Some(orig), Some(final_resp)) =
                    (&editors.loaded_original.response, &flow.response)
                {
                    let orig_raw = response_to_raw(orig);
                    let final_raw = response_to_raw(final_resp);
                    if orig_raw != final_raw {
                        vec![
                            ("原始响应".to_owned(), orig_raw),
                            ("最终响应".to_owned(), final_raw),
                        ]
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            } else {
                let s = &editors.loaded_original.provenance.request_steps;
                if s.len() > 1 {
                    s.iter()
                        .enumerate()
                        .map(|(i, st)| (format!("{i}. {}", st.label), request_to_raw(&st.request)))
                        .collect()
                } else if let Some(orig) = &editors.loaded_original.request {
                    let orig_raw = request_to_raw(orig);
                    let final_raw = request_to_raw(&flow.request);
                    if orig_raw != final_raw {
                        vec![
                            ("原始请求".to_owned(), orig_raw),
                            ("最终请求".to_owned(), final_raw),
                        ]
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            };
            let computed =
                if steps.len() <= 1 || !steps.iter().skip(1).any(|(_, raw)| raw != &steps[0].1) {
                    None
                } else {
                    let current = editor.bytes_lossless(cx);
                    let active_idx = steps
                        .iter()
                        .position(|(_, raw)| raw.as_slice() == current.as_slice())
                        .unwrap_or(steps.len() - 1);
                    Some((std::sync::Arc::new(steps), active_idx))
                };
            PROVENANCE_MENU_CACHE.with(|cache| {
                let mut entries = cache.borrow_mut();
                if let Some(pos) = entries.iter().position(|(k, _)| *k == cache_key) {
                    entries.remove(pos);
                } else if entries.len() >= 4 {
                    entries.remove(0);
                }
                entries.push((cache_key, computed.clone()));
            });
            computed
        };
        let (steps, active_idx) = resolved?;
        let current_label = steps[active_idx].0.clone();
        let owner = cx.weak_entity();
        Some(
            message_version_trigger(
                btn_id,
                current_label,
                "切换查看 Repeater 报文在会话规则、插件钩子或自动解包各阶段的历史快照",
            )
            .dropdown_menu(move |menu, _, _| {
                steps
                    .iter()
                    .enumerate()
                    .fold(menu.compact().min_w(px(156.)), |menu, (idx, (label, raw))| {
                        let owner = owner.clone();
                        let raw = raw.clone();
                        menu.item(
                            PopupMenuItem::new(label.clone())
                                .checked(idx == active_idx)
                                .on_click(move |_, window, cx| {
                                    let _ = owner.update(cx, |view, cx| {
                                        if let Some(ed) = view.tab_editors.get_mut(&tab_id) {
                                            if is_resp {
                                                ed.response.load(&raw, window, cx);
                                            } else {
                                                ed.request.load(&raw, window, cx);
                                                ed.dirty = true;
                                            }
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            })
            .into_any_element(),
        )
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_response_editor_body(
        &self,
        id: &'static str,
        response: &MessageEditor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if response.render_active {
            return self.render_response_visual_card(id, response, cx);
        }
        editing::message_editor(response.state())
            .readonly(true)
            .size_full()
            .context_menu(self.request_context_menu(response.state().clone(), true))
            .aria_label("响应消息编辑器")
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_response_visual_card(
        &self,
        id: &'static str,
        response: &MessageEditor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rev_key = crate::workbench::inspector::message_editor_rev_key(response, cx);
        let cached = RENDERED_RESPONSE_CACHE.with(|cache| {
            cache
                .borrow()
                .iter()
                .find(|(k, _)| *k == rev_key)
                .map(|(_, val)| val.clone())
        });
        let entry = if let Some(hit) = cached {
            hit
        } else {
            let raw = response.bytes_lossless(cx);
            let computed = if raw.is_empty() {
                None
            } else {
                let parsed = editing::parse_rendered_http_response(&raw);
                let body_for_browser =
                    std::sync::Arc::new(editing::extract_http_response_body(&raw));
                Some(std::rc::Rc::new(CachedRenderedResponse {
                    status_line: parsed.status_line,
                    status_code: parsed.status_code,
                    content_type: parsed.content_type,
                    location: parsed.location,
                    body_len: parsed.body_len,
                    summary: parsed.summary,
                    body_for_browser,
                }))
            };
            RENDERED_RESPONSE_CACHE.with(|cache| {
                let mut entries = cache.borrow_mut();
                if let Some(pos) = entries.iter().position(|(k, _)| *k == rev_key) {
                    entries.remove(pos);
                } else if entries.len() >= 4 {
                    entries.remove(0);
                }
                entries.push((rev_key, computed.clone()));
            });
            computed
        };
        let Some(parsed) = entry else {
            return div()
                .id(SharedString::from(format!("{id}-render-card")))
                .test_support()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::surface())
                .text_color(theme::muted_foreground())
                .child("暂无响应内容可供渲染预览")
                .into_any_element();
        };
        let dark = theme::is_dark();
        let card_bg = rgb(if dark { 0x25282e } else { 0xf8f9fb });
        let badge_color = parsed
            .status_code
            .map(|code| editing::status_code_color(code, dark, false))
            .unwrap_or_else(theme::foreground);
        let ct_label = parsed
            .content_type
            .clone()
            .unwrap_or_else(|| "未声明 Content-Type".into());
        let body_for_browser = std::sync::Arc::clone(&parsed.body_for_browser);
        let ct_for_browser = parsed
            .content_type
            .clone()
            .unwrap_or_else(|| "text/html; charset=utf-8".into());

        let mut header_bar = div()
            .w_full()
            .p(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme::border())
            .bg(card_bg)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .font_family(theme::mono_font_family())
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(badge_color)
                    .child(parsed.status_line.clone()),
            )
            .child(
                div()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(theme::toolbar_surface())
                    .text_size(theme::scaled_px(11.5))
                    .text_color(theme::muted_foreground())
                    .child(format!("{ct_label} · {} 字节", parsed.body_len)),
            )
            .child(div().flex_1())
            .child(
                button(
                    SharedString::from(format!("{id}-render-open-browser")),
                    "在系统浏览器中预览响应",
                )
                .icon(IconName::ExternalLink)
                .tooltip("将当前响应正文写入本地工作区预览文件并在系统默认浏览器中打开")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_rendered_response_in_browser(&body_for_browser, &ct_for_browser, cx);
                })),
            );

        if let Some(loc) = &parsed.location {
            let loc_copy = loc.clone();
            header_bar = header_bar.child(
                button(
                    SharedString::from(format!("{id}-render-copy-location")),
                    "复制重定向链接",
                )
                .icon(IconName::Link)
                .tooltip(format!("复制 Location 标头：{loc}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(loc_copy.clone()));
                    this.status = "已复制重定向 Location 链接到剪贴板。".into();
                    cx.notify();
                })),
            );
        }

        let mut content = div()
            .id(SharedString::from(format!("{id}-render-card")))
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .bg(theme::surface())
            .child(header_bar);

        match &parsed.summary {
            editing::RenderedBodySummary::Html {
                title,
                headings,
                paragraphs,
                links,
                forms,
                scripts,
            } => {
                if let Some(page_title) = title {
                    content = content.child(
                        div()
                            .p(px(10.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(card_bg)
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(11.))
                                    .text_color(theme::muted_foreground())
                                    .child("页面标题 (<title>)"),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(14.5))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(page_title.clone()),
                            ),
                    );
                }
                if !headings.is_empty() || !paragraphs.is_empty() {
                    let mut body_box = div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child("可见页面文本摘要"),
                        );
                    for (lvl, heading) in headings {
                        body_box = body_box.child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_size(theme::scaled_px((16.0 - f32::from(*lvl)).max(12.0)))
                                .child(format!("H{lvl} · {heading}")),
                        );
                    }
                    for para in paragraphs {
                        body_box = body_box.child(div().child(para.clone()));
                    }
                    content = content.child(body_box);
                }
                if !links.is_empty() {
                    let mut links_box = div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("超链接列表 ({})", links.len())),
                        );
                    for (idx, link) in links.iter().enumerate() {
                        let href_copy = link.href.clone();
                        let display_text = if link.text.is_empty() {
                            link.href.clone()
                        } else {
                            format!("{} — {}", link.text, link.href)
                        };
                        links_box = links_box.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(theme::mono_font_family())
                                        .text_size(theme::scaled_px(11.5))
                                        .child(display_text),
                                )
                                .child(
                                    button(
                                        (SharedString::from(format!("{id}-copy-href")), idx),
                                        "复制链接",
                                    )
                                    .icon(IconName::Copy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(
                                            gpui_kit::ClipboardItem::new_string(href_copy.clone()),
                                        );
                                        this.status = "已复制超链接地址到剪贴板。".into();
                                        cx.notify();
                                    })),
                                ),
                        );
                    }
                    content = content.child(links_box);
                }
                if !forms.is_empty() {
                    let mut forms_box = div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("表单与输入控件 ({})", forms.len())),
                        );
                    for form in forms {
                        let mut form_card = div()
                            .p(px(8.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface())
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .font_family(theme::mono_font_family())
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(format!("{} {}", form.method, form.action)),
                            );
                        for inp in &form.inputs {
                            form_card = form_card.child(
                                div()
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::scaled_px(11.5))
                                    .text_color(theme::muted_foreground())
                                    .child(format!(
                                        "[{}] name=\"{}\" value=\"{}\"",
                                        inp.input_type, inp.name, inp.value
                                    )),
                            );
                        }
                        forms_box = forms_box.child(form_card);
                    }
                    content = content.child(forms_box);
                }
                if !scripts.is_empty() {
                    let mut scripts_box = div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("外部脚本引用 ({})", scripts.len())),
                        );
                    for src in scripts {
                        scripts_box = scripts_box.child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .child(src.clone()),
                        );
                    }
                    content = content.child(scripts_box);
                }
            }
            editing::RenderedBodySummary::Json { pretty, byte_len } => {
                content = content.child(
                    div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("JSON 结构化响应卡片 ({byte_len} 字节)")),
                        )
                        .child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(12.))
                                .child(pretty.clone()),
                        ),
                );
            }
            editing::RenderedBodySummary::Image {
                format_name,
                mime_type,
                width,
                height,
                byte_len,
            } => {
                let dims = match (*width, *height) {
                    (Some(w), Some(h)) => format!("{w} × {h} px"),
                    _ => "尺寸未声明".into(),
                };
                content = content.child(
                    div()
                        .p(px(12.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("图像资源元数据卡片 · {format_name}")),
                        )
                        .child(div().child(format!("MIME 类型：{mime_type}")))
                        .child(div().child(format!("图像尺寸：{dims}")))
                        .child(div().child(format!("载荷大小：{byte_len} 字节"))),
                );
            }
            editing::RenderedBodySummary::PlainText { text, byte_len } => {
                content = content.child(
                    div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("纯文本响应预览 ({byte_len} 字节)")),
                        )
                        .child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(12.))
                                .child(text.clone()),
                        ),
                );
            }
            editing::RenderedBodySummary::Binary {
                byte_len,
                hex_preview,
            } => {
                content = content.child(
                    div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!(
                                    "二进制响应载荷 ({byte_len} 字节，可通过右键菜单保存响应体到文件)"
                                )),
                        )
                        .child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .child(hex_preview.clone()),
                        ),
                );
            }
        }
        content.into_any_element()
    }

    fn open_rendered_response_in_browser(
        &mut self,
        body_bytes: &[u8],
        content_type: &str,
        cx: &mut Context<Self>,
    ) {
        let lower_ct = content_type.to_ascii_lowercase();
        let ext = if lower_ct.contains("json") {
            "json"
        } else if lower_ct.contains("svg") {
            "svg"
        } else if lower_ct.contains("png") {
            "png"
        } else if lower_ct.contains("jpeg") || lower_ct.contains("jpg") {
            "jpg"
        } else if lower_ct.contains("gif") {
            "gif"
        } else if lower_ct.contains("webp") {
            "webp"
        } else if lower_ct.contains("plain") {
            "txt"
        } else {
            "html"
        };
        let preview_dir = self
            .project_path
            .as_ref()
            .and_then(|p| p.parent().map(PathBuf::from))
            .or_else(|| {
                mio_core::default_user_preferences_path()
                    .ok()
                    .and_then(|p| p.parent().map(|dir| dir.join("render-previews")))
            })
            .unwrap_or_else(|| PathBuf::from(".local/render-previews"));
        if let Err(error) = std::fs::create_dir_all(&preview_dir) {
            self.fail(format!("无法创建预览目录：{error}"), cx);
            return;
        }
        let file_path = preview_dir.join(format!(
            "mio-response-preview-{}.{ext}",
            mio_core::Uuid::new_v4()
        ));
        if let Err(error) = std::fs::write(&file_path, body_bytes) {
            self.fail(format!("写入响应预览文件失败：{error}"), cx);
            return;
        }
        let abs_path = std::fs::canonicalize(&file_path).unwrap_or(file_path);
        let path_display = abs_path.to_string_lossy().replace('\\', "/");
        let file_url = if path_display.starts_with('/') {
            format!("file://{path_display}")
        } else {
            format!("file:///{path_display}")
        };
        cx.open_url(&file_url);
        self.error = None;
        self.status = format!("已在系统浏览器中打开响应预览：{}", abs_path.display());
        cx.notify();
    }

    pub(in crate::workbench) fn is_message_soft_wrap(&self, id: &'static str) -> bool {
        match id {
            "history-request" => self.history_request.soft_wrap,
            "history-response" => self.history_response.soft_wrap,
            "logger-request" => self.extra_tools.logger_request.soft_wrap,
            "logger-response" => self.extra_tools.logger_response.soft_wrap,
            "target-request" => self.target_pane.request.soft_wrap,
            "target-response" => self.target_pane.response.soft_wrap,
            "organiser-request" => self.organiser.request.soft_wrap,
            "organiser-response" => self.organiser.response.soft_wrap,
            "dashboard-issue-request" => self.audit_dashboard.issue_request_editor.soft_wrap,
            "dashboard-issue-response" => self.audit_dashboard.issue_response_editor.soft_wrap,
            "websocket-payload" => self.websocket.payload.soft_wrap,
            "intercept" => self
                .intercepts
                .iter()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .is_some_and(|pending| pending.editor.soft_wrap),
            "repeater-request" => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .is_some_and(|editors| editors.request.soft_wrap),
            "repeater-response" => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .is_some_and(|editors| editors.response.soft_wrap),
            "ws-repeater-draft" => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get(&id))
                .is_some_and(|editors| editors.draft.soft_wrap),
            "ws-repeater-history" => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get(&id))
                .is_some_and(|editors| editors.history.soft_wrap),
            _ => theme::editor_soft_wrap(),
        }
    }

    pub(in crate::workbench) fn toggle_message_soft_wrap(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(id) {
            editor.toggle_soft_wrap(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn is_message_replace_readonly(&self, id: &'static str) -> bool {
        match id {
            "repeater-request" => {
                self.project.is_none() || self.transition == Some(TransitionPhase::Switching)
            }
            "intercept" => {
                !self.backend_available()
                    || self.transition.is_some()
                    || self
                        .selected_intercept
                        .and_then(|sel| self.intercepts.iter().find(|p| p.id() == sel))
                        .is_none_or(|p| p.deciding)
            }
            "ws-repeater-draft" => {
                self.project.is_none() || self.transition.is_some() || !self.backend_available()
            }
            _ => true,
        }
    }

    pub(in crate::workbench) fn message_editor_mut(
        &mut self,
        id: &'static str,
    ) -> Option<&mut MessageEditor> {
        match id {
            "history-request" => Some(&mut self.history_request),
            "history-response" => Some(&mut self.history_response),
            "logger-request" => Some(&mut self.extra_tools.logger_request),
            "logger-response" => Some(&mut self.extra_tools.logger_response),
            "target-request" => Some(&mut self.target_pane.request),
            "target-response" => Some(&mut self.target_pane.response),
            "organiser-request" => Some(&mut self.organiser.request),
            "organiser-response" => Some(&mut self.organiser.response),
            "dashboard-issue-request" => Some(&mut self.audit_dashboard.issue_request_editor),
            "dashboard-issue-response" => Some(&mut self.audit_dashboard.issue_response_editor),
            "websocket-payload" => Some(&mut self.websocket.payload),
            "ext-message" => self.ext_import.message_editor(),
            "intercept" => self
                .intercepts
                .iter_mut()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .map(|pending| &mut pending.editor),
            "repeater-request" => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get_mut(&id))
                .map(|editors| &mut editors.request),
            "repeater-response" => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get_mut(&id))
                .map(|editors| &mut editors.response),
            "ws-repeater-draft" => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get_mut(&id))
                .map(|editors| &mut editors.draft),
            "ws-repeater-history" => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get_mut(&id))
                .map(|editors| &mut editors.history),
            _ => None,
        }
    }

    pub(in crate::workbench) fn switch_message_view(
        &mut self,
        id: &'static str,
        view: MessageView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(id)
            && let Err(error) = editor.switch(view, window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn switch_pretty_view(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(id)
            && let Err(error) = editor.switch_pretty(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn switch_render_view(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(id)
            && let Err(error) = editor.switch_render(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_message_non_printable(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(id)
            && let Err(error) = editor.toggle_non_printable(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_message_hide_headers(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.message_editor_mut(id)
            && let Err(error) = editor.toggle_hide_headers(window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn handle_message_search_action(
        &mut self,
        id: &'static str,
        action: SearchBarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let readonly = self.is_message_replace_readonly(id);
        if let Some(editor) = self.message_editor_mut(id)
            && let Err(error) =
                editor.handle_search_bar_action_editable(action, readonly, window, cx)
        {
            self.fail(error, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn copy_converted_selection_action(
        &mut self,
        action: &editing::CopyConvertedSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            self.fail("正在切换或保存项目；未转换选区。", cx);
            return;
        }
        let pick_pair = |req: &MessageEditor, resp: &MessageEditor| {
            let req_state = req.state().clone();
            let resp_state = resp.state().clone();
            if resp.has_focus(window, cx)
                || (!req.has_focus(window, cx)
                    && !resp_state.read(cx).selected_range().is_empty()
                    && req_state.read(cx).selected_range().is_empty())
            {
                resp_state
            } else {
                req_state
            }
        };
        let target_state = match self.tool {
            Tool::Repeater => {
                if self.workspace.active.is_none()
                    && let Some(ws_id) = self.workspace.active_websocket_tab
                {
                    self.websocket_repeater
                        .editors
                        .get(&ws_id)
                        .map(|editors| pick_pair(&editors.draft, &editors.history))
                } else {
                    self.workspace
                        .active
                        .and_then(|id| self.tab_editors.get(&id))
                        .map(|editors| pick_pair(&editors.request, &editors.response))
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .intercepts
                .iter()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .map(|pending| pending.editor.state().clone()),
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                Some(pick_pair(&self.history_request, &self.history_response))
            }
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                Some(self.websocket.payload.state().clone())
            }
            Tool::Intruder => {
                let Some(id) = self.workspace.intruder.active else {
                    return;
                };
                let Some(editors) = self.intruder.editors.get(&id) else {
                    return;
                };
                let state = editors.request.state().clone();
                let result = if state.update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
                {
                    Err(editing::COMPOSITION_PENDING.into())
                } else {
                    let selected = state.read(cx).selected_value();
                    if selected.is_empty() {
                        Err("请先在消息编辑器中选中要转换的文本。".into())
                    } else {
                        editing::convert_selection(action.0, selected.as_ref())
                    }
                };
                self.finish_copy_converted_selection(result, cx);
                return;
            }
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => Some(
                pick_pair(&self.target_pane.request, &self.target_pane.response),
            ),
            Tool::Logger => Some(pick_pair(
                &self.extra_tools.logger_request,
                &self.extra_tools.logger_response,
            )),
            Tool::Organiser => Some(pick_pair(&self.organiser.request, &self.organiser.response)),
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open
                    && self
                        .audit_dashboard
                        .scan_request_editor
                        .has_focus(window, cx)
                {
                    Some(self.audit_dashboard.scan_request_editor.state().clone())
                } else {
                    Some(pick_pair(
                        &self.audit_dashboard.issue_request_editor,
                        &self.audit_dashboard.issue_response_editor,
                    ))
                }
            }
            Tool::Sequencer => Some(self.sequencer_pane.request.state().clone()),
            Tool::Collaborator => Some(self.collaborator_pane.raw_editor.state().clone()),
            _ => None,
        };
        let Some(state) = target_state else {
            return;
        };
        let result = editing::convert_selected_editor_text(&state, action.0, window, cx);
        self.finish_copy_converted_selection(result, cx);
    }

    fn finish_copy_converted_selection(
        &mut self,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(converted) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(converted));
                self.error = None;
                self.status = "已转换选区并复制到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn copy_links_in_response(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::History {
            self.copy_selected_history_links(window, cx);
            return;
        }
        if self.tool == Tool::Target && self.target_pane.page == target::TargetPage::SiteMap {
            self.copy_selected_target_links(window, cx);
            return;
        }
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let raw_response = match self.tool_message_bytes(true, window, cx) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                self.fail("当前没有可提取链接的 HTTP 响应报文。", cx);
                return;
            }
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let base_url = match self
            .current_transfer_draft("提取响应链接", window, cx)
            .and_then(|draft| {
                mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
                    .map_err(|error| error.to_string())
            }) {
                Ok(url) => url,
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
        let links = match crate::workbench::wb_render_history::extract_response_links_list(
            &base_url,
            &raw_response,
        ) {
            Ok(links) => links,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if links.is_empty() {
            self.error = None;
            self.status = "选中响应中未提取到有效的页面 URL 链接。".into();
            cx.notify();
            return;
        }
        let count = links.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(links.join("\n")));
        self.error = None;
        self.status = format!("已提取并复制 {count} 条页面链接到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench) fn export_selected_history_xml(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_active_item_xml(window, cx);
    }

    pub(crate) fn open_current_request_in_configured_browser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_current_request_in_browser_with_session(false, window, cx);
    }

    pub(crate) fn open_current_request_in_browser_with_session(
        &mut self,
        original_session: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.ext_import.visible
        {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let (url, raw_request) = match self
            .current_transfer_draft("在浏览器中发起请求", window, cx)
            .and_then(|draft| {
                if draft.raw.is_empty() {
                    return Err("当前没有已加载的 HTTP 请求报文。".into());
                }
                mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version)
                    .map(|url| (url, draft.raw))
                    .map_err(|error| error.to_string())
            }) {
            Ok(pair) => pair,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.fail(
                "配置浏览器仅支持打开 http:// 或 https:// 协议的请求 URL。",
                cx,
            );
            return;
        }
        let session = if original_session {
            mio_core::BrowserRequestSession::OriginalSession { raw_request }
        } else {
            mio_core::BrowserRequestSession::CurrentSession
        };
        if self.enqueue(
            Command::OpenConfiguredBrowser {
                project,
                url: Some(url.clone()),
                session,
            },
            cx,
        ) {
            self.error = None;
            self.status = format!("正在配置浏览器中打开请求：{url}");
            cx.notify();
        }
    }

    pub(crate) fn open_current_response_in_browser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let raw = match self.tool_message_bytes(true, window, cx) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                self.fail("当前没有可在浏览器中打开的 HTTP 响应报文。", cx);
                return;
            }
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let parsed = editing::parse_rendered_http_response(&raw);
        let body = editing::extract_http_response_body(&raw);
        if body.is_empty() {
            self.fail("当前 HTTP 响应正文为空，无法在浏览器中打开。", cx);
            return;
        }
        let content_type = parsed
            .content_type
            .as_deref()
            .unwrap_or("text/html; charset=utf-8");
        self.open_rendered_response_in_browser(&body, content_type, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::tests::{drain, fixture, tab};
    use gpui_kit::TestAppContext;
    use gpui_kit::test::{TestEventExt, TestWindowExt};
    use mio_core::{
        FlowProvenance, FlowState, FlowSummary, Header, HttpRequest, ProvenanceActor,
        ResponseProvenanceStep, TrafficSource, Uuid,
    };

    #[gpui_kit::test]
    fn editing_repeater_nonprintable_render_view_and_pretty_rewrite_preserve_wire_bytes(
        cx: &mut TestAppContext,
    ) {
        let mut initial = tab("视图与改写");
        initial.draft.target = "http://127.0.0.1:9001".into();
        initial.draft.raw = concat!(
            "POST /api/login HTTP/1.1\r\n",
            "Host: 127.0.0.1:9001\r\n",
            "Content-Type: application/json\r\n",
            "Content-Length: 27\r\n\r\n",
            "{\"user\":\"alice\",\"pin\":\"09\"}"
        )
        .as_bytes()
        .to_vec();
        let orig_wire = initial.draft.raw.clone();
        let tab_id = initial.id;
        let mut f = fixture(cx, vec![initial], 64);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.click("tool-repeater", cx);
            window.render_frame(cx);
            window.click("repeater-request-nonprintable", cx);
            window.render_frame(cx);
            let actual = f.view.update(cx, |view, cx| {
                let editor = &view.tab_editors[&tab_id].request;
                assert!(editor.show_non_printable);
                assert!(editor.state().read(cx).value().contains(" \\r   \\n "));
                editor.bytes(window, cx).unwrap()
            });
            assert_eq!(actual, orig_wire);

            window.click("repeater-request-nonprintable", cx);
            window.render_frame(cx);
            let restored = f.view.update(cx, |view, cx| {
                let editor = &view.tab_editors[&tab_id].request;
                assert!(!editor.show_non_printable);
                editor.bytes(window, cx).unwrap()
            });
            assert_eq!(restored, orig_wire);

            window.click("repeater-request-pretty", cx);
            window.render_frame(cx);
            assert!(f.view.read(cx).tab_editors[&tab_id].request.pretty_active);
            window.dispatch_action(Box::new(crate::ChangeRequestMethod), cx);
        })
        .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let rewritten = f.view.update(cx, |view, cx| {
                view.tab_editors[&tab_id].request.bytes(window, cx).unwrap()
            });
            let rewritten_str = std::str::from_utf8(&rewritten).unwrap();
            assert!(
                rewritten_str.starts_with("GET /api/login?user=alice&pin=09 HTTP/1.1\r\n"),
                "expected Pretty rewrite to flatten JSON into GET query: {rewritten_str}"
            );
        })
        .unwrap();
        let _ = drain(&mut f.commands);

        let flow_id = Uuid::new_v4();
        let req_obj = HttpRequest {
            method: "GET".into(),
            url: "http://127.0.0.1:9001/api/login".into(),
            version: HttpVersion::Http11,
            headers: vec![Header {
                name: "Host".into(),
                value: b"127.0.0.1:9001".to_vec(),
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        };
        let orig_resp = HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![Header {
                name: "Content-Encoding".into(),
                value: b"gzip".to_vec(),
            }],
            body: b"compressed-raw".to_vec(),
            trailers: Vec::new(),
        };
        let final_resp = HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![Header {
                name: "Content-Type".into(),
                value: b"text/html; charset=utf-8".to_vec(),
            }],
            body: b"<html><head><title>Portal</title></head><body><a href=\"/next\">Next</a></body></html>"
                .to_vec(),
            trailers: Vec::new(),
        };
        let record = FlowRecord {
            summary: FlowSummary {
                id: flow_id,
                revision: 1,
                source: TrafficSource::Repeater,
                method: "GET".into(),
                url: "http://127.0.0.1:9001/api/login".into(),
                status: Some(200),
                state: FlowState::Complete,
                request_bytes: 64,
                response_bytes: 96,
                started_unix_ms: 0,
                elapsed_ms: 12,
            },
            request: req_obj,
            response: Some(final_resp.clone()),
        };
        let original = OriginalMessages {
            request: None,
            response: Some(orig_resp.clone()),
            response_body_complete: Some(true),
            provenance: FlowProvenance {
                request_steps: Vec::new(),
                response_steps: vec![
                    ResponseProvenanceStep {
                        actor: ProvenanceActor::Original,
                        label: "原始压缩响应".into(),
                        response: orig_resp,
                    },
                    ResponseProvenanceStep {
                        actor: ProvenanceActor::UnpackCompressed,
                        label: "自动解包后响应".into(),
                        response: final_resp,
                    },
                ],
            },
        };

        cx.update_window(f.window, |_, _, cx| {
            f.view.update(cx, |view, _| {
                view.workspace.tabs[0].last_flow = Some(flow_id);
                view.repeater_reads.insert(99, tab_id);
            });
        })
        .unwrap();
        f.events
            .try_send(Event::FlowDetail {
                project: f.project,
                selection_revision: 99,
                record: Box::new(record),
                original: Box::new(original),
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("repeater-response-version").is_some(),
                "Repeater response provenance dropdown must appear when provenance steps > 1"
            );
            window.click("repeater-response-render", cx);
            window.render_frame(cx);
            assert!(window.try_find("repeater-response-render-card").is_some());
            assert!(
                window
                    .try_find("repeater-response-render-open-browser")
                    .is_some()
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn ext_message_editor_two_row_header_search_gear_menu_and_send_cancel_coplace(
        cx: &mut TestAppContext,
    ) {
        let mut initial = tab("1");
        initial.draft.target = "http://127.0.0.1:9001".into();
        initial.draft.raw = b"GET /?q=Alpha&x=alpha HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
        let tab_id = initial.id;
        let mut f = fixture(cx, vec![initial], 64);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.click("tool-repeater", cx);
            window.click("inspector-toggle", cx);
            window.render_frame(cx);

            // 1. Send and Cancel co-placement: when idle, only repeater-send exists.
            let send_bounds = window.find("repeater-send").bounds();
            assert_eq!(send_bounds.size.width, px(65.));
            assert_eq!(send_bounds.size.height, px(24.));
            assert!(window.try_find("repeater-cancel").is_none());

            // 2. Two-row header geometry and right-aligned wrap / non-printable controls.
            let pretty_bounds = window.find("repeater-request-pretty").bounds();
            let raw_bounds = window.find("repeater-request-raw").bounds();
            let hex_bounds = window.find("repeater-request-hex").bounds();
            let wrap_bounds = window.find("repeater-request-wrap").bounds();
            let np_bounds = window.find("repeater-request-nonprintable").bounds();
            assert_eq!(pretty_bounds.size.width, px(64.));
            assert_eq!(raw_bounds.size.width, px(56.));
            assert_eq!(hex_bounds.size.width, px(53.));
            assert_eq!(pretty_bounds.size.height, px(37.));
            assert_eq!(wrap_bounds.size.width, px(19.));
            assert_eq!(np_bounds.size.width, px(19.));
            assert!(wrap_bounds.origin.x > hex_bounds.right() + px(40.));
            assert!(np_bounds.origin.x > wrap_bounds.right());

            // 3. Layout switcher resides in repeater-response first row, above Render tab.
            let side_bounds = window.find("repeater-layout-side").bounds();
            let stack_bounds = window.find("repeater-layout-stack").bounds();
            let combined_bounds = window.find("repeater-layout-combined").bounds();
            let render_bounds = window.find("repeater-response-render").bounds();
            assert_eq!(side_bounds.size.width, px(25.));
            assert_eq!(stack_bounds.size.width, px(25.));
            assert_eq!(combined_bounds.size.width, px(25.));
            assert_eq!(side_bounds.size.height, px(19.));
            assert_eq!(render_bounds.size.width, px(72.));
            assert!(side_bounds.bottom() <= render_bounds.origin.y);

            // 4. Bottom search bar: case/regex/replace are tucked inside the gear menu.
            let search_bar = window.find("repeater-request-search-bar").bounds();
            assert_eq!(search_bar.size.height, px(47.));
            assert!(window.try_find("repeater-request-search-case").is_none());
            assert!(window.try_find("repeater-request-search-regex").is_none());
            assert!(window.try_find("repeater-request-search-replace-toggle").is_none());
            let prev_bounds = window.find("repeater-request-search-prev").bounds();
            let next_bounds = window.find("repeater-request-search-next").bounds();
            assert_eq!(prev_bounds.size.width, px(24.));
            assert_eq!(next_bounds.size.width, px(24.));

            // Open gear menu to reveal case/regex/replace options.
            window.click("repeater-request-search-options", cx);
            window.render_frame(cx);
            assert!(window.try_find("repeater-request-search-case").is_some());
            assert!(window.try_find("repeater-request-search-regex").is_some());
            assert!(window.try_find("repeater-request-search-replace-toggle").is_some());
            window.click("repeater-request-search-case", cx);
            window.render_frame(cx);
            assert!(f.view.read(cx).tab_editors[&tab_id].request.case_sensitive);

            // 5. Inspector sidebar width and 39px section headers.
            let inspector_bounds = window.find("inspector-sidebar").bounds();
            assert_eq!(inspector_bounds.size.width, px(343.));
            let expand_all = window.find("inspector-expand-all").bounds();
            assert_eq!(expand_all.size.width, px(18.));
            assert_eq!(expand_all.size.height, px(18.));

            // 6. Click Send -> co-placed button switches to repeater-cancel.
            window.click("repeater-send", cx);
            window.render_frame(cx);
            assert!(window.try_find("repeater-send").is_none());
            let cancel_bounds = window.find("repeater-cancel").bounds();
            assert_eq!(cancel_bounds.origin, send_bounds.origin);
            assert_eq!(cancel_bounds.size, send_bounds.size);
            window.click("repeater-cancel", cx);
        })
        .unwrap();
        let cancel_cmd = drain(&mut f.commands)
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::CancelSend {
                    tab_id: t,
                    run_id,
                    draft_revision,
                    ..
                } if t == tab_id => Some((run_id, draft_revision)),
                _ => None,
            })
            .expect("clicking Cancel must enqueue CancelSend");
        f.events
            .try_send(Event::SendRejected {
                project: f.project,
                tab_id,
                run_id: cancel_cmd.0,
                draft_revision: cancel_cmd.1,
                message: "用户已取消".into(),
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("repeater-send").is_some());
            assert!(window.try_find("repeater-cancel").is_none());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn history_message_actions_support_scan_launcher_selection_copy_save_and_browser_preview(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        let initial = drain(&mut f.commands);
        let query_revision = initial
            .iter()
            .find_map(|cmd| match cmd {
                Command::QueryHttpHistory { query } => Some(query.query_revision),
                _ => None,
            })
            .expect("initial history query");

        let mut flow = crate::workbench::tests::record(
            "https://portal.example.com/account?token=secret123",
            "{\"role\":\"admin\"}",
        );
        flow.request.url = "https://portal.example.com/account?token=secret123".into();
        flow.summary.status = Some(200);
        flow.response = Some(HttpResponse {
            version: HttpVersion::Http11,
            status: 200,
            reason_phrase: None,
            headers: vec![Header {
                name: "Content-Type".into(),
                value: b"text/html; charset=utf-8".to_vec(),
            }],
            body: b"<html><body><a href=\"/settings\">Settings</a></body></html>".to_vec(),
            trailers: Vec::new(),
        });
        let flow_id = flow.summary.id;

        f.events
            .try_send(Event::HistoryPage {
                project: f.project,
                query_revision,
                offset: 0,
                total: 1,
                rows: vec![HistoryRow::Live(flow.summary.clone())],
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.proxy_page = ProxyPage::History;
                view.select_flow(flow_id, window, cx);
            });
        })
        .unwrap();
        let sel_rev = drain(&mut f.commands)
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::ReadFlow {
                    selection_revision, ..
                } => Some(selection_revision),
                _ => None,
            })
            .expect("read flow command");
        f.events
            .try_send(Event::FlowDetail {
                project: f.project,
                selection_revision: sel_rev,
                record: Box::new(flow),
                original: Box::default(),
            })
            .unwrap();
        cx.run_until_parked();

        let out_dir = std::env::current_dir()
            .unwrap()
            .join(".local")
            .join(format!("test-msg-actions-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&out_dir).unwrap();
        let sel_path = out_dir.join("selected-token.txt");

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("history-layout-combined", cx);
            window.render_frame(cx);
            assert!(window.find("history-combined-container").visible());
            let req_tab_bounds = window.find("history-combined-req").bounds();
            let resp_tab_bounds = window.find("history-combined-resp").bounds();
            let pretty_bounds = window.find("history-request-pretty").bounds();
            let layout_side_bounds = window.find("history-layout-side").bounds();
            assert_eq!(req_tab_bounds.size.width, px(90.));
            assert_eq!(req_tab_bounds.size.height, px(39.));
            assert_eq!(resp_tab_bounds.size.width, px(102.));
            assert_eq!(resp_tab_bounds.size.height, px(39.));
            assert_eq!(resp_tab_bounds.origin.x, req_tab_bounds.right());
            assert_eq!(pretty_bounds.origin.y, req_tab_bounds.bottom());
            assert!(layout_side_bounds.origin.x > resp_tab_bounds.right());
            window.click("history-combined-resp", cx);
            window.render_frame(cx);
            assert_eq!(window.find("history-combined-resp").checked(), Some(true));
            assert!(window.try_find("history-response-render").is_some());
            window.click("history-combined-req", cx);
            window.render_frame(cx);
            assert_eq!(window.find("history-combined-req").checked(), Some(true));
            window.click("history-layout-side", cx);
            window.render_frame(cx);
            f.view.update(cx, |view, cx| {
                view.copy_selected_editor_text("history-request", window, cx);
            });
            assert!(
                f.view
                    .read(cx)
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("请先在消息编辑器中选中"))
            );

            f.view.update(cx, |view, cx| {
                let req_state = view.history_request.state().clone();
                let text = req_state.read(cx).value().to_string();
                let start = text.find("secret123").expect("token in request");
                req_state.update(cx, |s, cx| {
                    s.set_selected_range_with_direction(start..start + 9, false, cx);
                });
                view.copy_selected_editor_text("history-request", window, cx);
            });
            assert!(f.view.read(cx).error.is_none());
        })
        .unwrap();
        assert_eq!(
            cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text())),
            Some("secret123".into())
        );

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.save_selected_text_to_file("history-request", window, cx);
            });
        })
        .unwrap();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some(sel_path.clone()));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&sel_path).unwrap(), "secret123");
        let _ = std::fs::remove_dir_all(&out_dir);

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.open_current_response_in_browser(window, cx);
            });
            assert!(f.view.read(cx).error.is_none());
            assert!(f.view.read(cx).status.contains("已在系统浏览器中打开响应预览"));

            f.view.update(cx, |view, cx| {
                view.open_scan_launcher(window, cx);
            });
            assert_eq!(f.view.read(cx).tool, Tool::Dashboard);
            assert!(f.view.read(cx).audit_dashboard.new_scan_open);
            assert_eq!(
                f.view.read(cx).audit_dashboard.scan_url_input.read(cx).value(),
                "https://portal.example.com/account?token=secret123"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn message_actions_menu_follows_ext_group_order_height_cap_shortcuts_and_single_dispatch(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        let initial = drain(&mut f.commands);
        let query_revision = initial
            .iter()
            .find_map(|cmd| match cmd {
                Command::QueryHttpHistory { query } => Some(query.query_revision),
                _ => None,
            })
            .expect("initial history query");

        let mut flow = crate::workbench::tests::record(
            "https://portal.example.com/account?token=secret123",
            "{\"role\":\"admin\"}",
        );
        flow.request.url = "https://portal.example.com/account?token=secret123".into();
        flow.summary.status = Some(200);
        let flow_id = flow.summary.id;

        f.events
            .try_send(Event::HistoryPage {
                project: f.project,
                query_revision,
                offset: 0,
                total: 1,
                rows: vec![HistoryRow::Live(flow.summary.clone())],
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.proxy_page = ProxyPage::History;
                view.open_current_request_in_configured_browser(window, cx);
            });
            assert!(
                f.view
                    .read(cx)
                    .error
                    .as_deref()
                    .is_some_and(|err| err.contains("已加载"))
            );
        })
        .unwrap();
        assert!(drain(&mut f.commands).is_empty());

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.select_flow(flow_id, window, cx);
            });
        })
        .unwrap();
        let sel_rev = drain(&mut f.commands)
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::ReadFlow {
                    selection_revision, ..
                } => Some(selection_revision),
                _ => None,
            })
            .expect("read flow command");
        f.events
            .try_send(Event::FlowDetail {
                project: f.project,
                selection_revision: sel_rev,
                record: Box::new(flow),
                original: Box::default(),
            })
            .unwrap();
        cx.run_until_parked();
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("history-request-message-actions", cx);
            window.render_frame(cx);

            let popup_bounds = window.find("popup-menu").bounds();
            assert!(
                popup_bounds.size.height <= theme::scaled_px(420.),
                "message actions menu height {:?} must stay <= 420pt",
                popup_bounds.size.height
            );

            let menu = window.within("popup-menu");
            let ordered_labels = [
                "打开扫描启动器 (Scan)",
                "被动扫描当前消息 (Passive scan selected message)",
                "主动扫描当前消息 (Active scan selected message)",
                "发送到 Intruder",
                "发送到 Repeater",
                "发送到 Sequencer",
                "发送到 Comparer",
                "发送到 Decoder",
                "发送到 Organiser",
                "发送到 MioAT",
                "记录问题 (Record an issue)",
                "在浏览器中打开响应 (Open response in browser)",
                "在浏览器中发起请求 (Request in browser)",
                "辅助测试工具 (Engagement tools)",
                "复制 (Copy)",
                "复制美化报文 (Copy prettified)",
                "保存选中文本到文件… (Save selected text to file)",
                "复制 URL",
                "复制为 curl 命令",
                "提取页面链接 (Copy links in response)",
                "导出选中记录为 XML… (Save item)",
                "添加到站点地图 (Add to site map)",
            ];
            let mut prev_bottom = None;
            let separator_before = [
                "发送到 Intruder",
                "记录问题 (Record an issue)",
                "在浏览器中打开响应 (Open response in browser)",
                "复制 (Copy)",
                "复制 URL",
                "添加到站点地图 (Add to site map)",
            ];
            for label in ordered_labels {
                let item_bounds = if label == "发送到 Repeater" {
                    menu.find("history-request-to-repeater").bounds()
                } else {
                    menu.find_by_label(label).bounds()
                };
                assert_eq!(
                    item_bounds.size.height,
                    px(23.),
                    "compact menu item '{label}' height must be 23px"
                );
                if let Some(prev) = prev_bottom {
                    let expected_gap = if separator_before.contains(&label) {
                        px(1.)
                    } else {
                        px(0.)
                    };
                    assert!(
                        (item_bounds.origin.y - (prev + expected_gap)).abs() <= px(0.5),
                        "menu item '{label}' at {:?} must follow {:?} with gap {:?}",
                        item_bounds.origin.y,
                        prev,
                        expected_gap
                    );
                }
                prev_bottom = Some(item_bounds.bottom());
            }

            window.scroll(
                "popup-menu",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-240.))),
                cx,
            );
            window.render_frame(cx);
            let last_item_bounds = window
                .within("popup-menu")
                .find_by_label("添加到站点地图 (Add to site map)")
                .bounds();
            assert!(
                last_item_bounds.bottom() <= popup_bounds.bottom() + px(1.),
                "scrolling to the last menu item must bring it inside popup_bounds {:?}, got {:?}",
                popup_bounds,
                last_item_bounds
            );
            window.scroll(
                "popup-menu",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(240.))),
                cx,
            );
            window.render_frame(cx);

            let rendered_texts = gpui_kit::base::test_support::texts(window);
            let expected_repeater_kbd = gpui_kit::component::kbd::Kbd::format(
                &gpui_kit::Keystroke::parse("cmd-r").unwrap(),
            );
            let expected_intruder_kbd = gpui_kit::component::kbd::Kbd::format(
                &gpui_kit::Keystroke::parse("cmd-i").unwrap(),
            );
            let expected_copy_kbd = gpui_kit::component::kbd::Kbd::format(
                &gpui_kit::Keystroke::parse("cmd-c").unwrap(),
            );
            let expected_organiser_kbd = gpui_kit::component::kbd::Kbd::format(
                &gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") {
                    "cmd-o"
                } else {
                    "ctrl-o"
                })
                .unwrap(),
            );
            let expected_ai_kbd = gpui_kit::component::kbd::Kbd::format(
                &gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") {
                    "cmd-g"
                } else {
                    "ctrl-g"
                })
                .unwrap(),
            );
            assert!(rendered_texts.iter().any(|t| t.text() == expected_repeater_kbd));
            assert!(rendered_texts.iter().any(|t| t.text() == expected_intruder_kbd));
            assert!(rendered_texts.iter().any(|t| t.text() == expected_copy_kbd));
            assert!(rendered_texts.iter().any(|t| t.text() == expected_organiser_kbd));
            assert!(rendered_texts.iter().any(|t| t.text() == expected_ai_kbd));

            window.click("history-request-to-repeater", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let repeater_cmds = drain(&mut f.commands);
        assert_eq!(
            repeater_cmds.len(),
            1,
            "clicking Send to Repeater must dispatch exactly once"
        );
        assert!(matches!(
            repeater_cmds.as_slice(),
            [Command::OpenRepeater { project, flow_id: f_id, original: false }]
                if *project == f.project && *f_id == flow_id
        ));

        cx.update_window(f.window, |_, window, cx| {
            window.click("tool-proxy", cx);
            window.render_frame(cx);
            window.click("history-request-message-actions", cx);
            window.render_frame(cx);
            let req_in_browser = window
                .within("popup-menu")
                .find_by_label("在浏览器中发起请求 (Request in browser)")
                .bounds()
                .center();
            window.pointer_down(req_in_browser, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(req_in_browser, gpui_kit::MouseButton::Left, cx);
            window.render_frame(cx);
            let orig_bounds = window
                .within("submenu")
                .find_by_label("在原始会话中 (In original session)")
                .bounds();
            let curr_bounds = window
                .within("submenu")
                .find_by_label("在当前浏览器会话中 (In current session)")
                .bounds();
            assert_eq!(orig_bounds.size.height, px(23.));
            assert_eq!(curr_bounds.size.height, px(23.));
            assert!((curr_bounds.origin.y - orig_bounds.bottom()).abs() <= px(0.5));
            let curr_session = curr_bounds.center();
            window.pointer_down(curr_session, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(curr_session, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
        cx.run_until_parked();
        let browser_cmds = drain(&mut f.commands);
        assert_eq!(
            browser_cmds.len(),
            1,
            "clicking Request in browser must dispatch exactly once"
        );
        assert!(matches!(
            browser_cmds.as_slice(),
            [Command::OpenConfiguredBrowser {
                project,
                url: Some(url),
                session: mio_core::BrowserRequestSession::CurrentSession,
            }] if *project == f.project && url == "https://portal.example.com/account?token=secret123"
        ));
    }
}
