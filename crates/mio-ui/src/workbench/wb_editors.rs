use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use mio_core::RepeaterLayoutMode;

mod actions;
mod response_visual;
#[cfg(test)]
mod tests;

type ProvenanceMenuCacheKey = (
    TabId,
    FlowId,
    u64,
    bool,
    usize,
    bool,
    crate::workbench::inspector::MessageEditorRevKey,
);

type ProvenanceMenuCacheVal = Option<(std::sync::Arc<Vec<(String, Vec<u8>)>>, usize)>;

#[derive(Default)]
pub(in crate::workbench) struct EditorUiCaches {
    provenance_menu_cache:
        std::cell::RefCell<Vec<(ProvenanceMenuCacheKey, ProvenanceMenuCacheVal)>>,
    pub(in crate::workbench::wb_editors) rendered_response_cache: std::cell::RefCell<
        Vec<(
            crate::workbench::inspector::MessageEditorRevKey,
            Option<std::rc::Rc<response_visual::CachedRenderedResponse>>,
        )>,
    >,
}

impl EditorUiCaches {
    pub(in crate::workbench) fn clear(&self) {
        self.provenance_menu_cache.borrow_mut().clear();
        self.rendered_response_cache.borrow_mut().clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::workbench) enum EditorSlot {
    HistoryRequest,
    HistoryResponse,
    LoggerRequest,
    LoggerResponse,
    TargetRequest,
    TargetResponse,
    OrganiserRequest,
    OrganiserResponse,
    DashboardIssueRequest,
    DashboardIssueResponse,
    WebsocketPayload,
    ExtMessage,
    Intercept,
    RepeaterRequest,
    RepeaterResponse,
    WsRepeaterDraft,
    WsRepeaterHistory,
}

impl EditorSlot {
    pub(in crate::workbench) const fn id(self) -> &'static str {
        match self {
            Self::HistoryRequest => "history-request",
            Self::HistoryResponse => "history-response",
            Self::LoggerRequest => "logger-request",
            Self::LoggerResponse => "logger-response",
            Self::TargetRequest => "target-request",
            Self::TargetResponse => "target-response",
            Self::OrganiserRequest => "organiser-request",
            Self::OrganiserResponse => "organiser-response",
            Self::DashboardIssueRequest => "dashboard-issue-request",
            Self::DashboardIssueResponse => "dashboard-issue-response",
            Self::WebsocketPayload => "websocket-payload",
            Self::ExtMessage => "ext-message",
            Self::Intercept => "intercept",
            Self::RepeaterRequest => "repeater-request",
            Self::RepeaterResponse => "repeater-response",
            Self::WsRepeaterDraft => "ws-repeater-draft",
            Self::WsRepeaterHistory => "ws-repeater-history",
        }
    }

    pub(in crate::workbench) const fn owner(self) -> Option<&'static str> {
        match self {
            Self::RepeaterRequest | Self::RepeaterResponse => Some("repeater"),
            Self::HistoryRequest | Self::HistoryResponse => Some("history"),
            Self::TargetRequest | Self::TargetResponse => Some("target"),
            Self::LoggerRequest | Self::LoggerResponse => Some("logger"),
            Self::OrganiserRequest | Self::OrganiserResponse => Some("organiser"),
            Self::DashboardIssueRequest | Self::DashboardIssueResponse => Some("dashboard"),
            _ => None,
        }
    }

    pub(in crate::workbench) const fn is_request(self) -> bool {
        matches!(
            self,
            Self::RepeaterRequest
                | Self::HistoryRequest
                | Self::TargetRequest
                | Self::LoggerRequest
                | Self::OrganiserRequest
                | Self::DashboardIssueRequest
        )
    }

    pub(in crate::workbench) const fn is_response(self) -> bool {
        matches!(
            self,
            Self::RepeaterResponse
                | Self::HistoryResponse
                | Self::TargetResponse
                | Self::LoggerResponse
                | Self::OrganiserResponse
                | Self::DashboardIssueResponse
        )
    }

    pub(in crate::workbench) const fn pair_for_owner(owner: &str) -> (Self, Self) {
        match owner {
            _ if matches_str(owner, "history") || matches_str(owner, "imported-history") => {
                (Self::HistoryRequest, Self::HistoryResponse)
            }
            _ if matches_str(owner, "logger") => (Self::LoggerRequest, Self::LoggerResponse),
            _ if matches_str(owner, "target") => (Self::TargetRequest, Self::TargetResponse),
            _ if matches_str(owner, "organiser") => {
                (Self::OrganiserRequest, Self::OrganiserResponse)
            }
            _ if matches_str(owner, "dashboard") => {
                (Self::DashboardIssueRequest, Self::DashboardIssueResponse)
            }
            _ => (Self::RepeaterRequest, Self::RepeaterResponse),
        }
    }
}

const fn matches_str(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
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

    #[inline(never)]
    pub(in crate::workbench) fn message_header(
        &self,
        slot: EditorSlot,
        title: &'static str,
        view: MessageView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = slot.id();
        let pretty = self.is_message_pretty(slot);
        let hide_headers = self.is_message_hide_headers(slot);
        let non_printable = self.is_message_non_printable(slot);
        let render_active = self.is_message_render(slot);
        let soft_wrap = self.is_message_soft_wrap(slot);
        let raw_selected = !pretty && !render_active && view == MessageView::Raw;
        let hex_selected = !pretty && !render_active && view == MessageView::Hex;
        let provenance = self
            .render_message_provenance_menu(slot, cx)
            .or_else(|| self.render_repeater_provenance_menu(slot, cx))
            .or_else(|| self.render_target_provenance_menu(slot, cx));
        let title = if slot.is_response() {
            "Response"
        } else if slot.is_request() {
            "Request"
        } else {
            title
        };
        let editor_owner = slot.owner();
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
                slot.is_request()
            } else {
                slot.is_response()
            }
        });
        let is_target_editor = matches!(slot, EditorSlot::TargetRequest | EditorSlot::TargetResponse);
        let compact_header = is_target_editor && !is_combined;
        let narrow_view_tabs = is_target_editor && self.inspector.open && !is_combined && !is_top_bottom;
        let row1_h = if compact_header { px(30.) } else { px(39.) };
        let row2_h = if compact_header { px(30.) } else { px(38.) };
        let header_h = if compact_header { px(60.) } else { px(77.) };
        let tab_h = if compact_header { px(29.) } else { px(37.) };
        let overflow_active = hex_selected || (slot.is_response() && render_active);
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
                            this.switch_pretty_view(slot, window, cx)
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
                            this.switch_message_view(slot, MessageView::Raw, window, cx)
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
                                    this.switch_message_view(slot, MessageView::Hex, window, cx)
                                })),
                            )
                            .when(slot.is_response(), |group| {
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
                                        this.switch_render_view(slot, window, cx)
                                    })),
                                )
                            })
                    })
                    .when(narrow_view_tabs, |group| {
                        let is_resp = slot.is_response();
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
                                                            slot,
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
                                                            this.switch_render_view(slot, window, cx);
                                                        });
                                                    }),
                                            )
                                        })
                                }),
                        )
                    })
                    .when(slot != EditorSlot::ExtMessage, |row| {
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
                                            this.toggle_message_hide_headers(slot, window, cx)
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
                                            this.toggle_message_soft_wrap(slot, window, cx)
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
                            this.toggle_message_non_printable(slot, window, cx)
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
                                            populate_message_actions_menu(
                                                menu,
                                                menu_owner.clone(),
                                                slot,
                                                unavailable,
                                                window,
                                                cx,
                                            )
                                        }),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn message_pair(
        &self,
        owner: &'static str,
        request: &MessageEditor,
        response: &MessageEditor,
        readonly: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (request_slot, response_slot) = EditorSlot::pair_for_owner(owner);
        let request_id = request_slot.id();
        let response_id = response_slot.id();
        let split_id: SharedString = if owner == "target" {
            "target-messages-hsplit".into()
        } else {
            format!("{owner}-hsplit").into()
        };
        let replace_readonly = readonly || self.is_message_replace_readonly(request_slot);
        let req_readonly = readonly || request.is_readonly_preview();
        editing::set_url_encode_as_you_type(self.url_encode_as_you_type_enabled(), cx);
        let request_pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.message_header(request_slot, "Request", request.view(), cx))
            .child(
                div().flex_1().min_h_0().child(
                    editing::message_editor(request)
                        .readonly(req_readonly)
                        .size_full()
                        .context_menu(self.request_context_menu(
                            request.state(),
                            readonly,
                        ))
                        .aria_label("请求消息编辑器"),
                ),
            )
            .child(request.render_search_bar_editable(
                request_id,
                replace_readonly || request.show_non_printable(),
                cx,
                move |this, action, window, cx| {
                    this.handle_message_search_action(request_slot, action, window, cx);
                },
            ));
        let response_pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.message_header(response_slot, "Response", response.view(), cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_response_editor_body(response_slot, response, cx)),
            )
            .child(
                response.render_search_bar(response_id, cx, move |this, action, window, cx| {
                    this.handle_message_search_action(response_slot, action, window, cx);
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

    pub(in crate::workbench) fn is_message_pretty(&self, slot: EditorSlot) -> bool {
        self.message_editor_ref(slot)
            .is_some_and(MessageEditor::pretty_active)
    }

    pub(in crate::workbench) fn is_message_non_printable(&self, slot: EditorSlot) -> bool {
        self.message_editor_ref(slot)
            .is_some_and(MessageEditor::show_non_printable)
    }

    pub(in crate::workbench) fn is_message_hide_headers(&self, slot: EditorSlot) -> bool {
        self.message_editor_ref(slot)
            .is_some_and(MessageEditor::hide_headers)
    }

    pub(in crate::workbench) fn is_message_render(&self, slot: EditorSlot) -> bool {
        self.message_editor_ref(slot)
            .is_some_and(|editor| editor.render_active)
    }

    pub(in crate::workbench) fn message_editor_ref(
        &self,
        slot: EditorSlot,
    ) -> Option<&MessageEditor> {
        match slot {
            EditorSlot::HistoryRequest => Some(&self.history_request),
            EditorSlot::HistoryResponse => Some(&self.history_response),
            EditorSlot::LoggerRequest => Some(&self.extra_tools.logger_request),
            EditorSlot::LoggerResponse => Some(&self.extra_tools.logger_response),
            EditorSlot::TargetRequest => Some(&self.target_pane.request),
            EditorSlot::TargetResponse => Some(&self.target_pane.response),
            EditorSlot::OrganiserRequest => Some(&self.organiser.request),
            EditorSlot::OrganiserResponse => Some(&self.organiser.response),
            EditorSlot::DashboardIssueRequest => Some(&self.audit_dashboard.issue_request_editor),
            EditorSlot::DashboardIssueResponse => Some(&self.audit_dashboard.issue_response_editor),
            EditorSlot::WebsocketPayload => Some(&self.websocket.payload),
            EditorSlot::ExtMessage => self.ext_import.message_editor_ref(),
            EditorSlot::Intercept => self
                .intercepts
                .iter()
                .find(|pending| Some(pending.id()) == self.selected_intercept)
                .map(|pending| &pending.editor),
            EditorSlot::RepeaterRequest => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .map(|editors| &editors.request),
            EditorSlot::RepeaterResponse => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .map(|editors| &editors.response),
            EditorSlot::WsRepeaterDraft => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get(&id))
                .map(|editors| &editors.draft),
            EditorSlot::WsRepeaterHistory => self
                .workspace
                .active_websocket_tab
                .and_then(|id| self.websocket_repeater.editors.get(&id))
                .map(|editors| &editors.history),
        }
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_repeater_provenance_menu(
        &self,
        slot: EditorSlot,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let is_resp = match slot {
            EditorSlot::RepeaterRequest => false,
            EditorSlot::RepeaterResponse => true,
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
            tab_id,
            flow.summary.id,
            flow.summary.revision,
            is_resp,
            steps_len,
            has_orig,
            crate::workbench::inspector::message_editor_rev_key(editor, cx),
        );
        let cached = self
            .editor_ui_caches
            .provenance_menu_cache
            .borrow()
            .iter()
            .find(|(k, _)| *k == cache_key)
            .map(|(_, val)| val.clone());
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
            {
                let mut entries = self.editor_ui_caches.provenance_menu_cache.borrow_mut();
                if let Some(pos) = entries.iter().position(|(k, _)| *k == cache_key) {
                    entries.remove(pos);
                } else if entries.len() >= 4 {
                    entries.remove(0);
                }
                entries.push((cache_key, computed.clone()));
            }
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

}

#[inline(never)]
fn populate_message_actions_menu(
    menu: PopupMenu,
    menu_owner: gpui_kit::WeakEntity<Workbench>,
    slot: EditorSlot,
    unavailable: bool,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let id = slot.id();
    let _ = menu_owner.update(cx, |this, cx| {
        this.focus_message_editor_by_id(slot, window, cx);
    });
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
    let is_resp_side = slot.is_response();
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
                                this.focus_message_editor_by_id(slot, window, cx);
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
                                this.focus_message_editor_by_id(slot, window, cx);
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
                                this.focus_message_editor_by_id(slot, window, cx);
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
                let _ = launcher_owner.update(cx, |this, cx| {
                    this.open_scan_launcher(window, cx);
                });
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
                let _ = browser_owner.update(cx, |this, cx| {
                    this.open_current_response_in_browser(window, cx);
                });
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
                let _ = copy_owner.update(cx, |this, cx| {
                    this.copy_selected_editor_text(slot, window, cx);
                });
            }),
    )
    .item(
        PopupMenuItem::new("复制美化报文 (Copy prettified)")
            .icon(IconName::Copy)
            .action(Box::new(CopyPrettified))
            .disabled(unavailable)
            .on_click(move |_, window, cx| {
                let _ = o8.update(cx, |this, cx| {
                    this.focus_message_editor_by_id(slot, window, cx);
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
                    this.save_selected_text_to_file(slot, window, cx);
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
                let _ = o_site_map.update(cx, |this, cx| this.add_to_site_map(window, cx));
            }),
    )
    .compact()
    .max_h(theme::scaled_px(412.))
    .scrollable(true)

}
