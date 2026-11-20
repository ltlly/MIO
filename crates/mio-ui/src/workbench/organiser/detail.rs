use super::table::HIGHLIGHT_NAMES;
use super::*;
use gpui_kit::assets::IconName;

impl Workbench {
    pub(in crate::workbench) fn render_organiser_notes_card(
        &self,
        selected: Option<OrganiserItem>,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (number, status, highlight, notes_len) =
            selected
                .as_ref()
                .map_or((None, OrganiserStatus::Unknown, 0u8, 0usize), |item| {
                    (
                        Some(item.number),
                        item.workflow_status,
                        item.highlight.unwrap_or(0),
                        item.notes.len(),
                    )
                });
        let title = number
            .map(|number| format!("备注与状态 · #{number}"))
            .unwrap_or_else(|| "备注与状态".into());
        let status_buttons = OrganiserStatus::ALL.map(|candidate| {
            button(
                SharedString::from(format!(
                    "organiser-card-status-{}",
                    status_button_suffix(candidate)
                )),
                candidate.label(),
            )
            .small()
            .min_h(theme::scaled_px(24.))
            .h_auto()
            .py(px(2.))
            .px(px(6.))
            .tooltip(format!("将当前条目标记为「{}」", candidate.label()))
            .selected(status == candidate)
            .disabled(!editable || selected.is_none())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_selected_organiser_status(candidate, window, cx);
            }))
        });
        let highlight_buttons = (0u8..=9).map(|color| {
            let target = (color > 0).then_some(color);
            let color_name = HIGHLIGHT_NAMES[usize::from(color)];
            let tip: SharedString = if color == 0 {
                "清除高亮颜色".into()
            } else {
                format!("设置高亮颜色：{color_name}").into()
            };
            div()
                .id(("organiser-card-highlight", usize::from(color)))
                .test_support()
                .role(gpui_kit::accesskit::Role::MenuItem)
                .aria_label(color_name)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .w(theme::scaled_px(18.).max(px(18.)))
                .h(theme::scaled_px(18.).max(px(18.)))
                .rounded_full()
                .cursor_pointer()
                .border_1()
                .border_color(if highlight == color {
                    theme::accent()
                } else {
                    theme::border()
                })
                .when_some(
                    crate::history::highlight_background(color),
                    |this, background| this.bg(background),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_selected_organiser_highlight(target, window, cx);
                }))
        });
        div()
            .id("organiser-notes-card")
            .test_support()
            .w(px(270.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                toolbar().child(title).child(div().flex_1()).child(
                    button("organiser-save-notes", "保存备注")
                        .primary()
                        .icon(IconName::Save)
                        .tooltip("保存当前条目的分析备注到项目工作区")
                        .disabled(!editable || selected.is_none())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_selected_organiser_notes(window, cx);
                        })),
                ),
            )
            .child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .children(status_buttons),
            )
            .child(
                div()
                    .px(px(8.))
                    .pb(px(6.))
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .children(highlight_buttons),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Textarea::new(&self.organiser.notes)
                        .readonly(!editable || selected.is_none())
                        .size_full()
                        .bordered(false)
                        .aria_label("Organiser 备注编辑器"),
                ),
            )
            .child(
                div()
                    .id("organiser-notes-meta")
                    .test_support()
                    .px(px(8.))
                    .py(px(5.))
                    .text_size(theme::scaled_px(11.))
                    .text_color(theme::muted_foreground())
                    .border_t_1()
                    .border_color(theme::border())
                    .child(format!(
                        "状态：{} · 高亮：{} · 备注：{notes_len} B / 64 KiB",
                        status.label(),
                        HIGHLIGHT_NAMES[usize::from(highlight)],
                    )),
            )
            .into_any_element()
    }
}

fn status_button_suffix(status: OrganiserStatus) -> &'static str {
    match status {
        OrganiserStatus::Unknown => "unknown",
        OrganiserStatus::New => "new",
        OrganiserStatus::InProgress => "in-progress",
        OrganiserStatus::Postponed => "postponed",
        OrganiserStatus::Done => "done",
        OrganiserStatus::Ignored => "ignored",
    }
}
