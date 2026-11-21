use std::rc::Rc;

use gpui_kit::{ClickEvent, uniform_list};

use super::*;

struct TargetItemRowStyle<'a> {
    selected_id: Option<HistoryEntryId>,
    selected_ids: &'a HashSet<HistoryEntryId>,
    col_widths: [f32; SITEMAP_COL_COUNT],
    visible_cols: &'a [usize],
    flex_col_ix: usize,
    stripe_bg: gpui_kit::Hsla,
    sel_border: gpui_kit::Hsla,
    weak: &'a WeakEntity<Workbench>,
    visible_ids: &'a Rc<Vec<HistoryEntryId>>,
}

fn render_target_item_row(
    index: usize,
    row: &HistoryRow,
    summary: &mime::TargetRowSummary,
    hl_color: Option<u8>,
    note_preview: String,
    style: &TargetItemRowStyle<'_>,
) -> AnyElement {
    let selected = style.selected_ids.contains(&row.id())
        || (style.selected_ids.is_empty() && style.selected_id == Some(row.id()));
    let has_note = !note_preview.trim().is_empty();
    let row_label = format!(
        "{} {} {} {} {} {} {}{}",
        summary.host,
        summary.method,
        summary.url,
        summary.status,
        summary.length,
        summary.mime,
        summary.title,
        if has_note {
            format!(" {note_preview}")
        } else {
            String::new()
        }
    );
    let row_bg = if selected {
        theme::selected()
    } else if let Some(bg) = hl_color.and_then(crate::history::highlight_background) {
        bg
    } else if index % 2 == 1 {
        style.stripe_bg
    } else {
        theme::surface()
    };
    let right_view = style.weak.clone();
    let click_view = style.weak.clone();
    let menu_view = style.weak.clone();
    let row_right = row.clone();
    let row_click = row.clone();
    let click_order = Rc::clone(style.visible_ids);
    let row_cells: Vec<_> = style
        .visible_cols
        .iter()
        .map(|&col_ix| {
            let w = style.col_widths[col_ix];
            let text = match col_ix {
                0 => summary.host.clone(),
                1 => summary.method.clone(),
                2 => summary.url.clone(),
                3 => summary.params.clone(),
                4 => summary.status.clone(),
                5 => summary.length.clone(),
                6 => summary.mime.clone(),
                7 => summary.title.clone(),
                8 if has_note => note_preview.clone(),
                _ => String::new(),
            };
            if col_ix == style.flex_col_ix {
                div()
                    .flex_1()
                    .min_w(theme::scaled_px(w))
                    .px(px(6.))
                    .truncate()
                    .child(text)
            } else {
                div()
                    .w(theme::scaled_px(w))
                    .flex_shrink_0()
                    .px(px(6.))
                    .truncate()
                    .child(text)
            }
        })
        .collect();
    div()
        .id(("target-sitemap-row", index))
        .test_support()
        .aria_label(row_label)
        .aria_selected(selected)
        .w_full()
        .h(theme::row_height())
        .flex()
        .items_center()
        .gap_0()
        .cursor_pointer()
        .bg(row_bg)
        .when(selected, |el| el.border_1().border_color(style.sel_border))
        .when(!selected && hl_color.is_some(), |el| {
            el.text_color(crate::history::highlight_foreground(
                hl_color.unwrap_or(0),
            ))
        })
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            let _ = right_view.update(cx, |this, cx| {
                this.handle_target_row_right_down(&row_right, window, cx);
            });
        })
        .on_click(move |event: &ClickEvent, window, cx| {
            let shift = event.modifiers().shift;
            let toggle = event.modifiers().secondary(cx);
            let _ = click_view.update(cx, |this, cx| {
                this.handle_target_row_click(&row_click, &click_order, shift, toggle, window, cx);
            });
        })
        .context_menu(move |menu, window, cx| {
            target_item_menu(menu_view.clone(), menu, window, cx)
        })
        .children(row_cells)
        .into_any_element()
}

impl Workbench {
    fn resolve_cached_target_rows(
        &self,
        cx: &Context<Self>,
    ) -> Rc<Vec<(usize, mime::TargetRowSummary)>> {
        let selected_id = self.target_pane.selected_id;
        let sort = self.target_pane.item_sort;
        let loaded_count = self.target_pane.items.len();
        let hide_not_found = self.target_pane.hide_not_found;
        let status_classes = self.target_pane.status_classes;
        let hide_binary = self.target_pane.hide_css_image_binary;

        let items_ptr = self.target_pane.items.as_ptr() as usize;
        let first_item = self
            .target_pane
            .items
            .first()
            .map(|r| (r.id(), row_status_code(r), row_response_length(r)));
        let last_item = self
            .target_pane
            .items
            .last()
            .map(|r| (r.id(), row_status_code(r), row_response_length(r)));
        let query_revision = self.target_pane.query_revision;
        let history_query_revision = self.query_revision;
        let row_metadata_len = self.history.read(cx).delegate().row_metadata.len();
        let selected_response_len = self.target_pane.selected_response_bytes.len();
        let response_rev =
            crate::workbench::inspector::message_editor_rev_key(&self.target_pane.response, cx);
        let hidden_item_ids_len = self.target_pane.hidden_item_ids.len();
        let hidden_item_ids_sig =
            self.target_pane
                .hidden_item_ids
                .iter()
                .fold(0u64, |acc, id| {
                    let raw = match *id {
                        HistoryEntryId::Live(flow_id) => flow_id.as_u128(),
                        HistoryEntryId::External { source_id, item_id } => {
                            source_id.as_u128() ^ item_id.as_u128().rotate_left(17)
                        }
                    };
                    let mut x = (raw as u64) ^ ((raw >> 64) as u64);
                    x ^= x >> 30;
                    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
                    x ^= x >> 27;
                    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
                    acc.wrapping_add(x ^ (x >> 31))
                });
        let cache_hit = self.target_pane.items_cache.borrow().as_ref().and_then(|c| {
            let k = &c.key;
            (k.items_ptr == items_ptr
                && k.items_len == loaded_count
                && k.first_item == first_item
                && k.last_item == last_item
                && k.query_revision == query_revision
                && k.history_query_revision == history_query_revision
                && k.row_metadata_len == row_metadata_len
                && k.selected_id == selected_id
                && k.selected_response_len == selected_response_len
                && k.response_rev == response_rev
                && k.hidden_item_ids_len == hidden_item_ids_len
                && k.hidden_item_ids_sig == hidden_item_ids_sig
                && k.hidden_prefixes == self.target_pane.tree_state.hidden_prefixes
                && k.sort == sort
                && k.hide_not_found == hide_not_found
                && k.status_classes == status_classes
                && k.hide_css_image_binary == hide_binary)
                .then(|| Rc::clone(&c.rows))
        });
        if let Some(hit) = cache_hit {
            return hit;
        }
        let loaded_response_owned: Option<Vec<u8>> = (!self
            .target_pane
            .selected_response_bytes
            .is_empty())
        .then(|| self.target_pane.selected_response_bytes.clone())
        .or_else(|| {
            let raw = self.target_pane.response.raw.read(cx).value();
            (!raw.is_empty()).then(|| raw.as_bytes().to_vec())
        });
        let row_metadata = &self.history.read(cx).delegate().row_metadata;
        let built = Rc::new(build_sorted_target_rows_with_metadata(
            &self.target_pane.items,
            selected_id,
            loaded_response_owned.as_deref(),
            Some(row_metadata),
            &self.target_pane.tree_state.hidden_prefixes,
            &self.target_pane.hidden_item_ids,
            hide_not_found,
            status_classes,
            hide_binary,
            sort,
        ));
        *self.target_pane.items_cache.borrow_mut() = Some(CachedTargetItems {
            key: TargetItemsCacheKey {
                items_ptr,
                items_len: loaded_count,
                first_item,
                last_item,
                query_revision,
                history_query_revision,
                row_metadata_len,
                selected_id,
                selected_response_len,
                response_rev,
                hidden_item_ids_len,
                hidden_item_ids_sig,
                hidden_prefixes: self.target_pane.tree_state.hidden_prefixes.clone(),
                sort,
                hide_not_found,
                status_classes,
                hide_css_image_binary: hide_binary,
            },
            rows: Rc::clone(&built),
        });
        built
    }

    pub(super) fn render_target_items(
        &mut self,
        view_ctx: &TargetViewContext<'_>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected_id = self.target_pane.selected_id;
        let sort = self.target_pane.item_sort;
        let loaded_count = self.target_pane.items.len();
        let total_items = self.target_pane.total_items;
        let loading = self.target_pane.site_map_loading;
        let items_v_scroll = view_ctx.items_v_scroll.clone();
        let items_h_scroll = view_ctx.items_h_scroll.clone();
        let cached_rows = self.resolve_cached_target_rows(cx);

        let row_count = cached_rows.len();
        let weak = cx.weak_entity();
        let col_widths = self.target_pane.item_col_widths;
        let col_visible = self.target_pane.item_col_visible;
        let visible_cols: Vec<usize> = (0..SITEMAP_COL_COUNT)
            .filter(|&col_ix| col_visible[col_ix])
            .collect();
        let flex_col_ix = if col_visible[2] {
            2
        } else {
            visible_cols.last().copied().unwrap_or(0)
        };
        let total_min_w = theme::scaled_px(
            visible_cols
                .iter()
                .map(|&col_ix| col_widths[col_ix])
                .sum::<f32>()
                .max(260.),
        );
        let selected_ids_rc = Rc::new(self.target_pane.selected_ids.clone());
        let visible_ids: Rc<Vec<HistoryEntryId>> = Rc::new(
            cached_rows
                .iter()
                .filter_map(|(item_index, _)| {
                    self.target_pane.items.get(*item_index).map(HistoryRow::id)
                })
                .collect(),
        );
        let list_rows = Rc::clone(&cached_rows);
        let stripe_bg = if theme::table_zebra_stripe() {
            table_stripe_bg()
        } else {
            theme::surface()
        };
        let sel_border = selected_row_border();
        let corner_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x2b2d30 } else { 0xf7f7f7 }).into();
        let corner_border: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x3c3f41 } else { 0xe0e0e0 }).into();

        let list = uniform_list("target-sitemap-items", row_count, move |range, _, cx| {
            let Some(owner) = weak.upgrade() else {
                return Vec::new();
            };
            if !loading
                && loaded_count > 0
                && loaded_count < total_items
                && range.end.saturating_add(30) >= row_count
            {
                let load_owner = weak.clone();
                cx.defer(move |cx| {
                    let _ = load_owner.update(cx, |this, cx| {
                        this.load_more_target_site_map(loaded_count, cx);
                    });
                });
            }
            let workbench = owner.read(cx);
            let items = &workbench.target_pane.items;
            let delegate = workbench.history.read(cx).delegate();
            let live_highlights = delegate.live_highlights.borrow();
            let live_notes = delegate.live_notes.borrow();
            let row_style = TargetItemRowStyle {
                selected_id,
                selected_ids: &selected_ids_rc,
                col_widths,
                visible_cols: &visible_cols,
                flex_col_ix,
                stripe_bg,
                sel_border,
                weak: &weak,
                visible_ids: &visible_ids,
            };
            let rendered = range
                .filter_map(|index| {
                    let (item_index, summary) = list_rows.get(index)?;
                    let row = items.get(*item_index)?;
                    Some((index, row, summary))
                })
                .map(|(index, row, summary)| {
                    let hl_color = match row {
                        HistoryRow::ExtProxy(archive) => {
                            archive.highlight.as_ref().map(|h| h.color)
                        }
                        HistoryRow::Live(flow) => live_highlights.get(&flow.id).map(|h| h.color),
                    }
                    .filter(|c| (1..=9).contains(c));
                    let note_preview = match row {
                        HistoryRow::ExtProxy(archive) => archive
                            .notes_preview
                            .replace("\r\n", " ")
                            .replace(['\r', '\n', '\t'], " "),
                        HistoryRow::Live(flow) => live_notes
                            .get(&flow.id)
                            .map(|(_, text)| {
                                text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ")
                            })
                            .unwrap_or_default(),
                    };
                    render_target_item_row(index, row, summary, hl_color, note_preview, &row_style)
                })
                .collect::<Vec<_>>();
            drop(live_notes);
            drop(live_highlights);
            rendered
        })
        .track_scroll(&items_v_scroll)
        .size_full();

        let header = self.render_target_items_header(sort, cached_rows, view_ctx, cx);
        let h_scroll_body = div()
            .id("target-sitemap-items-hwrap")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .track_scroll(&items_h_scroll)
            .child(
                div()
                    .h_full()
                    .w_full()
                    .min_w(total_min_w)
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h_0().w_full().child(list)),
            );

        let v_scroll_col = div()
            .w(px(12.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .h(theme::table_header_height())
                    .flex_shrink_0()
                    .border_b_1()
                    .border_l_1()
                    .border_color(theme::border())
                    .bg(table_header_bg()),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(mio_vertical_scrollbar_track(
                        "target-sitemap-items-vscroll",
                        &items_v_scroll,
                    )),
            );

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(h_scroll_body)
                    .child(v_scroll_col),
            )
            .child(
                div()
                    .w_full()
                    .h(px(12.))
                    .flex_shrink_0()
                    .flex()
                    .child(mio_horizontal_scrollbar_track(
                        "target-sitemap-items-hscroll",
                        &items_h_scroll,
                    ))
                    .child(
                        div()
                            .w(px(12.))
                            .h_full()
                            .flex_shrink_0()
                            .border_t_1()
                            .border_l_1()
                            .border_color(corner_border)
                            .bg(corner_bg),
                    ),
            )
            .into_any_element()
    }
}
