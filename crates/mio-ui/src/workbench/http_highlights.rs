use super::*;
use gpui_kit::WeakEntity;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use mio_core::{ExtHttpHighlight, ExtHttpHighlightChange, ExtHttpHighlightError, Uuid};

mod receipts;

const MAX_HIGHLIGHTS: usize = 500;
type SelectedHighlights = Vec<((Uuid, Uuid), ExtHttpHighlight)>;

#[derive(Default)]
pub(super) struct HttpHighlights {
    drafts: HashMap<(Uuid, Uuid), HighlightDraft>,
    pending: Option<PendingHighlights>,
}

struct HighlightDraft {
    stored: ExtHttpHighlight,
    color: u8,
    error: Option<String>,
}

struct PendingHighlights {
    operation: Uuid,
    changes: Vec<ExtHttpHighlightChange>,
    cancel: CancellationToken,
}

impl Workbench {
    pub(crate) fn http_highlight_menu(
        owner: WeakEntity<Self>,
        ids: Vec<HistoryEntryId>,
        label: String,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<HistoryDelegate>>,
    ) -> PopupMenu {
        let Some(view) = owner.upgrade() else {
            return menu;
        };
        let Some(project) = view.read(cx).project else {
            return menu;
        };
        let submenu = PopupMenu::build(window, cx, |menu, _, _| {
            (0..=9).fold(menu.min_w(theme::scaled_px(88.)), |menu, color| {
                let label = label.clone();
                let owner = owner.clone();
                let ids = ids.clone();
                menu.item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .id(("http-highlight-color", usize::from(color)))
                            .test_support()
                            .role(gpui_kit::accesskit::Role::MenuItem)
                            .aria_label(format!(
                                "Highlight {}",
                                [
                                    "None", "Red", "Orange", "Yellow", "Green", "Cyan", "Blue",
                                    "Pink", "Magenta", "Gray"
                                ][usize::from(color)]
                            ))
                            .w_full()
                            .h(theme::scaled_px(26.).max(px(26.)))
                            .mx_neg_2()
                            .px(theme::scaled_px(8.))
                            .flex()
                            .items_center()
                            .gap(theme::scaled_px(6.))
                            .when_some(
                                crate::history::highlight_background(color),
                                |this, background| this.bg(background),
                            )
                            .text_color(crate::history::highlight_foreground(color))
                            .child(
                                div()
                                    .w(theme::scaled_px(10.))
                                    .h(theme::scaled_px(10.))
                                    .when(color == 0, |this| {
                                        this.border_1().border_dashed().border_color(rgb(0x888888))
                                    }),
                            )
                            .child(label.clone())
                    })
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            if view.project == Some(project) {
                                view.set_http_highlights(&ids, color, cx);
                            }
                        });
                    }),
                )
            })
        });
        menu.item(PopupMenuItem::submenu("Highlight", submenu))
    }

    pub(in crate::workbench) fn reset_http_highlights(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = self.http_highlights.pending.take() {
            pending.cancel.cancel();
        }
        self.http_highlights.drafts.clear();
        let delegate = self.history.read(cx).delegate();
        delegate.live_highlights.borrow_mut().clear();
        delegate.live_highlight_errors.borrow_mut().clear();
        cx.notify();
    }

    pub(in crate::workbench) fn http_highlights_disconnected(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = &self.http_highlights.pending {
            pending.cancel.cancel();
        }
        for draft in self.http_highlights.drafts.values_mut() {
            draft.error = Some("后台服务已断开；Highlight 保存结果未确认，当前选择仍保留。".into());
        }
        self.update_http_highlight_rows(cx);
    }

    pub(in crate::workbench) fn http_highlights_pending(&self) -> bool {
        self.http_highlights.pending.is_some()
    }

    pub(in crate::workbench) fn http_highlights_unsaved(&self) -> bool {
        !self.http_highlights.drafts.is_empty()
    }

    pub(in crate::workbench) fn snapshot_http_highlights(
        &self,
    ) -> Result<Vec<ExtHttpHighlightChange>, String> {
        if self.http_highlights_pending() {
            return Err("Highlight 保存仍在确认；请等待当前回执后保存项目。".into());
        }
        Ok(self
            .http_highlights
            .drafts
            .iter()
            .map(|(id, draft)| ExtHttpHighlightChange {
                source_id: id.0,
                item_id: id.1,
                expected_highlight_revision: draft.stored.revision,
                color: draft.color,
            })
            .collect())
    }

    pub(crate) fn set_http_highlights(
        &mut self,
        ids: &[HistoryEntryId],
        color: u8,
        cx: &mut Context<Self>,
    ) {
        let selected = match self.capture_http_highlights(ids, color, cx) {
            Ok(selected) => selected,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            for ((source, item), stored) in &selected {
                if source.is_nil() {
                    delegate.live_highlights.borrow_mut().insert(
                        *item,
                        ExtHttpHighlight {
                            revision: stored.revision,
                            color,
                        },
                    );
                    delegate.live_highlight_errors.borrow_mut().remove(item);
                }
            }
            for row in delegate.cached_rows_mut() {
                if let HistoryRow::ExtProxy(row) = row
                    && let Some((_, stored)) = selected
                        .iter()
                        .find(|(id, _)| *id == (row.item.source_id, row.item.id))
                {
                    row.highlight = Some(ExtHttpHighlight {
                        revision: stored.revision,
                        color,
                    });
                    row.highlight_error = None;
                }
            }
            table.refresh(cx);
        });
        cx.notify();
        self.send_http_highlights(false, cx);
    }

    fn capture_http_highlights(
        &mut self,
        ids: &[HistoryEntryId],
        color: u8,
        cx: &App,
    ) -> Result<SelectedHighlights, String> {
        if !self.backend_available() {
            return Err("后台服务不可用；未修改 Highlight。".into());
        }
        if color > 9 || ids.is_empty() || ids.len() > MAX_HIGHLIGHTS {
            return Err("Highlight 颜色无效，或选择数量不在 1 至 500 条范围内。".into());
        }
        let mut changes = Vec::with_capacity(ids.len());
        let mut seen = HashSet::with_capacity(ids.len());
        let delegate = self.history.read(cx).delegate();
        for id in ids {
            let (source_id, item_id, is_live) = match *id {
                HistoryEntryId::External { source_id, item_id } => (source_id, item_id, false),
                HistoryEntryId::Live(flow_id) => (Uuid::nil(), flow_id, true),
            };
            let key = (source_id, item_id);
            if (!is_live && source_id.is_nil()) || item_id.is_nil() || !seen.insert(key) {
                return Err("Highlight 来源或条目 ID 为空或重复；未修改任何记录。".into());
            }
            let stored = self
                .http_highlights
                .drafts
                .get(&key)
                .map(|draft| draft.stored.clone())
                .or_else(|| match delegate.row_by_id(*id) {
                    Some(HistoryRow::ExtProxy(row)) => row.highlight.clone(),
                    Some(HistoryRow::Live(flow)) => Some(
                        delegate
                            .live_highlights
                            .borrow()
                            .get(&flow.id)
                            .cloned()
                            .unwrap_or(ExtHttpHighlight {
                                revision: 0,
                                color: 0,
                            }),
                    ),
                    _ => None,
                })
                .filter(|stored| stored.color <= 9)
                .ok_or("所选记录的原始 Highlight 未识别或未读取；未修改任何记录。")?;
            changes.push((key, stored));
        }
        let new_drafts = changes
            .iter()
            .filter(|(id, stored)| {
                !self.http_highlights.drafts.contains_key(id) && stored.color != color
            })
            .count();
        if self.http_highlights.drafts.len() + new_drafts > MAX_HIGHLIGHTS {
            return Err(
                "已有过多 Highlight 待保存；最多同时保留 500 条修改，未修改本次选择。".into(),
            );
        }
        for (id, stored) in &changes {
            let pending = self
                .http_highlights
                .pending
                .as_ref()
                .is_some_and(|pending| {
                    pending
                        .changes
                        .iter()
                        .any(|change| (change.source_id, change.item_id) == *id)
                });
            let saving = self.project_saves_in_flight > 0
                && self
                    .project_http_highlights_save
                    .as_ref()
                    .is_some_and(|batch| {
                        batch
                            .iter()
                            .any(|change| (change.source_id, change.item_id) == *id)
                    });
            if color == stored.color && !pending && !saving {
                self.http_highlights.drafts.remove(id);
            } else {
                self.http_highlights.drafts.insert(
                    *id,
                    HighlightDraft {
                        stored: stored.clone(),
                        color,
                        error: None,
                    },
                );
            }
        }
        Ok(changes)
    }

    pub(in crate::workbench) fn send_http_highlights(
        &mut self,
        retry: bool,
        cx: &mut Context<Self>,
    ) {
        if self.http_highlights_pending()
            || self.notes_save_wait.is_some()
            || self.pending_project_save.is_some()
            || self.choosing_save_as
            || self.project_saves_in_flight > 0
            || self.transition.is_some()
        {
            return;
        }
        let Some(project) = self.project else { return };
        let changes: Vec<_> = self
            .http_highlights
            .drafts
            .iter()
            .filter(|(_, draft)| retry || draft.error.is_none())
            .map(|(id, draft)| ExtHttpHighlightChange {
                source_id: id.0,
                item_id: id.1,
                expected_highlight_revision: draft.stored.revision,
                color: draft.color,
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        let operation = Uuid::new_v4();
        let cancel = CancellationToken::new();
        if !self.enqueue(
            Command::SetExtHttpHighlights {
                project,
                operation_id: operation,
                changes: changes.clone(),
                cancel: cancel.clone(),
            },
            cx,
        ) {
            for change in changes {
                if let Some(draft) = self
                    .http_highlights
                    .drafts
                    .get_mut(&(change.source_id, change.item_id))
                {
                    draft.error = self.error.clone();
                }
            }
            self.update_http_highlight_rows(cx);
            return;
        }
        for change in &changes {
            self.http_highlights
                .drafts
                .get_mut(&(change.source_id, change.item_id))
                .unwrap()
                .error = None;
        }
        self.http_highlights.pending = Some(PendingHighlights {
            operation,
            changes,
            cancel: cancel.clone(),
        });
        let owner = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            let _ = owner.update(cx, |view, cx| {
                if view.project != Some(project)
                    || view
                        .http_highlights
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.operation != operation)
                {
                    return;
                }
                cancel.cancel();
                let message = "Highlight 保存确认超时；已请求取消，当前选择保留，等待实际结果。";
                for change in &view.http_highlights.pending.as_ref().unwrap().changes {
                    if let Some(draft) = view
                        .http_highlights
                        .drafts
                        .get_mut(&(change.source_id, change.item_id))
                    {
                        draft.error = Some(message.into());
                    }
                }
                view.update_http_highlight_rows(cx);
                view.fail(message, cx);
            });
        })
        .detach();
        cx.notify();
    }
}

fn valid_receipts(
    changes: &[ExtHttpHighlightChange],
    stored: &[(Uuid, Uuid, ExtHttpHighlight)],
) -> bool {
    changes.len() == stored.len()
        && changes.iter().all(|change| {
            stored
                .iter()
                .filter(|(source, item, value)| {
                    (*source, *item) == (change.source_id, change.item_id)
                        && value.color == change.color
                        && (value.revision == change.expected_highlight_revision
                            || Some(value.revision)
                                == change.expected_highlight_revision.checked_add(1))
                })
                .count()
                == 1
        })
}

#[cfg(test)]
mod shared_tests;
#[cfg(test)]
mod tests;
