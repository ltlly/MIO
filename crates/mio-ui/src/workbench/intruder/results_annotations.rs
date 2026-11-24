use gpui_kit::component::{
    menu::{PopupMenu, PopupMenuItem},
    table::TableState,
};
use gpui_kit::{Bounds, WindowOptions, size};

use super::*;
use crate::workbench::intruder::results_view::ResultTarget;

#[path = "results_annotations/notes_window.rs"]
mod notes_window;
pub(in crate::workbench::intruder) use notes_window::ResultNotesWindow;

const MAX_RESULT_NOTE_BYTES: usize = 64 * 1024;
const PREVIEW_NOTE_BYTES: usize = 256;

pub(in crate::workbench::intruder) const HIGHLIGHT_NAMES: [&str; 10] = [
    "None", "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Pink", "Magenta", "Gray",
];

#[derive(Clone, Copy)]
pub(in crate::workbench::intruder) struct PendingHighlight {
    pub(in crate::workbench::intruder) view_id: Uuid,
    pub(in crate::workbench::intruder) ordinal: u64,
    pub(in crate::workbench::intruder) expected_revision: u64,
    pub(in crate::workbench::intruder) highlight: Option<u8>,
}

#[derive(Clone, Copy)]
pub(in crate::workbench::intruder) struct PendingNotes {
    pub(in crate::workbench::intruder) operation_id: Uuid,
    pub(in crate::workbench::intruder) view_id: Uuid,
    pub(in crate::workbench::intruder) ordinal: u64,
    pub(in crate::workbench::intruder) expected_revision: u64,
}

pub(in crate::workbench::intruder) fn notes_preview_from_bytes(notes: &[u8]) -> String {
    String::from_utf8_lossy(&notes[..notes.len().min(PREVIEW_NOTE_BYTES)]).into_owned()
}

pub(in crate::workbench::intruder) fn can_annotate_result(
    workbench: &Workbench,
    target: &ResultTarget,
    ordinal: u64,
) -> bool {
    if !target.active(workbench)
        || !workbench.backend_available()
        || workbench.shutdown_requested
        || workbench.transition.is_some()
        || workbench.quit_attempt.is_some()
        || workbench.file_picker_pending()
    {
        return false;
    }
    let view = &workbench.intruder.results.views[&target.view_id];
    if view.summary.as_ref().is_some_and(|run| {
        run.baseline
            .is_some_and(|baseline| !baseline.completed && baseline.ordinal == ordinal)
    }) {
        return false;
    }
    view.query
        .view_row(ordinal)
        .is_some_and(|row| row.result.is_some())
}

pub(in crate::workbench::intruder) fn append_annotation_menu<
    D: gpui_kit::component::table::TableDelegate,
>(
    target: &ResultTarget,
    ordinal: Option<u64>,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<TableState<D>>,
) -> PopupMenu {
    let target_ordinals: Vec<u64> = if window.window_handle() == target.owner
        && let Some(ordinal) = ordinal
        && let Some(owner) = target.workbench.upgrade()
    {
        let wb = owner.read(cx);
        let candidates: Vec<u64> = wb
            .intruder
            .results
            .views
            .get(&target.view_id)
            .filter(|v| v.selected_ordinals.len() > 1 && v.selected_ordinals.contains(&ordinal))
            .map(|v| v.selected_ordinals.iter().copied().collect())
            .unwrap_or_else(|| vec![ordinal]);
        candidates
            .into_iter()
            .filter(|&ord| can_annotate_result(wb, target, ord))
            .collect()
    } else {
        Vec::new()
    };
    let can_annotate = !target_ordinals.is_empty();
    let hl_ordinals = target_ordinals.clone();
    let submenu = PopupMenu::build(window, cx, |menu, _, _| {
        (0u8..=9).fold(menu.min_w(theme::scaled_px(116.)), |menu, color| {
            let name = HIGHLIGHT_NAMES[usize::from(color)];
            let target = target.clone();
            let batch_ordinals = hl_ordinals.clone();
            menu.item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .id(("intruder-highlight-color", usize::from(color)))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::MenuItem)
                        .aria_label(format!("Highlight {name}"))
                        .w_full()
                        .min_h(theme::scaled_px(26.))
                        .mx_neg_2()
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when_some(
                            crate::history::highlight_background(color),
                            |this, background| this.bg(background),
                        )
                        .text_color(crate::history::highlight_foreground(color))
                        .child(div().w(px(10.)).h(px(10.)).when(color == 0, |this| {
                            this.border_1().border_dashed().border_color(rgb(0x888888))
                        }))
                        .child(name)
                })
                .disabled(!can_annotate)
                .on_click(move |_, window, cx| {
                    if !batch_ordinals.is_empty() {
                        let ords = batch_ordinals.clone();
                        target.update(window, cx, |workbench, _, cx| {
                            workbench.set_intruder_results_highlight_batch(
                                target.view_id,
                                &ords,
                                color,
                                cx,
                            );
                        });
                    }
                }),
            )
        })
    });
    let notes_target = target.clone();
    let primary_notes_ordinal = ordinal
        .filter(|ord| target_ordinals.contains(ord))
        .or_else(|| target_ordinals.first().copied());
    menu.separator()
        .item(PopupMenuItem::submenu("Highlight", submenu).disabled(!can_annotate))
        .item(
            PopupMenuItem::new("编辑备注…")
                .disabled(!can_annotate)
                .on_click(move |_, window, cx| {
                    if let Some(ordinal) = primary_notes_ordinal {
                        notes_target.update(window, cx, |workbench, window, cx| {
                            workbench.open_intruder_result_notes(
                                notes_target.view_id,
                                ordinal,
                                window,
                                cx,
                            );
                        });
                    }
                }),
        )
}

impl Workbench {
    pub(in crate::workbench::intruder) fn set_intruder_result_highlight(
        &mut self,
        view_id: Uuid,
        ordinal: u64,
        color: u8,
        cx: &mut Context<Self>,
    ) {
        let batch: Vec<u64> = self
            .intruder
            .results
            .views
            .get(&view_id)
            .filter(|v| v.selected_ordinals.len() > 1 && v.selected_ordinals.contains(&ordinal))
            .map(|v| v.selected_ordinals.iter().copied().collect())
            .unwrap_or_else(|| vec![ordinal]);
        self.set_intruder_results_highlight_batch(view_id, &batch, color, cx);
    }

    pub(in crate::workbench::intruder) fn set_intruder_results_highlight_batch(
        &mut self,
        view_id: Uuid,
        ordinals: &[u64],
        color: u8,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        if color > 9
            || ordinals.is_empty()
            || !self.backend_available()
            || self.shutdown_requested
            || self.transition.is_some()
            || self.quit_attempt.is_some()
            || self.file_picker_pending()
        {
            return;
        }
        let Some(view) = self.intruder.results.views.get(&view_id) else {
            return;
        };
        if view.discarded || self.intruder_result_close_pending(view.run_id) {
            return;
        }
        let mut entries = Vec::new();
        for &ordinal in ordinals {
            if view.summary.as_ref().is_some_and(|run| {
                run.baseline
                    .is_some_and(|baseline| !baseline.completed && baseline.ordinal == ordinal)
            }) {
                continue;
            }
            let Some(row) = view.query.view_row(ordinal) else {
                continue;
            };
            if row.result.is_none() {
                continue;
            }
            entries.push((ordinal, row.highlight_revision));
        }
        if entries.is_empty() {
            return;
        }
        let run_id = view.run_id;
        let highlight = (color != 0).then_some(color);
        let operation_id = Uuid::new_v4();
        let single_entry = (entries.len() == 1).then(|| entries[0]);
        if !self.enqueue(
            Command::SetIntruderResultHighlight {
                project,
                run_id,
                entries,
                highlight,
                operation_id,
            },
            cx,
        ) {
            let error = self
                .error
                .clone()
                .unwrap_or_else(|| "Highlight 修改未入队；当前列表保留。".into());
            if let Some(view) = self.intruder.results.views.get_mut(&view_id) {
                view.query.error = Some(error);
            }
            cx.notify();
            return;
        }
        if let Some((ordinal, expected_revision)) = single_entry {
            self.intruder.results.pending_highlights.insert(
                operation_id,
                PendingHighlight {
                    view_id,
                    ordinal,
                    expected_revision,
                    highlight,
                },
            );
        }
    }

    pub(in crate::workbench::intruder) fn open_intruder_result_notes(
        &mut self,
        view_id: Uuid,
        ordinal: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let Some(view) = self.intruder.results.views.get(&view_id) else {
            return;
        };
        let target = ResultTarget {
            workbench: cx.weak_entity(),
            project,
            view_id,
            run_id: view.run_id,
            owner: view.owner,
            editor_id: view.request.raw.entity_id(),
        };
        if !can_annotate_result(self, &target, ordinal) {
            return;
        }
        let scale = theme::ui_scale();
        let content_w = if scale > 1.0 {
            theme::scaled_px(420.)
        } else {
            px(420.)
        };
        let content_h = if scale > 1.0 {
            theme::scaled_px(260.)
        } else {
            px(260.)
        };
        let desired_h = if cfg!(test) {
            content_h
        } else {
            let bar_h = if scale > 1.0 {
                theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                    .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
            } else {
                px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
            };
            content_h + bar_h
        };
        let raw_dimensions = size(content_w, desired_h);
        let (bounds, display_id) = if let Some(display) = window.display(cx) {
            let visible = display.visible_bounds();
            let dimensions = size(
                raw_dimensions.width.min(visible.size.width),
                raw_dimensions.height.min(visible.size.height),
            );
            let mut bounds =
                Bounds::centered_at(window.window_bounds().get_bounds().center(), dimensions);
            bounds.origin.x = bounds.origin.x.clamp(
                visible.origin.x,
                (visible.right() - dimensions.width).max(visible.origin.x),
            );
            bounds.origin.y = bounds.origin.y.clamp(
                visible.origin.y,
                (visible.bottom() - dimensions.height).max(visible.origin.y),
            );
            (bounds, Some(display.id()))
        } else {
            (Bounds::centered(None, raw_dimensions, cx), None)
        };
        cx.defer(move |cx| {
            let Some(owner) = target.workbench.upgrade() else {
                return;
            };
            if !can_annotate_result(owner.read(cx), &target, ordinal) {
                return;
            }
            if let Some(existing) = owner.read(cx).intruder.results.views[&view_id].notes_window {
                if owner.read(cx).intruder.results.views[&view_id].notes_ordinal == Some(ordinal)
                    && crate::chrome::activate_popup_window(existing, cx)
                {
                    return;
                }
                let _ = existing.update(cx, |_, window, _| window.remove_window());
                owner.update(cx, |workbench, _| {
                    if let Some(view) = workbench.intruder.results.views.get_mut(&view_id)
                        && view.notes_window == Some(existing)
                    {
                        view.notes_window = None;
                        view.notes_ordinal = None;
                    }
                });
            }
            let (initial_text, expected_revision, request_number, truncated_preview) = {
                let workbench = owner.read(cx);
                let Some(view) = workbench.intruder.results.views.get(&view_id) else {
                    return;
                };
                let Some(row) = view.query.view_row(ordinal) else {
                    return;
                };
                let request_number = view
                    .summary
                    .as_ref()
                    .and_then(|run| run.request_number(ordinal));
                let cached = workbench
                    .intruder
                    .results
                    .cached_notes
                    .get(&(view.run_id, ordinal))
                    .filter(|(revision, _)| *revision == row.notes_revision)
                    .map(|(_, text)| text.clone());
                let truncated = cached.is_none()
                    && (row.notes_preview.len() >= PREVIEW_NOTE_BYTES
                        || row.notes_preview.contains('\u{fffd}'));
                (
                    cached.unwrap_or_else(|| row.notes_preview.clone()),
                    row.notes_revision,
                    request_number,
                    truncated,
                )
            };
            let opened = crate::chrome::open_popup_window(
                WindowOptions {
                    display_id,
                    ..crate::chrome::mio_window_options(bounds, false, false)
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        ResultNotesWindow::new(
                            target.clone(),
                            ordinal,
                            request_number,
                            initial_text,
                            expected_revision,
                            truncated_preview,
                            window,
                            cx,
                        )
                    })
                },
            );
            owner.update(cx, |workbench, cx| match opened {
                Ok((handle, _)) if can_annotate_result(workbench, &target, ordinal) => {
                    let view = workbench.intruder.results.views.get_mut(&view_id).unwrap();
                    view.notes_window = Some(handle);
                    view.notes_ordinal = Some(ordinal);
                    view.notes_outcome = None;
                    cx.notify();
                }
                Ok((handle, _)) => {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
                Err(error) => workbench.fail(format!("无法打开 Intruder 结果备注：{error}"), cx),
            });
        });
    }

    pub(in crate::workbench::intruder) fn submit_intruder_result_notes(
        &mut self,
        view_id: Uuid,
        ordinal: u64,
        expected_notes_revision: u64,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<Uuid, String> {
        let project = self.project.ok_or("当前项目已关闭；备注草稿保留。")?;
        if !self.backend_available()
            || self.shutdown_requested
            || self.transition.is_some()
            || self.quit_attempt.is_some()
            || self.file_picker_pending()
        {
            return Err("当前项目或后台状态不可保存备注；草稿保留。".into());
        }
        let view = self
            .intruder
            .results
            .views
            .get(&view_id)
            .ok_or("结果窗口已关闭；备注未保存。")?;
        if view.discarded || self.intruder_result_close_pending(view.run_id) {
            return Err("此攻击结果正在关闭或已丢弃；备注未保存。".into());
        }
        if text.len() > MAX_RESULT_NOTE_BYTES {
            return Err("结果备注不能超过 64 KiB（65536 字节）；草稿仍保留。".into());
        }
        let run_id = view.run_id;
        let extra_targets: Vec<(u64, u64)> = if view.selected_ordinals.len() > 1
            && view.selected_ordinals.contains(&ordinal)
        {
            view.selected_ordinals
                .iter()
                .copied()
                .filter(|&ord| ord != ordinal)
                .filter_map(|ord| {
                    if view.summary.as_ref().is_some_and(|run| {
                        run.baseline
                            .is_some_and(|baseline| !baseline.completed && baseline.ordinal == ord)
                    }) {
                        return None;
                    }
                    let row = view.query.view_row(ord)?;
                    row.result.as_ref()?;
                    Some((ord, row.notes_revision))
                })
                .collect()
        } else {
            Vec::new()
        };
        let notes_bytes = text.into_bytes();
        let operation_id = Uuid::new_v4();
        if !self.enqueue(
            Command::SetIntruderResultNotes {
                project,
                run_id,
                ordinal,
                expected_notes_revision,
                notes: notes_bytes.clone(),
                operation_id,
            },
            cx,
        ) {
            return Err(self
                .error
                .clone()
                .unwrap_or_else(|| "结果备注保存命令未入队；草稿仍保留。".into()));
        }
        let view = self.intruder.results.views.get_mut(&view_id).unwrap();
        view.notes_outcome = None;
        self.intruder.results.pending_notes.insert(
            operation_id,
            PendingNotes {
                operation_id,
                view_id,
                ordinal,
                expected_revision: expected_notes_revision,
            },
        );
        for (other_ordinal, other_expected_rev) in extra_targets {
            let extra_op = Uuid::new_v4();
            if self.enqueue(
                Command::SetIntruderResultNotes {
                    project,
                    run_id,
                    ordinal: other_ordinal,
                    expected_notes_revision: other_expected_rev,
                    notes: notes_bytes.clone(),
                    operation_id: extra_op,
                },
                cx,
            ) {
                self.intruder.results.pending_notes.insert(
                    extra_op,
                    PendingNotes {
                        operation_id: extra_op,
                        view_id,
                        ordinal: other_ordinal,
                        expected_revision: other_expected_rev,
                    },
                );
            }
        }
        cx.notify();
        Ok(operation_id)
    }
}
