use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_intruder_detail_or_annotation_event(
        &mut self,
        event: &Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            Event::IntruderSpec {
                project,
                run_id,
                selection_revision,
                result,
            } if self.project == Some(*project) => {
                let Some(view) = self.intruder.results.views.values_mut().find(|view| {
                    view.run_id == *run_id && view.spec_pending == Some(*selection_revision)
                }) else {
                    return false;
                };
                view.spec_pending = None;
                match result {
                    Ok(spec)
                        if view
                            .summary
                            .as_ref()
                            .is_none_or(|run| run.spec_revision == spec.revision) =>
                    {
                        view.spec = Some(spec.clone());
                        view.spec_error = None;
                    }
                    Ok(_) => {
                        view.spec_error =
                            Some("冻结配置 revision 与此运行不一致，请重试读取。".into())
                    }
                    Err(error) => view.spec_error = Some(error.clone()),
                }
            }
            Event::FlowDetail {
                project,
                selection_revision,
                record,
                ..
            } if self.project == Some(*project) => {
                let Some(view) = self.intruder.results.views.values_mut().find(|view| {
                    view.detail_pending.is_some_and(|read| {
                        read.selection_revision == *selection_revision
                            && read.flow_id == record.summary.id
                    })
                }) else {
                    return false;
                };
                let read = view.detail_pending.take().unwrap();
                if record.summary.revision < read.flow_revision {
                    view.detail_error =
                        Some("结果消息 revision 已改变，请重试读取最新内容。".into());
                } else {
                    view.load_detail(record, window, cx);
                    self.refresh_intruder_grep_summaries(cx);
                }
            }
            Event::IntruderResultNotes {
                project,
                operation_id,
                run_id,
                ordinal,
                notes_revision,
                notes,
                result,
            } if self.project == Some(*project) => {
                let pending = self.intruder.results.pending_notes.remove(operation_id);
                let outcome = result.as_ref().map_err(Clone::clone).and_then(|()| {
                    if *notes_revision == 0
                        || *notes_revision > i64::MAX as u64
                        || notes.len() > 64 * 1024
                        || pending.is_some_and(|pending| {
                            pending.ordinal != *ordinal
                                || *notes_revision <= pending.expected_revision
                        })
                    {
                        Err("Intruder 结果备注回执的序号或 revision 不一致；草稿仍保留。".into())
                    } else {
                        Ok(())
                    }
                });
                match outcome {
                    Ok(()) => {
                        if let Ok(text) = std::str::from_utf8(notes) {
                            self.intruder
                                .results
                                .cached_notes
                                .insert((*run_id, *ordinal), (*notes_revision, text.to_owned()));
                        } else {
                            self.intruder
                                .results
                                .cached_notes
                                .remove(&(*run_id, *ordinal));
                        }
                        let preview = results_annotations::notes_preview_from_bytes(notes);
                        let mut requery = Vec::new();
                        for (&view_id, view) in &mut self.intruder.results.views {
                            if view.run_id != *run_id || view.discarded {
                                continue;
                            }
                            view.query.update_rows(*ordinal, |row| {
                                if row.notes_revision <= *notes_revision {
                                    row.notes_revision = *notes_revision;
                                    row.notes_preview = preview.clone();
                                }
                            });
                            view.query.error = None;
                            if pending.is_some_and(|pending| pending.view_id == view_id) {
                                view.record_notes_outcome(*operation_id, Ok(()));
                            }
                            let (filter, sort) = view.query.requested();
                            if filter.only_with_notes || filter.only_highlighted {
                                let offset = view.query.page.as_ref().map_or(0, |page| page.offset);
                                requery.push((view_id, offset, filter, sort));
                            }
                        }
                        for (view_id, offset, filter, sort) in requery {
                            self.query_intruder_result_view(
                                view_id, offset, filter, sort, window, cx,
                            );
                        }
                    }
                    Err(error) => {
                        if let Some(pending) = pending
                            && let Some(view) =
                                self.intruder.results.views.get_mut(&pending.view_id)
                            && view.run_id == *run_id
                            && !view.discarded
                        {
                            view.record_notes_outcome(*operation_id, Err(error));
                        } else {
                            return false;
                        }
                    }
                }
            }
            Event::IntruderResultHighlight {
                project,
                operation_id,
                run_id,
                updated,
                highlight,
                result,
            } if self.project == Some(*project) => {
                let pending = self
                    .intruder
                    .results
                    .pending_highlights
                    .remove(operation_id);
                let outcome = result.as_ref().map_err(Clone::clone).and_then(|()| {
                    if highlight.is_some_and(|color| !(1..=9).contains(&color))
                        || updated.is_empty()
                        || updated
                            .iter()
                            .any(|&(_, revision)| revision == 0 || revision > i64::MAX as u64)
                        || pending.is_some_and(|pending| {
                            pending.highlight != *highlight
                                || updated.len() != 1
                                || updated[0].0 != pending.ordinal
                                || updated[0].1 <= pending.expected_revision
                        })
                    {
                        Err(
                            "Intruder 结果高亮回执的序号、颜色或 revision 不一致；当前列表保留。"
                                .into(),
                        )
                    } else {
                        Ok(())
                    }
                });
                match outcome {
                    Ok(()) => {
                        let mut requery = Vec::new();
                        for (&view_id, view) in &mut self.intruder.results.views {
                            if view.run_id != *run_id || view.discarded {
                                continue;
                            }
                            for &(ordinal, revision) in updated {
                                view.query.update_rows(ordinal, |row| {
                                    if row.highlight_revision <= revision {
                                        row.highlight = *highlight;
                                        row.highlight_revision = revision;
                                    }
                                });
                            }
                            view.query.error = None;
                            let (filter, sort) = view.query.requested();
                            if filter.only_with_notes || filter.only_highlighted {
                                let offset = view.query.page.as_ref().map_or(0, |page| page.offset);
                                requery.push((view_id, offset, filter, sort));
                            }
                        }
                        for (view_id, offset, filter, sort) in requery {
                            self.query_intruder_result_view(
                                view_id, offset, filter, sort, window, cx,
                            );
                        }
                    }
                    Err(error) => {
                        if let Some(pending) = pending
                            && let Some(view) =
                                self.intruder.results.views.get_mut(&pending.view_id)
                            && view.run_id == *run_id
                            && !view.discarded
                        {
                            view.query.error = Some(error);
                        } else {
                            return false;
                        }
                    }
                }
            }
            _ => return false,
        }
        self.advance_intruder_result_close(cx);
        cx.notify();
        true
    }
}
