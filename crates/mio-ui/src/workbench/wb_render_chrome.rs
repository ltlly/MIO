use super::*;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};

mod overlays;
mod tool_tabs;

pub(in crate::workbench) use overlays::GlobalSearchUi;

pub(in crate::workbench) fn button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
) -> Button {
    Button::new(id)
        .label(label)
        .small()
        .h(theme::control_height())
        .rounded(px(3.))
        .px(theme::scaled_px(10.))
}

pub(in crate::workbench) fn flat_button(
    id: impl Into<gpui_kit::ElementId>,
    inset_focus: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let id = id.into();
    let focus = window
        .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    gpui_kit::base::Button::new(id)
        .track_focus(&focus)
        .relative()
        .when(
            focus.is_focused(window) && window.last_input_was_keyboard(),
            |button| {
                if inset_focus {
                    // Kit's 3pt outer ring must stay inside edge-to-edge root tools.
                    button.child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .bottom(px(3.))
                            .left(px(3.))
                            .right(px(3.))
                            .focus_ring_style(window, cx),
                    )
                } else {
                    button.focus_ring_style(window, cx)
                }
            },
        )
}

#[allow(dead_code)]
pub(in crate::workbench) fn tool_tab(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    tool_tab_with_icon(id, label, None, selected, window, cx)
}

pub(in crate::workbench) fn tool_tab_with_icon(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    icon: Option<gpui_kit::assets::IconName>,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label = label.into();
    let dark = theme::is_dark();
    let normal_bg: gpui_kit::Hsla = if dark {
        if selected {
            rgb(0x35383d).into()
        } else {
            theme::toolbar_surface()
        }
    } else {
        if selected {
            theme::background()
        } else {
            theme::surface()
        }
    };
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(if selected { 0x35383d } else { 0x2f3237 }).into()
    } else {
        rgb(if selected { 0xfbfbfb } else { 0xeeeeee }).into()
    };
    flat_button(id, true, window, cx)
        .role(gpui_kit::accesskit::Role::Tab)
        .accessibility_label(label.clone())
        .aria_selected(selected)
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .h(theme::tool_tab_height())
        .flex_shrink_0()
        .px(theme::scaled_px(10.))
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .line_height(theme::scaled_px(17.))
        .text_color(if selected && !dark { rgb(0x000000).into() } else { theme::foreground() })
        .flex()
        .items_center()
        .gap(px(3.5))
        .bg(normal_bg)
        .hover(move |s| s.bg(hover_bg))
        .when_some(icon, |tab, ic| {
            tab.child(
                gpui_kit::component::Icon::new(ic)
                    .size(px(11.5 * theme::ui_scale()))
                    .text_color(if selected {
                        theme::accent()
                    } else {
                        theme::muted_foreground()
                    }),
            )
        })
        .child(label)
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(1.))
                    .h(px(3.))
                    .bg(theme::accent()),
            )
        })
}

#[allow(dead_code)]
pub(in crate::workbench) fn secondary_tab(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label = label.into();
    let dark = theme::is_dark();
    let base_bg = theme::toolbar_surface();
    let hover_bg: gpui_kit::Hsla = if selected {
        base_bg
    } else if dark {
        rgb(0x2f3237).into()
    } else {
        rgb(0xeeeeee).into()
    };
    flat_button(id, true, window, cx)
        .role(gpui_kit::accesskit::Role::Tab)
        .accessibility_label(label.clone())
        .aria_selected(selected)
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .h(theme::secondary_tab_height() - px(1.))
        .self_start()
        .flex_shrink_0()
        .px(theme::scaled_px(15.))
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .line_height(theme::scaled_px(17.))
        .text_color(if selected && !dark { rgb(0x000000).into() } else { theme::foreground() })
        .flex()
        .items_center()
        .gap(px(3.5))
        .bg(base_bg)
        .hover(move |s| s.bg(hover_bg))
        .child(label)
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(3.))
                    .bg(theme::accent()),
            )
        })
}

#[allow(dead_code)]
pub(in crate::workbench) fn secondary_tab_bar() -> gpui_kit::Div {
    div()
        .relative()
        .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(theme::border()))
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(theme::secondary_tab_height())
        .flex_shrink_0()
        .bg(theme::toolbar_surface())
}

pub(in crate::workbench) fn toolbar() -> gpui_kit::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .w_full()
        .min_w_0()
        .gap(theme::scaled_px(5.))
        .px(theme::scaled_px(10.))
        .py(theme::scaled_px(4.))
        .min_h(theme::toolbar_height())
        .flex_shrink_0()
        .bg(theme::toolbar_surface())
        .border_b_1()
        .border_color(theme::border())
}

pub(in crate::workbench) fn sidebar_rail() -> gpui_kit::Div {
    sidebar_rail_with_dock(false)
}

pub(in crate::workbench) fn sidebar_rail_with_dock(dock_left: bool) -> gpui_kit::Div {
    let dark = theme::is_dark();
    let divider_color: gpui_kit::Hsla = if dark {
        theme::border()
    } else {
        rgb(0xdfdfdf).into()
    };
    div()
        .w(theme::scaled_px(35.))
        .flex_shrink_0()
        .h_full()
        .relative()
        .pb(px(39.))
        .when(dock_left, |d| d.border_r_1())
        .when(!dock_left, |d| d.border_l_1())
        .border_color(theme::border())
        .bg(theme::toolbar_surface())
        .flex()
        .flex_col()
        .items_center()
        .child(
            Button::new("sidebar-rail-settings")
                .ghost()
                .small()
                .compact()
                .rounded(ButtonRounded::None)
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .w_full()
                .h(px(39.))
                .p_0()
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                        .size(px(18.))
                        .text_color(theme::foreground()),
                )
                .accessibility_label("用户设置")
                .tooltip("打开用户界面与消息编辑器设置")
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::OpenUserSettings), cx);
                }),
        )
        .child(
            div()
                .absolute()
                .bottom(px(38.))
                .left_0()
                .right_0()
                .h(px(1.))
                .bg(divider_color),
        )
}

fn xml_escape_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn sidebar_rail_rotated_label_svg_cached(
    label: &str,
    text_box_height: f32,
    dock_left: bool,
) -> std::sync::Arc<[u8]> {
    type RailSvgCache = HashMap<String, Vec<(u32, bool, std::sync::Arc<[u8]>)>>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<RailSvgCache>> =
        std::sync::OnceLock::new();
    let h = text_box_height.round().max(32.0);
    let h_bits = h.to_bits();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some(entries) = guard.get(label)
        && let Some((_, _, bytes)) = entries
            .iter()
            .find(|(bits, left, _)| *bits == h_bits && *left == dock_left)
    {
        return std::sync::Arc::clone(bytes);
    }
    let escaped = xml_escape_attr(label);
    let transform = if dock_left {
        format!("translate(15, {h}) rotate(-90)")
    } else {
        "translate(4, 0) rotate(90)".to_owned()
    };
    let generated: std::sync::Arc<[u8]> = std::sync::Arc::from(
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"19\" height=\"{h}\" viewBox=\"0 0 19 {h}\"><g transform=\"{transform}\"><text x=\"0\" y=\"0\" font-family=\"{font}, sans-serif\" font-size=\"15\" fill=\"#000000\">{escaped}</text></g></svg>",
            font = theme::svg_ui_font_family(),
        )
        .into_bytes(),
    );
    if let Ok(mut guard) = cache.lock() {
        if guard.len() >= 128 && !guard.contains_key(label) {
            guard.clear();
        }
        let entries = guard.entry(label.to_owned()).or_default();
        if !entries
            .iter()
            .any(|(bits, left, _)| *bits == h_bits && *left == dock_left)
        {
            entries.push((h_bits, dock_left, std::sync::Arc::clone(&generated)));
        }
    }
    generated
}

#[cfg(test)]
pub(in crate::workbench) fn sidebar_rail_rotated_label_svg(
    label: &str,
    text_box_height: f32,
    dock_left: bool,
) -> Vec<u8> {
    sidebar_rail_rotated_label_svg_cached(label, text_box_height, dock_left).to_vec()
}

pub(in crate::workbench) fn sidebar_rail_tab(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    a11y_label: &'static str,
    icon: gpui_kit::assets::IconName,
    selected: bool,
    disabled: bool,
    tip: impl Into<SharedString>,
) -> Button {
    sidebar_rail_tab_with_dock(id, label, a11y_label, icon, selected, disabled, false, tip)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::workbench) fn sidebar_rail_tab_with_dock(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    a11y_label: &'static str,
    icon: gpui_kit::assets::IconName,
    selected: bool,
    disabled: bool,
    dock_left: bool,
    tip: impl Into<SharedString>,
) -> Button {
    let is_inspector = a11y_label.eq_ignore_ascii_case("Inspector")
        || label.eq_ignore_ascii_case("Inspector");
    let is_notes = a11y_label.eq_ignore_ascii_case("Notes")
        || a11y_label == "备注"
        || a11y_label == "备注与状态"
        || label.starts_with("Notes");
    let is_explanations = a11y_label.eq_ignore_ascii_case("Explanations")
        || label.eq_ignore_ascii_case("Explanations");
    let is_custom_actions = a11y_label.eq_ignore_ascii_case("Custom actions")
        || label.eq_ignore_ascii_case("Custom actions");
    let has_note_dot = is_notes && label.contains('•');
    let (slot_height, rotated_text, text_box_height): (f32, std::borrow::Cow<'static, str>, f32) =
        if is_inspector {
            (138.0_f32, std::borrow::Cow::Borrowed("Inspector"), 64.0_f32)
        } else if is_notes {
            (115.0_f32, std::borrow::Cow::Borrowed("Notes"), 41.0_f32)
        } else if is_explanations {
            (
                154.0_f32,
                std::borrow::Cow::Borrowed("Explanations"),
                88.0_f32,
            )
        } else if is_custom_actions {
            (
                162.0_f32,
                std::borrow::Cow::Borrowed("Custom actions"),
                102.0_f32,
            )
        } else {
            let clean: String = label
                .chars()
                .map(|ch| if ch == '\n' { ' ' } else { ch })
                .collect();
            (115.0_f32, std::borrow::Cow::Owned(clean), 56.0_f32)
        };
    let dark = theme::is_dark();
    let selected_fill: gpui_kit::Hsla = rgb(if dark { 0x2e436e } else { 0xcadaf0 }).into();
    let selected_fg: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x000000).into()
    };
    let border_bottom_color: gpui_kit::Hsla = if dark {
        theme::border()
    } else {
        rgb(0xdfdfdf).into()
    };
    let fg_color = if selected {
        selected_fg
    } else {
        theme::foreground()
    };
    let svg_bytes =
        sidebar_rail_rotated_label_svg_cached(rotated_text.as_ref(), text_box_height, dock_left);
    let tab_icon = if is_inspector {
        gpui_kit::component::Icon::default()
            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.35\"><rect x=\"2.1\" y=\"1.5\" width=\"4.6\" height=\"13\" rx=\"2.3\"/><circle cx=\"4.4\" cy=\"11.6\" r=\"1.35\" fill=\"currentColor\" stroke=\"none\"/><rect x=\"9.3\" y=\"1.5\" width=\"4.6\" height=\"13\" rx=\"2.3\"/><circle cx=\"11.6\" cy=\"4.4\" r=\"1.35\" fill=\"currentColor\" stroke=\"none\"/></svg>")
            .size(theme::scaled_px(17.))
            .text_color(fg_color)
    } else if is_explanations {
        gpui_kit::component::Icon::default()
            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.35\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M6 12.2h4M6.6 14h2.8M8 2a4 4 0 0 0-2.5 7.1c.5.4.8 1 .9 1.6h3.2c.1-.6.4-1.2.9-1.6A4 4 0 0 0 8 2z\"/></svg>")
            .size(theme::scaled_px(17.))
            .text_color(fg_color)
    } else if is_custom_actions {
        gpui_kit::component::Icon::default()
            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.35\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M9 1.8L4.2 8.5H8L7 14.2L11.8 7.5H8L9 1.8Z\"/></svg>")
            .size(theme::scaled_px(17.))
            .text_color(fg_color)
    } else {
        gpui_kit::component::Icon::new(icon)
            .size(theme::scaled_px(18.))
            .text_color(fg_color)
    };
    Button::new(id)
        .ghost()
        .rounded(ButtonRounded::None)
        .w_full()
        .h(theme::scaled_px(slot_height))
        .flex_shrink_0()
        .px_0()
        .py_0()
        .relative()
        .selected(selected)
        .toggled(selected)
        .disabled(disabled)
        .accessibility_label(a11y_label)
        .tooltip(tip)
        .when(selected, |b| b.bg(selected_fill).text_color(selected_fg))
        .child(
            div()
                .size_full()
                .pt(theme::scaled_px(15.))
                .flex()
                .flex_col()
                .items_center()
                .justify_start()
                .gap(theme::scaled_px(15.))
                .child(tab_icon)
                .child(
                    gpui_kit::svg()
                        .data(svg_bytes.as_ref())
                        .w(theme::scaled_px(19.))
                        .h(theme::scaled_px(text_box_height))
                        .flex_shrink_0()
                        .text_color(fg_color),
                )
                .when(has_note_dot, |col| {
                    col.child(
                        div()
                            .size(theme::scaled_px(6.))
                            .rounded(theme::scaled_px(3.))
                            .bg(theme::accent()),
                    )
                }),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(1.))
                .bg(border_bottom_color),
        )
}
