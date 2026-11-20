use gpui_kit::{
    FontWeight, Hsla, Pixels,
    base::{Checkbox, CheckboxIndicator, Radio},
    component::{
        Icon, IconName,
        button::{ButtonRounded, ButtonVariants},
        scroll::{Scrollbar, ScrollbarHandle, ScrollbarMode},
    },
};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WizardButtonSize {
    Browse,
    Footer,
    Compact,
}

pub(super) fn wizard_border() -> Hsla {
    rgb(if theme::is_dark() { 0x3c3f41 } else { 0xd4d4d4 }).into()
}

pub(super) fn wizard_divider() -> Hsla {
    rgb(if theme::is_dark() { 0x3c3f41 } else { 0xe0e0e0 }).into()
}

pub(super) fn wizard_table_header_bg() -> Hsla {
    rgb(if theme::is_dark() { 0x25272b } else { 0xeaeaea }).into()
}

pub(super) fn wizard_subtle_bg() -> Hsla {
    rgb(if theme::is_dark() { 0x25272b } else { 0xf7f7f7 }).into()
}

pub(super) fn wizard_heading_text() -> Hsla {
    rgb(if theme::is_dark() { 0xdfe1e5 } else { 0x141414 }).into()
}

pub(super) fn wizard_muted_text() -> Hsla {
    rgb(if theme::is_dark() { 0x9da2a8 } else { 0x5a5a5a }).into()
}

fn wizard_control_accent() -> Hsla {
    rgb(if theme::is_dark() { 0x4f8cc9 } else { 0x26649d }).into()
}

fn wizard_control_border() -> Hsla {
    rgb(if theme::is_dark() { 0x5a5d62 } else { 0xb8b8b8 }).into()
}

fn wizard_input_border() -> Hsla {
    rgb(if theme::is_dark() { 0x4e535b } else { 0xcfcfcf }).into()
}

fn wizard_input_inactive_bg() -> Hsla {
    rgb(if theme::is_dark() { 0x25272b } else { 0xf7f7f7 }).into()
}

pub(super) fn wizard_divider_line() -> gpui_kit::Div {
    div()
        .h(px(1.))
        .w_full()
        .flex_shrink_0()
        .bg(wizard_divider())
}

pub(super) fn wizard_field_label(text: &'static str, enabled: bool) -> gpui_kit::Div {
    let dark = theme::is_dark();
    div()
        .w(theme::scaled_px(44.))
        .flex_shrink_0()
        .text_right()
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .text_color(if enabled {
            rgb(if dark { 0xdfe1e5 } else { 0x1a1a1a }).into()
        } else {
            wizard_muted_text()
        })
        .child(text)
}

pub(super) fn wizard_input(state: &Entity<InputState>, enabled: bool, active: bool) -> Input {
    let mut input = Input::new(state)
        .small()
        .px(px(8.))
        .py_0()
        .text_size(theme::ui_font_size())
        .rounded(px(3.))
        .border_color(wizard_input_border())
        .bg(if enabled && active {
            theme::surface()
        } else {
            wizard_input_inactive_bg()
        })
        .disabled(!enabled);
    input.style().size.height = Some(theme::scaled_px(26.).max(px(24.)).into());
    input
}

pub(super) fn wizard_vertical_scrollbar<H: ScrollbarHandle + Clone>(
    id: &'static str,
    handle: &H,
    top_offset: Pixels,
) -> gpui_kit::Div {
    let dark = theme::is_dark();
    let track_bg: Hsla = rgb(if dark { 0x25272b } else { 0xf7f7f7 }).into();
    let track_border: Hsla = rgb(if dark { 0x3c3f41 } else { 0xe2e2e2 }).into();
    let thumb_bg: Hsla = rgb(if dark { 0x5c626b } else { 0xc8c8c8 }).into();
    let thumb_hover_bg: Hsla = rgb(if dark { 0x6b717a } else { 0xb0b0b0 }).into();
    div()
        .absolute()
        .top(top_offset)
        .right_0()
        .bottom_0()
        .w(px(11.))
        .child(
            Scrollbar::vertical(handle)
                .id(id)
                .viewport_from_layout()
                .mode(ScrollbarMode::Always)
                .styles(move |styles| {
                    styles
                        .track(|s| s.width(px(11.)).bg(track_bg).border_color(track_border))
                        .track_hover(|s| s.width(px(11.)).bg(track_bg).border_color(track_border))
                        .track_active(|s| s.width(px(11.)).bg(track_bg).border_color(track_border))
                        .thumb(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_bg)
                        })
                        .thumb_hover(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_hover_bg)
                        })
                        .thumb_active(|s| {
                            s.width(px(7.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_hover_bg)
                        })
                }),
        )
}

pub(super) fn wizard_neutral_button(
    id: &'static str,
    label: &'static str,
    tooltip: Option<&'static str>,
    enabled: bool,
    size: WizardButtonSize,
) -> Button {
    let dark = theme::is_dark();
    let (bg, hover_bg, border, fg): (Hsla, Hsla, Hsla, Hsla) = if enabled {
        if dark {
            (
                rgb(0x35383d).into(),
                rgb(0x40444a).into(),
                rgb(0x4e535b).into(),
                theme::foreground(),
            )
        } else {
            (
                rgb(0xeaeaea).into(),
                rgb(0xe0e0e0).into(),
                rgb(0xd0d0d0).into(),
                rgb(0x1a1a1a).into(),
            )
        }
    } else if dark {
        (
            rgb(0x2b2d30).into(),
            rgb(0x2b2d30).into(),
            rgb(0x3c3f41).into(),
            rgb(0x6b717a).into(),
        )
    } else {
        (
            rgb(0xefefef).into(),
            rgb(0xefefef).into(),
            rgb(0xdcdcdc).into(),
            rgb(0x8a8a8a).into(),
        )
    };
    let mut btn = Button::new(id)
        .ghost()
        .small()
        .compact()
        .rounded(ButtonRounded::Size(px(3.)))
        .flex_shrink_0()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .when(enabled, |b| {
            b.hover(move |s| s.bg(hover_bg).border_color(border).text_color(fg))
        })
        .disabled(!enabled)
        .accessibility_label(label)
        .when_some(tooltip, |b, tip| b.tooltip(tip))
        .child(
            div()
                .font_family(theme::ui_font_family())
                .text_size(theme::ui_font_size())
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .child(label),
        );
    btn = match size {
        WizardButtonSize::Browse => btn
            .w(theme::scaled_px(118.))
            .h(theme::scaled_px(26.).max(px(24.)))
            .px(theme::scaled_px(10.)),
        WizardButtonSize::Footer => btn
            .min_w(theme::scaled_px(86.))
            .h(theme::scaled_px(27.).max(px(25.)))
            .px(theme::scaled_px(14.)),
        WizardButtonSize::Compact => btn
            .h(theme::scaled_px(24.).max(px(22.)))
            .px(theme::scaled_px(12.)),
    };
    btn
}

pub(super) fn wizard_primary_button(
    id: &'static str,
    label: &'static str,
    tooltip: Option<&'static str>,
    enabled: bool,
    min_width: f32,
) -> Button {
    let dark = theme::is_dark();
    let (bg, hover_bg, border, fg): (Hsla, Hsla, Hsla, Hsla) = if enabled {
        (
            rgb(0xdb612f).into(),
            rgb(0xc45122).into(),
            rgb(0xdb612f).into(),
            rgb(0xffffff).into(),
        )
    } else if dark {
        (
            rgb(0x52372c).into(),
            rgb(0x52372c).into(),
            rgb(0x52372c).into(),
            rgb(0x9da2a8).into(),
        )
    } else {
        (
            rgb(0xedb8a3).into(),
            rgb(0xedb8a3).into(),
            rgb(0xedb8a3).into(),
            rgb(0xffffff).into(),
        )
    };
    Button::new(id)
        .ghost()
        .small()
        .compact()
        .rounded(ButtonRounded::Size(px(3.)))
        .h(theme::scaled_px(27.).max(px(25.)))
        .min_w(theme::scaled_px(min_width))
        .px(theme::scaled_px(16.))
        .flex_shrink_0()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .when(enabled, |b| {
            b.hover(move |s| s.bg(hover_bg).border_color(hover_bg).text_color(fg))
        })
        .disabled(!enabled)
        .accessibility_label(label)
        .when_some(tooltip, |b, tip| b.tooltip(tip))
        .child(
            div()
                .font_family(theme::ui_font_family())
                .text_size(theme::ui_font_size())
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .child(label),
        )
}

pub(super) fn wizard_radio(
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
    let accent = wizard_control_accent();
    Radio::new(id)
        .accessibility_label(label)
        .checked(checked)
        .disabled(!enabled)
        .track_focus(&focus)
        .flex()
        .items_center()
        .min_h(theme::scaled_px(22.))
        .py(px(1.))
        .gap(px(10.))
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .font_weight(FontWeight::BOLD)
        .text_color(wizard_heading_text())
        .rounded(px(3.))
        .when(enabled && focus.is_focused(window), |this| {
            this.bg(theme::selected())
        })
        .styles(|styles| styles.disabled(|style| style.opacity(0.48)))
        .child(
            div()
                .size(px(16.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(theme::surface())
                .when(checked, |circle| {
                    circle
                        .border_2()
                        .border_color(accent)
                        .child(div().size(px(6.)).rounded(px(3.)).bg(accent))
                })
                .when(!checked, |circle| {
                    circle.border_1().border_color(wizard_control_border())
                }),
        )
        .child(label)
}

pub(super) fn wizard_checkbox(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> Checkbox {
    let focus = window
        .use_keyed_state(id, cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let dark = theme::is_dark();
    let checked_fill: Hsla = if enabled {
        wizard_control_accent()
    } else if dark {
        rgb(0x4a4e54).into()
    } else {
        rgb(0xcbcbcb).into()
    };
    Checkbox::new(id)
        .accessibility_label(label)
        .checked(checked)
        .disabled(!enabled)
        .track_focus(&focus)
        .flex()
        .items_center()
        .min_h(theme::scaled_px(22.))
        .gap(px(8.))
        .px(px(2.))
        .py(px(1.))
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .font_weight(FontWeight::NORMAL)
        .text_color(if enabled {
            wizard_heading_text()
        } else {
            wizard_muted_text()
        })
        .rounded(px(3.))
        .when(enabled && focus.is_focused(window), |this| {
            this.bg(theme::selected())
        })
        .styles(|styles| styles.disabled(|style| style.opacity(0.75)))
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
                    checked_fill
                } else {
                    wizard_control_border()
                })
                .bg(if checked {
                    checked_fill
                } else {
                    theme::surface()
                })
                .rounded(px(3.))
                .when(checked, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(px(11.))
                            .text_color(rgb(0xffffff)),
                    )
                }),
        )
        .child(label)
}
