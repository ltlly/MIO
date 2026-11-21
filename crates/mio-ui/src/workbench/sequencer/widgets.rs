use gpui_kit::assets::IconName;
use gpui_kit::base::Radio;
use gpui_kit::component::Icon;

use super::*;

pub(super) fn sequencer_table_header_bg() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0x25272b } else { 0xf1f1f1 }).into()
}

pub(super) fn sequencer_divider() -> gpui_kit::Div {
    div()
        .w_full()
        .h(px(1.))
        .my(px(12.))
        .flex_shrink_0()
        .bg(theme::border())
}

pub(super) fn sequencer_help_icon(
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
) -> AnyElement {
    let dark = theme::is_dark();
    Button::new(id)
        .ghost()
        .compact()
        .p_0()
        .size(px(20.))
        .child(
            Icon::new(IconName::CircleQuestionMark)
                .size(px(18.))
                .text_color(if dark {
                    theme::foreground()
                } else {
                    rgb(0x1f1f1f).into()
                }),
        )
        .accessibility_label(label)
        .tooltip(tooltip)
        .into_any_element()
}

pub(super) fn sequencer_section(
    help_id: &'static str,
    help_label: &'static str,
    help_tooltip: &'static str,
    title: &'static str,
    subtitle: Option<&'static str>,
    body: AnyElement,
) -> AnyElement {
    let dark = theme::is_dark();
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .w(px(20.))
                .pt(px(1.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(sequencer_help_icon(help_id, help_label, help_tooltip)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(theme::scaled_px(14.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if dark {
                            theme::foreground()
                        } else {
                            rgb(0x000000).into()
                        })
                        .child(title),
                )
                .when_some(subtitle, |col, text| {
                    col.child(
                        div()
                            .mt(px(5.))
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child(text),
                    )
                })
                .child(div().mt(px(10.)).w_full().child(body)),
        )
        .into_any_element()
}

pub(super) fn sequencer_list_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label: SharedString = label.into();
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = if dark {
        rgb(0x34373c).into()
    } else {
        rgb(0xefefef).into()
    };
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(0x3e4248).into()
    } else {
        rgb(0xe3e3e3).into()
    };
    let border: gpui_kit::Hsla = if dark {
        rgb(0x4e535b).into()
    } else {
        rgb(0xd4d4d4).into()
    };
    let fg: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x1a1a1a).into()
    };
    flat_button(id, false, window, cx)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(theme::scaled_px(26.))
        .px(theme::scaled_px(12.))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .when(enabled, |btn| btn.hover(move |s| s.bg(hover_bg)))
        .font_family(theme::ui_font_family())
        .text_size(theme::scaled_px(12.5))
        .font_weight(FontWeight::BOLD)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(label)
}

pub(super) fn sequencer_stack_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label: SharedString = label.into();
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = if dark {
        rgb(0x34373c).into()
    } else {
        rgb(0xefefef).into()
    };
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(0x3e4248).into()
    } else {
        rgb(0xe3e3e3).into()
    };
    let border: gpui_kit::Hsla = if dark {
        rgb(0x4e535b).into()
    } else {
        rgb(0xd4d4d4).into()
    };
    let fg: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x1a1a1a).into()
    };
    flat_button(id, false, window, cx)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(theme::scaled_px(26.))
        .px(theme::scaled_px(12.))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .when(enabled, |btn| btn.hover(move |s| s.bg(hover_bg)))
        .font_family(theme::ui_font_family())
        .text_size(theme::scaled_px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .styles(|styles| styles.disabled(|style| style.opacity(0.55)))
        .child(label)
}

pub(super) fn sequencer_outline_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label: SharedString = label.into();
    let dark = theme::is_dark();
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(0x34373c).into()
    } else {
        rgb(0xf2f2f2).into()
    };
    let border: gpui_kit::Hsla = if dark {
        rgb(0x4e535b).into()
    } else {
        rgb(0xd2d2d2).into()
    };
    let fg: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x1a1a1a).into()
    };
    flat_button(id, false, window, cx)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(theme::scaled_px(26.))
        .px(theme::scaled_px(14.))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(theme::surface())
        .when(enabled, |btn| btn.hover(move |s| s.bg(hover_bg)))
        .font_family(theme::ui_font_family())
        .text_size(theme::scaled_px(12.5))
        .font_weight(FontWeight::BOLD)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(label)
}

pub(super) fn sequencer_primary_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label: SharedString = label.into();
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = if enabled {
        rgb(0xdb612f).into()
    } else if dark {
        rgb(0x35383d).into()
    } else {
        rgb(0xdfdfdf).into()
    };
    let fg: gpui_kit::Hsla = if enabled {
        rgb(0xffffff).into()
    } else if dark {
        rgb(0x888d96).into()
    } else {
        rgb(0x555555).into()
    };
    flat_button(id, false, window, cx)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(theme::scaled_px(28.))
        .min_w(theme::scaled_px(136.))
        .px(theme::scaled_px(18.))
        .rounded(px(3.))
        .bg(bg)
        .when(!enabled, |btn| {
            btn.border_1()
                .border_color(rgb(if dark { 0x454950 } else { 0xd4d4d4 }))
        })
        .when(enabled, |btn| btn.hover(|s| s.bg(rgb(0xc55426))))
        .font_family(theme::ui_font_family())
        .text_size(theme::scaled_px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(label)
}

pub(super) fn sequencer_radio(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> Radio {
    let focus = window
        .use_keyed_state(id, cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let dark = theme::is_dark();
    let ring_border: gpui_kit::Hsla = if !enabled {
        if dark {
            rgb(0x4b4f57).into()
        } else {
            rgb(0xd4d4d4).into()
        }
    } else if checked {
        theme::accent()
    } else if dark {
        rgb(0x5c616a).into()
    } else {
        rgb(0xb8b8b8).into()
    };
    let dot_bg: gpui_kit::Hsla = if !enabled {
        if dark {
            rgb(0x666b75).into()
        } else {
            rgb(0xc6c6c6).into()
        }
    } else {
        theme::accent()
    };
    Radio::new(id)
        .accessibility_label(label)
        .checked(checked)
        .disabled(!enabled)
        .track_focus(&focus)
        .w(theme::scaled_px(116.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .min_h(theme::scaled_px(24.))
        .py(px(1.))
        .gap(px(8.))
        .text_size(theme::ui_font_size())
        .font_weight(FontWeight::NORMAL)
        .text_color(theme::foreground())
        .rounded(px(3.))
        .when(enabled && focus.is_focused(window), |this| {
            this.bg(theme::selected())
        })
        .child(
            div()
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .border_1()
                .border_color(ring_border)
                .bg(theme::surface())
                .when(checked, |this| {
                    this.child(div().size(px(6.)).rounded(px(3.)).bg(dot_bg))
                }),
        )
        .child(label)
}
