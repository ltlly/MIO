use std::sync::atomic::Ordering;

use super::*;

impl NativePanel {
    pub(crate) fn replace(
        &mut self,
        snapshot: Arc<PanelSnapshot>,
        editor_bases: &[EditorSnapshot],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.replace_with_sync(snapshot, editor_bases, None, &[], window, cx)
    }

    pub(crate) fn replace_with_sync(
        &mut self,
        snapshot: Arc<PanelSnapshot>,
        editor_bases: &[EditorSnapshot],
        synced_ui_seq: Option<u64>,
        rejected_ui_seqs: &[u64],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        snapshot.validate()?;
        if snapshot.revision < self.snapshot.revision {
            return Err("忽略已过期的插件面板。".into());
        }
        let newer_revision = snapshot.revision > self.snapshot.revision;
        let sync_cutoff = synced_ui_seq.unwrap_or(u64::MAX);
        let mut nodes = Vec::new();
        walk(snapshot.root.as_ref().unwrap(), &mut nodes);
        let ids: HashSet<_> = nodes.iter().map(|node| node.id).collect();
        let pending_input: HashSet<_> = self
            .documents
            .iter()
            .filter_map(|(id, document)| {
                (document.pending_rebase
                    || document.state.update(cx, |state, cx| {
                        state.marked_text_range(window, cx).is_some()
                            || state.value().as_ref()
                                != display_bytes(&document.snapshot.contents, document.format)
                    }))
                .then_some(*id)
            })
            .collect();
        // Check before mutating any document so overflow preserves the entire last valid panel.
        for node in &nodes {
            if let Some(node::Kind::Editor(editor)) = &node.kind
                && let Some(document) = self.documents.get(&editor.document_id)
                && document.retains_local(
                    editor,
                    pending_input.contains(&editor.document_id),
                    editor_bases,
                )
                && editor.revision >= document.snapshot.revision
            {
                editor
                    .revision
                    .checked_add(1)
                    .ok_or("插件文档 revision 已耗尽；已保留当前面板和编辑内容。")?;
            }
        }
        let mut rebased = Vec::new();
        for (id, document) in &mut self.documents {
            // Change observers can run after this layout update, including after unbinding.
            document.unsaved |= pending_input.contains(id);
            document.live = false;
        }
        for node in &nodes {
            match node.kind.as_ref().unwrap() {
                node::Kind::Editor(editor) => {
                    let format = EditorFormat::try_from(editor.format).unwrap();
                    if let Some(document) = self.documents.get_mut(&editor.document_id) {
                        document.live = true;
                        document.node_id = node.id;
                        document.label.clone_from(&node.accessibility_label);
                        document.read_only = editor.read_only;
                        let unharvested = pending_input.contains(&editor.document_id);
                        let retain = document.retains_local(editor, unharvested, editor_bases);
                        let same = editor.contents == document.snapshot.contents
                            && format == document.format;
                        if !retain
                            && editor.revision >= document.snapshot.revision
                            && (editor.revision > document.snapshot.revision || same)
                        {
                            if !same {
                                let value = display_bytes(&editor.contents, format);
                                document.state.update(cx, |state, cx| {
                                    let selection = state.selected_range();
                                    state.set_value(value, window, cx);
                                    state.set_selected_range(selection, cx);
                                });
                            }
                            document.format = format;
                            document.snapshot = initial_snapshot(editor);
                            document.pending_rebase = false;
                        } else if retain && editor.revision >= document.snapshot.revision {
                            if unharvested {
                                // Marked text and partial Hex stay native until a safe capture.
                                document.snapshot.revision = editor.revision;
                                document.pending_rebase = true;
                            } else {
                                document.snapshot.revision = editor.revision + 1;
                                rebased.push((node.id, document.snapshot.clone()));
                            }
                        }
                    } else {
                        let state = editing::new_editor(window, cx);
                        state.update(cx, |state, cx| {
                            state.set_value(display_bytes(&editor.contents, format), window, cx)
                        });
                        let id = editor.document_id;
                        let subscription =
                            cx.subscribe_in(&state, window, move |this, _, event, window, cx| {
                                this.editor_event(id, event, window, cx);
                            });
                        let selection_seq = Rc::new(Cell::new(0));
                        let seq_for_observe = selection_seq.clone();
                        let last_sel = Cell::new((0usize, 0usize, false));
                        let selection_subscription = cx.observe(&state, move |_, editor, cx| {
                            let st = editor.read(cx);
                            let r = st.selected_range();
                            let cur = (r.start, r.end, st.is_selecting());
                            let prev = last_sel.get();
                            if cur == prev {
                                return;
                            }
                            last_sel.set(cur);
                            if r.is_empty() && prev.0 == prev.1 && cur.2 == prev.2 {
                                return;
                            }
                            if !r.is_empty() {
                                seq_for_observe.set(
                                    editing::NEXT_SELECTION_SEQ.fetch_add(1, Ordering::Relaxed),
                                );
                            }
                            cx.emit(PanelEvent::Edited);
                            cx.notify();
                        });
                        self.documents.insert(
                            id,
                            Document {
                                node_id: node.id,
                                label: node.accessibility_label.clone(),
                                state,
                                snapshot: initial_snapshot(editor),
                                format,
                                read_only: editor.read_only,
                                live: true,
                                unsaved: false,
                                pending_rebase: false,
                                selection_seq,
                                _subscription: subscription,
                                _selection_subscription: selection_subscription,
                            },
                        );
                    }
                }
                node::Kind::VirtualTable(table) => {
                    if let Some(state) = self.tables.get_mut(&node.id) {
                        state.next_viewport_revision =
                            state.next_viewport_revision.max(table.viewport_revision);
                        let prev_active = state.active_viewport_revision();
                        state.confirmed_viewport_revision = table.viewport_revision;
                        state.pending_viewports.retain(|&(rev, offset, seq)| {
                            rev > table.viewport_revision
                                && (offset == 0 || offset < table.total_rows)
                                && seq > sync_cutoff
                                && !rejected_ui_seqs.contains(&seq)
                        });
                        if let Some(&(rev, offset, seq)) = state.pending_viewports.last() {
                            state.requested_offset = Some(offset);
                            state.requested_viewport = Some((rev, seq, false));
                        } else if let Some(req_offset) = state.requested_offset {
                            let target_arrived = table.offset == req_offset;
                            let out_of_range = req_offset > 0 && req_offset >= table.total_rows;
                            let (req_rev, req_seq, loading_snapshot_seen) =
                                state.requested_viewport.unwrap_or((prev_active, 0, false));
                            let rejected = req_seq != 0 && rejected_ui_seqs.contains(&req_seq);
                            let worker_published_loading = loading_snapshot_seen || newer_revision;
                            let awaiting_async_page = worker_published_loading
                                && table.viewport_revision == req_rev
                                && !target_arrived
                                && !out_of_range
                                && !rejected;
                            if awaiting_async_page {
                                if newer_revision && !loading_snapshot_seen {
                                    state.requested_viewport = Some((req_rev, req_seq, true));
                                }
                            } else {
                                state.requested_offset = None;
                                state.requested_viewport = None;
                            }
                        }
                        state
                            .selected
                            .retain(|id| table.rows.iter().any(|row| &row.id == id));
                    } else {
                        let filter =
                            cx.new(|cx| InputState::new(window, cx).placeholder("筛选此表格"));
                        let id = node.id;
                        let subscription =
                            cx.subscribe_in(&filter, window, move |this, _, event, window, cx| {
                                match event {
                                    InputEvent::PressEnter { .. } => {
                                        this.filter_table(id, window, cx)
                                    }
                                    InputEvent::Focus => {
                                        this.emit(id, ui_event::Event::Focus(true), cx)
                                    }
                                    InputEvent::Blur => {
                                        this.emit(id, ui_event::Event::Focus(false), cx)
                                    }
                                    _ => {}
                                }
                            });
                        let focus = cx.focus_handle();
                        let focused = cx.on_focus(&focus, window, move |this, _, cx| {
                            this.emit(id, ui_event::Event::Focus(true), cx)
                        });
                        let blurred = cx.on_blur(&focus, window, move |this, _, cx| {
                            this.emit(id, ui_event::Event::Focus(false), cx)
                        });
                        self.tables.insert(
                            id,
                            Table {
                                filter,
                                selected: vec![],
                                anchor: None,
                                cursor: None,
                                focus,
                                scroll: UniformListScrollHandle::new(),
                                horizontal: ScrollHandle::new(),
                                next_viewport_revision: table.viewport_revision,
                                confirmed_viewport_revision: table.viewport_revision,
                                pending_viewports: Vec::new(),
                                requested_offset: None,
                                requested_viewport: None,
                                _subscription: subscription,
                                _focus_subscriptions: vec![focused, blurred],
                            },
                        );
                    }
                }
                node::Kind::Canvas(model) => {
                    let entry = self.drawings.entry(node.id).or_insert_with(|| Drawing {
                        model: model.clone(),
                        image: None,
                        error: None,
                        bounds: Rc::new(Cell::new(Bounds::default())),
                        focus: HashMap::new(),
                        dirty: true,
                        pixel_budget: 0,
                    });
                    if entry.model != *model {
                        entry.model = model.clone();
                        entry.dirty = true;
                        entry.error = None;
                    }
                    entry.focus.retain(|id, _| {
                        model
                            .hit_regions
                            .iter()
                            .any(|hit| hit.id == *id && hit.focusable)
                    });
                    for hit in model.hit_regions.iter().filter(|hit| hit.focusable) {
                        entry.focus.entry(hit.id).or_insert_with(|| {
                            let handle = cx.focus_handle();
                            let id = node.id;
                            let focused = cx.on_focus(&handle, window, move |this, _, cx| {
                                this.emit(id, ui_event::Event::Focus(true), cx)
                            });
                            let blurred = cx.on_blur(&handle, window, move |this, _, cx| {
                                this.cancel_pointer(cx);
                                this.emit(id, ui_event::Event::Focus(false), cx);
                            });
                            CanvasFocus {
                                handle,
                                _subscriptions: vec![focused, blurred],
                            }
                        });
                    }
                }
                node::Kind::Split(_) => {
                    self.geometry
                        .entry(node.id)
                        .or_insert_with(|| Rc::new(Cell::new(Bounds::default())));
                }
                _ => {}
            }
        }
        self.tables.retain(|id, _| ids.contains(id));
        self.drawings.retain(|id, _| ids.contains(id));
        self.geometry.retain(|id, _| ids.contains(id));
        if newer_revision {
            let old_root = self.snapshot.root.as_ref();
            let new_root = snapshot.root.as_ref();
            retain_semantic_overlays(
                &mut self.tabs,
                sync_cutoff,
                rejected_ui_seqs,
                old_root,
                new_root,
                |old_node, new_node, &tab_id| {
                    matches!(
                        (&old_node.kind, &new_node.kind),
                        (Some(node::Kind::Tabs(old_t)), Some(node::Kind::Tabs(new_t)))
                            if old_t
                                .tabs
                                .iter()
                                .find_map(|t| {
                                    t.content
                                        .as_ref()
                                        .is_some_and(|c| c.id == tab_id)
                                        .then_some(t.title.as_str())
                                })
                                .is_some_and(|old_title| {
                                    new_t.tabs.iter().any(|t| {
                                        t.content.as_ref().is_some_and(|c| c.id == tab_id)
                                            && t.title == old_title
                                    })
                                })
                    )
                },
            );
            retain_semantic_overlays(
                &mut self.splits,
                sync_cutoff,
                rejected_ui_seqs,
                old_root,
                new_root,
                |old_node, new_node, _| {
                    matches!(
                        (&old_node.kind, &new_node.kind),
                        (Some(node::Kind::Split(old_s)), Some(node::Kind::Split(new_s)))
                            if old_s.axis == new_s.axis
                    )
                },
            );
            retain_semantic_overlays(
                &mut self.checkboxes,
                sync_cutoff,
                rejected_ui_seqs,
                old_root,
                new_root,
                |old_node, new_node, _| {
                    matches!(
                        (&old_node.kind, &new_node.kind),
                        (
                            Some(node::Kind::Checkbox(old_c)),
                            Some(node::Kind::Checkbox(new_c))
                        ) if new_c.enabled && old_c.label == new_c.label
                    )
                },
            );
            retain_semantic_overlays(
                &mut self.selects,
                sync_cutoff,
                rejected_ui_seqs,
                old_root,
                new_root,
                |old_node, new_node, &idx| {
                    matches!(
                        (&old_node.kind, &new_node.kind),
                        (
                            Some(node::Kind::Select(old_s)),
                            Some(node::Kind::Select(new_s))
                        ) if new_s.enabled
                            && old_s.label == new_s.label
                            && new_s
                                .options
                                .get(idx as usize)
                                .is_some_and(|opt| old_s.options.get(idx as usize) == Some(opt))
                    )
                },
            );
        } else {
            prune_overlays(&mut self.tabs, sync_cutoff, rejected_ui_seqs);
            prune_overlays(&mut self.splits, sync_cutoff, rejected_ui_seqs);
            prune_overlays(&mut self.checkboxes, sync_cutoff, rejected_ui_seqs);
            prune_overlays(&mut self.selects, sync_cutoff, rejected_ui_seqs);
        }
        if let Some(capture) = self.capture
            && !self.drawings.get(&capture.node_id).is_some_and(|drawing| {
                drawing
                    .model
                    .hit_regions
                    .iter()
                    .any(|hit| hit.id == capture.hit_id && hit.capture_pointer)
            })
        {
            self.cancel_pointer(cx);
        }
        if self
            .resize
            .is_some_and(|resize| !ids.contains(&resize.node_id))
        {
            self.resize = None;
        }
        self.snapshot = snapshot;
        self.remember_visible_document();
        self.prepare_drawings(window, cx);
        for (node, editor) in rebased {
            if self
                .documents
                .get(&editor.document_id)
                .is_some_and(|document| !document.read_only)
            {
                self.emit(node, ui_event::Event::EditorChanged(editor), cx);
            }
        }
        cx.notify();
        Ok(())
    }

    pub(in crate::plugin_panel) fn prepare_drawings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut visible = Vec::new();
        self.visible_nodes(self.snapshot.root.as_ref().unwrap(), &mut visible);
        let visible: HashSet<_> = visible.into_iter().collect();
        for (id, drawing) in &mut self.drawings {
            if !visible.contains(id) {
                drawing.image = None;
                drawing.dirty = true;
            }
        }
        let count = self
            .drawings
            .keys()
            .filter(|id| visible.contains(id))
            .count()
            .max(1);
        // All visible canvases in this panel share the same raster budget; inactive tabs
        // release their images instead of multiplying the per-canvas allocation limit.
        let pixel_budget = canvas::PANEL_PIXEL_BUDGET / count;
        for (id, drawing) in &mut self.drawings {
            if visible.contains(id) && drawing.pixel_budget != pixel_budget {
                drawing.pixel_budget = pixel_budget;
                drawing.dirty = true;
            }
        }
        if self.image_task.is_some() {
            return;
        }
        let pending: Vec<_> = self
            .drawings
            .iter()
            .filter(|(id, drawing)| drawing.dirty && visible.contains(id))
            .map(|(id, drawing)| (*id, drawing.model.clone()))
            .collect();
        if pending.is_empty() {
            return;
        }
        let renderer = cx.svg_renderer();
        let render = cx.background_executor().spawn(async move {
            pending
                .into_iter()
                .map(|(id, model)| {
                    let result = canvas::svg(&model, pixel_budget).and_then(|svg| {
                        renderer
                            .render_single_frame(&svg, 1.)
                            .map_err(|error| format!("Canvas 绘制失败：{error}"))
                    });
                    (id, model, result)
                })
                .collect::<Vec<_>>()
        });
        self.image_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = render.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.image_task = None;
                let mut visible = Vec::new();
                this.visible_nodes(this.snapshot.root.as_ref().unwrap(), &mut visible);
                for (id, model, result) in result {
                    if let Some(drawing) = this.drawings.get_mut(&id)
                        && drawing.model == model
                        && drawing.pixel_budget == pixel_budget
                        && visible.contains(&id)
                    {
                        drawing.dirty = false;
                        match result {
                            Ok(image) => {
                                drawing.image = Some(image);
                                drawing.error = None;
                            }
                            Err(error) => {
                                drawing.image = None;
                                drawing.error = Some(error);
                            }
                        }
                    }
                }
                // Coalesce patches received during this one background job. Never spawn an
                // unbounded queue of raster jobs or install a result for a replaced canvas.
                this.prepare_drawings(window, cx);
                cx.notify();
            });
        }));
    }

    pub(in crate::plugin_panel) fn visible_nodes(&self, node: &Node, output: &mut Vec<u64>) {
        output.push(node.id);
        match node.kind.as_ref().unwrap() {
            node::Kind::Row(container)
            | node::Kind::Column(container)
            | node::Kind::ScrollColumn(container) => {
                for child in &container.children {
                    self.visible_nodes(child, output);
                }
            }
            node::Kind::Split(split) => {
                self.visible_nodes(split.first.as_ref().unwrap(), output);
                self.visible_nodes(split.second.as_ref().unwrap(), output);
            }
            node::Kind::Tabs(tabs) => {
                let active = latest_overlay(&self.tabs, node.id).unwrap_or(tabs.active);
                if let Some(child) = tabs
                    .tabs
                    .iter()
                    .filter_map(|tab| tab.content.as_ref())
                    .find(|node| node.id == active)
                {
                    self.visible_nodes(child, output);
                }
            }
            _ => {}
        }
    }
}

fn retain_semantic_overlays<V>(
    map: &mut HashMap<u64, Vec<(V, u64)>>,
    sync_cutoff: u64,
    rejected_ui_seqs: &[u64],
    old_root: Option<&Node>,
    new_root: Option<&Node>,
    mut valid_target: impl FnMut(&Node, &Node, &V) -> bool,
) {
    map.retain(|&node_id, history| {
        let Some((old_node, new_node)) = old_root
            .and_then(|r| r.find(node_id))
            .zip(new_root.and_then(|r| r.find(node_id)))
        else {
            return false;
        };
        history.retain(|(value, seq)| {
            *seq > sync_cutoff
                && !rejected_ui_seqs.contains(seq)
                && valid_target(old_node, new_node, value)
        });
        !history.is_empty()
    });
}
