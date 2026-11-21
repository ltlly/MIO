use std::rc::Rc;

use gpui_kit::{ClickEvent, uniform_list};

use super::*;

impl CollaboratorView {
    pub(super) fn render_interaction_rows(
        &self,
        wb_entity: &Entity<Workbench>,
        interactions: Vec<(usize, Uuid)>,
        total_interactions: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let col_num = theme::scaled_px(48.);
        let col_time = theme::scaled_px(108.);
        let col_type = theme::scaled_px(76.);
        let col_ip = theme::scaled_px(156.);
        let col_payload = theme::scaled_px(210.);

        let header_cell = |label: &'static str, width: Option<gpui_kit::Pixels>, last: bool| {
            div()
                .h_full()
                .px(px(8.))
                .flex()
                .items_center()
                .when_some(width, |cell, w| cell.w(w).flex_shrink_0())
                .when(width.is_none(), |cell| cell.flex_1().min_w_0())
                .when(!last, |cell| {
                    cell.border_r_1().border_color(theme::border())
                })
                .truncate()
                .child(label)
        };

        let table_header = div()
            .id("collaborator-table-header")
            .test_support()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(header_bg)
            .text_color(header_fg)
            .text_size(theme::scaled_px(12.))
            .border_b_1()
            .border_color(theme::border())
            .child(header_cell("#", Some(col_num), false))
            .child(header_cell("时间", Some(col_time), false))
            .child(header_cell("类型", Some(col_type), false))
            .child(header_cell("Client IP", Some(col_ip), false))
            .child(header_cell("Payload", Some(col_payload), false))
            .child(header_cell("摘要", None, true));

        let table_body: AnyElement = if interactions.is_empty() {
            let empty_msg = if total_interactions == 0 {
                "尚无交互记录。点击「复制 Payload」生成载荷后，等待 HTTP 或 DNS 回调。"
            } else {
                "当前筛选条件下没有匹配的交互记录。"
            };
            div()
                .id("collaborator-interaction-list")
                .flex_1()
                .min_h_0()
                .bg(theme::surface())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .px(px(16.))
                .child(
                    div()
                        .id("collaborator-interaction-empty")
                        .test_support()
                        .text_size(theme::scaled_px(12.5))
                        .text_color(theme::foreground())
                        .child(empty_msg),
                )
                .child(
                    div()
                        .text_size(theme::scaled_px(11.5))
                        .text_color(theme::muted_foreground())
                        .child(LOCAL_LOOPBACK_NOTICE),
                )
                .into_any_element()
        } else {
            let selected_id = wb_entity.read(cx).collaborator_pane.selected_interaction;
            let total = total_interactions;
            let wb_weak = self.workbench.clone();
            let rows_rc = Rc::new(interactions);
            let row_count = rows_rc.len();
            uniform_list(
                "collaborator-interaction-list",
                row_count,
                move |range, _, cx| {
                    let Some(owner) = wb_weak.upgrade() else {
                        return Vec::new();
                    };
                    let view = owner.read(cx);
                    let dark = theme::is_dark();
                    let zebra_bg: gpui_kit::Hsla =
                        rgb(if dark { 0x25272b } else { 0xfafafa }).into();
                    let hover_bg: gpui_kit::Hsla =
                        rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
                    range
                        .filter_map(|index| {
                            let &(full_index, interaction_id) = rows_rc.get(index)?;
                            let interaction = view
                                .collaborator_pane
                                .status
                                .interactions
                                .get(full_index)
                                .filter(|item| item.id == interaction_id)?;
                            let label = view
                                .collaborator_pane
                                .token_label(interaction.token.as_deref().unwrap_or_default())
                                .unwrap_or_default()
                                .to_owned();
                            Some((index, full_index, interaction, label))
                        })
                        .map(|(visible_index, full_index, interaction, label)| {
                            let ordinal = total - full_index;
                            let (badge, badge_bg, badge_fg) =
                                protocol_badge_style(interaction.protocol);
                            let token = interaction.token.clone().unwrap_or_default();
                            let payload_col = if label.is_empty() {
                                token.clone()
                            } else {
                                format!("{token} ({label})")
                            };
                            let selected = selected_id == Some(interaction.id);
                            let interaction_id = interaction.id;
                            let right_view = wb_weak.clone();
                            let click_view = wb_weak.clone();
                            let menu_view = wb_weak.clone();
                            let row_bg = if selected {
                                theme::selected()
                            } else if visible_index % 2 == 1 {
                                zebra_bg
                            } else {
                                theme::surface()
                            };
                            div()
                                .id(SharedString::from(format!(
                                    "collaborator-interaction-{}",
                                    interaction.id
                                )))
                                .test_support()
                                .aria_label(format!(
                                    "#{} {} {} {} {}",
                                    ordinal,
                                    badge,
                                    token,
                                    interaction.peer_addr,
                                    interaction.summary
                                ))
                                .aria_selected(selected)
                                .w_full()
                                .h(theme::row_height())
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .bg(row_bg)
                                .when(!selected, |row| row.hover(move |s| s.bg(hover_bg)))
                                .border_b_1()
                                .border_color(theme::border())
                                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                                    let _ = right_view.update(cx, |this, cx| {
                                        if this.collaborator_pane.selected_interaction
                                            != Some(interaction_id)
                                        {
                                            this.select_collaborator_interaction(
                                                interaction_id,
                                                window,
                                                cx,
                                            );
                                        }
                                    });
                                })
                                .on_click(move |_: &ClickEvent, window, cx| {
                                    let _ = click_view.update(cx, |this, cx| {
                                        this.select_collaborator_interaction(
                                            interaction_id,
                                            window,
                                            cx,
                                        );
                                    });
                                })
                                .context_menu(move |menu, _, cx| {
                                    collaborator_row_menu(menu_view.clone(), interaction_id, menu, cx)
                                })
                                .child(
                                    div()
                                        .w(col_num)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(format!("{ordinal}")),
                                )
                                .child(
                                    div()
                                        .w(col_time)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(format_local_hms(interaction.observed_unix_ms)),
                                )
                                .child(
                                    div()
                                        .w(col_type)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .flex()
                                        .items_center()
                                        .child(
                                            div()
                                                .id(("collaborator-protocol-badge", full_index))
                                                .rounded(px(8.))
                                                .px(px(6.))
                                                .py(px(1.))
                                                .text_size(theme::scaled_px(10.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .bg(rgb(badge_bg))
                                                .text_color(rgb(badge_fg))
                                                .child(badge),
                                        ),
                                )
                                .child(
                                    div()
                                        .w(col_ip)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(interaction.peer_addr.clone()),
                                )
                                .child(
                                    div()
                                        .w(col_payload)
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .truncate()
                                        .font_family(theme::mono_font_family())
                                        .text_size(theme::scaled_px(11.5))
                                        .child(payload_col),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .px(px(8.))
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(interaction.summary.clone()),
                                )
                        })
                        .collect::<Vec<_>>()
                },
            )
            .w_full()
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(table_header)
            .child(table_body)
            .into_any_element()
    }
}

fn collaborator_row_menu(
    view: WeakEntity<Workbench>,
    interaction_id: Uuid,
    menu: PopupMenu,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(owner) = view.upgrade() else {
        return menu;
    };
    let state = owner.read(cx);
    let can_operate = state.collaborator_can_operate();
    let interaction = state
        .collaborator_pane
        .status
        .interactions
        .iter()
        .find(|i| i.id == interaction_id);
    let has_interaction = interaction.is_some();
    let is_http = interaction.is_some_and(|i| i.protocol == OastProtocol::Http);
    let raw_empty = interaction.is_none_or(|i| i.raw_request.is_empty());
    let has_payload = interaction
        .and_then(|i| i.token.as_deref())
        .is_some_and(|t| !t.is_empty());
    let file_picker_pending = state.file_picker_pending();

    let repeater_view = view.clone();
    let intruder_view = view.clone();
    let comparer_view = view.clone();
    let decoder_view = view.clone();
    let mio_ai_view = view.clone();
    let payload_view = view.clone();
    let delete_view = view.clone();
    let save_view = view;

    menu.item(
        PopupMenuItem::new("发送到 Repeater")
            .disabled(!can_operate || !is_http || raw_empty)
            .on_click(move |_, window, cx| {
                let _ = repeater_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_repeater(Some(interaction_id), window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Intruder")
            .disabled(!can_operate || !is_http || raw_empty)
            .on_click(move |_, window, cx| {
                let _ = intruder_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_intruder(Some(interaction_id), window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Comparer")
            .disabled(!has_interaction || raw_empty)
            .on_click(move |_, window, cx| {
                let _ = comparer_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_comparer(Some(interaction_id), window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Decoder")
            .disabled(!has_interaction || raw_empty)
            .on_click(move |_, window, cx| {
                let _ = decoder_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_decoder(Some(interaction_id), window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 MioAT")
            .disabled(!has_interaction)
            .on_click(move |_, _, cx| {
                let _ = mio_ai_view.update(cx, |v, cx| {
                    v.send_collaborator_interaction_to_mio_ai(Some(interaction_id), cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("复制 Payload")
            .disabled(!has_payload)
            .on_click(move |_, _, cx| {
                let _ = payload_view.update(cx, |v, cx| {
                    v.copy_collaborator_interaction_payload(Some(interaction_id), cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("删除此交互")
            .disabled(!has_interaction)
            .on_click(move |_, window, cx| {
                let _ = delete_view.update(cx, |v, cx| {
                    v.delete_collaborator_interaction(interaction_id, window, cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new("保存交互详情…")
            .disabled(!can_operate || !has_interaction || file_picker_pending)
            .on_click(move |_, window, cx| {
                let _ = save_view.update(cx, |v, cx| {
                    v.save_collaborator_interaction_details(Some(interaction_id), window, cx)
                });
            }),
    )
}
