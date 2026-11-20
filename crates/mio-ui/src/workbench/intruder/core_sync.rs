use super::*;

impl Workbench {
    pub(in crate::workbench) fn sync_intruder_editors(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.intruder
            .tabs_focus
            .get_or_insert_with(|| cx.focus_handle());
        let project = self.project;
        let ids: HashSet<_> = self
            .workspace
            .intruder
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect();
        self.intruder.editors.retain(|id, _| ids.contains(id));
        for tab in &self.workspace.intruder.tabs {
            if let Some(editors) = self.intruder.editors.get_mut(&tab.id) {
                if tab.spec.positions.is_empty()
                    && editors.payload_input.focus_handle(cx).is_focused(window)
                {
                    self.focus.focus(window, cx);
                }
                if editors.payloads_revision != tab.spec.revision {
                    editors.payloads_identical = payloads::payload_sets_identical(&tab.spec);
                    editors.payloads_revision = tab.spec.revision;
                    if (editors.payload_position.is_none() && !editors.payloads_identical)
                        || editors
                            .payload_position
                            .is_some_and(|index| index >= tab.spec.payload_sets.len())
                    {
                        editors.payload_selection.clear();
                    }
                }
                // Preedit can arrive before the observer runs, without emitting Change.
                if !editors.dirty
                    && (editors.loaded_revision < tab.spec.draft.revision
                        || editors.request.saved_positions != tab.spec.positions)
                    && !editors.has_composition(window, cx)
                {
                    editors
                        .request
                        .load(&tab.spec.draft.raw, &tab.spec.positions, window, cx);
                    editors.hex_length = Some(tab.spec.draft.raw.len());
                    editors.target.update(cx, |state, cx| {
                        state.set_value(tab.spec.draft.target.clone(), window, cx)
                    });
                    editors.loaded_revision = tab.spec.draft.revision;
                }
                continue;
            }
            let mut request = positions::PositionsEditor::new(window, cx);
            request.load(&tab.spec.draft.raw, &tab.spec.positions, window, cx);
            let target = cx
                .new(|cx| InputState::new(window, cx).default_value(tab.spec.draft.target.clone()));
            let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
            let payload_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入新条目"));
            let numbers_from = cx.new(|cx| InputState::new(window, cx).placeholder("起始值"));
            let numbers_to = cx.new(|cx| InputState::new(window, cx).placeholder("结束值"));
            let numbers_step = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("步长")
                    .default_value("1")
            });
            let numbers_min_digits = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("位数")
                    .default_value("1")
            });
            let grep_match_input = cx
                .new(|cx| InputState::new(window, cx).placeholder("输入匹配词 (如 error / flag)"));
            let grep_extract_start =
                cx.new(|cx| InputState::new(window, cx).placeholder("起始分隔符 (如 token=)"));
            let grep_extract_end =
                cx.new(|cx| InputState::new(window, cx).placeholder("结束分隔符 (如 &)"));
            let id = tab.id;
            let mut subscriptions = Vec::new();
            for input in [&request.raw, &request.hex] {
                let mut composing = false;
                let mut last_sel = (0usize, 0usize);
                let mut last_selecting = false;
                subscriptions.push(
                    cx.observe_in(input, window, move |this, input, window, cx| {
                        if this.project != project {
                            return;
                        }
                        let (pair, selecting) = {
                            let state = input.read(cx);
                            let sel = state.selected_range();
                            ((sel.start, sel.end), state.is_selecting())
                        };
                        let sel_changed =
                            pair != last_sel && (pair.0 != pair.1 || last_sel.0 != last_sel.1);
                        let selecting_ended = last_selecting && !selecting;
                        if pair != last_sel {
                            last_sel = pair;
                        }
                        last_selecting = selecting;
                        let marked = input.update(cx, |state, cx| {
                            state.marked_text_range(window, cx).is_some()
                        });
                        if marked != composing {
                            composing = marked;
                            if marked {
                                this.cancel_intruder_save_wait(id, cx);
                            }
                            cx.notify();
                        } else if sel_changed || selecting_ended {
                            cx.notify();
                        }
                    }),
                );
                subscriptions.push(cx.subscribe_in(
                    input,
                    window,
                    move |this, input, event, window, cx| {
                        if this.project == project && matches!(event, InputEvent::Change) {
                            if let Some(editors) = this.intruder.editors.get_mut(&id)
                                && editors.request.view == MessageView::Hex
                                && input == &editors.request.hex
                            {
                                editors.hex_length = editors.request.byte_length(cx);
                            }
                            this.intruder_input_changed(id, window, cx);
                        }
                    },
                ));
            }
            let mut composing = false;
            subscriptions.push(
                cx.observe_in(&target, window, move |this, input, window, cx| {
                    if this.project != project {
                        return;
                    }
                    let marked = input.update(cx, |state, cx| {
                        state.marked_text_range(window, cx).is_some()
                    });
                    if marked != composing {
                        composing = marked;
                        if marked {
                            this.cancel_intruder_save_wait(id, cx);
                        }
                        cx.notify();
                    }
                }),
            );
            subscriptions.push(cx.subscribe_in(
                &target,
                window,
                move |this, _, event, window, cx| {
                    if this.project == project && matches!(event, InputEvent::Change) {
                        this.intruder_input_changed(id, window, cx);
                    }
                },
            ));
            subscriptions.push(cx.subscribe_in(
                &search,
                window,
                move |this, _, event, window, cx| {
                    if this.project != project {
                        return;
                    }
                    match event {
                        InputEvent::Change => this.search_intruder(id, None, window, cx),
                        InputEvent::PressEnter { shift, .. } => {
                            this.search_intruder(id, Some(!shift), window, cx)
                        }
                        _ => {}
                    }
                },
            ));
            subscriptions.push(cx.subscribe_in(
                &payload_input,
                window,
                move |this, _, event, window, cx| {
                    if this.project == project && matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_intruder_payload(id, window, cx);
                    }
                },
            ));
            let pool_form =
                resource_pools::CreatePoolForm::new(tab.spec.resource_pool.as_ref(), window, cx);
            for input in pool_form.inputs() {
                let mut composing = false;
                subscriptions.push(cx.observe_in(
                    &input,
                    window,
                    move |this, input, window, cx| {
                        if this.project != project {
                            return;
                        }
                        let marked = input.update(cx, |state, cx| {
                            state.marked_text_range(window, cx).is_some()
                        });
                        if marked != composing {
                            composing = marked;
                            if marked {
                                this.cancel_intruder_save_wait(id, cx);
                            }
                            cx.notify();
                        }
                    },
                ));
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event, window, cx| {
                        if this.project == project && matches!(event, InputEvent::Change) {
                            this.intruder_pool_form_changed(id, window, cx);
                        }
                    },
                ));
            }
            let auto_expand = self.intruder_user_misc().intruder_side_panel_auto_expand;
            let initial_payload_type = self
                .workspace
                .intruder
                .payload_configs
                .get(&id)
                .and_then(|configs| configs.first())
                .map(|cfg| cfg.payload_type)
                .unwrap_or_default();
            self.intruder.editors.insert(
                id,
                IntruderEditors {
                    request,
                    hex_length: Some(tab.spec.draft.raw.len()),
                    target,
                    search,
                    payload_input,
                    numbers_from,
                    numbers_to,
                    numbers_step,
                    numbers_min_digits,
                    numbers_hex: false,
                    grep_match_input,
                    grep_extract_start,
                    grep_extract_end,
                    payload_type: initial_payload_type,
                    payload_position: None,
                    payload_selection: HashSet::new(),
                    payloads_revision: tab.spec.revision,
                    payloads_identical: payloads::payload_sets_identical(&tab.spec),
                    payloads_open: auto_expand,
                    settings_open: false,
                    resource_pool_open: false,
                    pool_form,
                    baseline_restore: None,
                    loading_payloads: false,
                    dirty: false,
                    loaded_revision: tab.spec.draft.revision,
                    _subscriptions: subscriptions,
                },
            );
        }
    }

    pub(in crate::workbench) fn snapshot_intruder_spec(
        &self,
        id: TabId,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<IntruderSpec, String> {
        let tab = self
            .workspace
            .intruder
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or_else(|| "Intruder 工作标签已关闭，未开始。".to_owned())?;
        let editors = self
            .intruder
            .editors
            .get(&id)
            .ok_or_else(|| "Intruder 编辑器尚未载入，未开始。".to_owned())?;
        let mut next = tab.spec.clone();
        if editors.has_composition(window, cx) {
            return Err(editing::COMPOSITION_PENDING.to_owned());
        }
        (next.draft.raw, next.positions) = editors.request.snapshot(window, cx)?;
        next.draft.target = editing::input_snapshot(&editors.target, window, cx)
            .map_err(str::to_owned)?
            .to_string();
        next.resource_pool = if editors.pool_form.creating {
            Some(mio_core::IntruderPoolSelection::Create(
                editors.pool_form.snapshot(window, cx)?,
            ))
        } else {
            editors
                .pool_form
                .existing
                .map(mio_core::IntruderPoolSelection::Existing)
        };
        revised_spec(&tab.spec, next)
    }

    pub(in crate::workbench) fn snapshot_intruder_drafts(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Vec<(TabId, IntruderSpec)>, String> {
        self.workspace
            .intruder
            .tabs
            .iter()
            .map(|tab| {
                self.snapshot_intruder_spec(tab.id, window, cx)
                    .map(|spec| (tab.id, spec))
            })
            .collect()
    }

    pub(in crate::workbench) fn apply_intruder_drafts(
        &mut self,
        snapshots: Vec<(TabId, IntruderSpec)>,
    ) -> bool {
        let mut changed = false;
        for (id, spec) in snapshots {
            let tab = self
                .workspace
                .intruder
                .tabs
                .iter_mut()
                .find(|tab| tab.id == id)
                .expect("snapshotted Intruder tab remains open");
            changed |= tab.spec != spec;
            tab.spec = spec;
            let editors = self
                .intruder
                .editors
                .get_mut(&id)
                .expect("snapshotted editor exists");
            if matches!(&tab.spec.resource_pool, Some(mio_core::IntruderPoolSelection::Create(pool)) if pool.retained_inputs.is_some())
            {
                editors.pool_form.accept_retained_inputs();
            }
            editors.loaded_revision = tab.spec.draft.revision;
            editors.request.saved_positions = tab.spec.positions.clone();
            editors.hex_length = Some(tab.spec.draft.raw.len());
            editors.payloads_revision = tab.spec.revision;
            editors.dirty = false;
        }
        changed
    }

    pub(in crate::workbench) fn intruder_unsaved(&self, window: &mut Window, cx: &mut App) -> bool {
        self.snapshot_intruder_rename(window, cx)
            .map_or(true, |rename| {
                rename.is_some_and(|(id, title)| {
                    self.workspace
                        .intruder
                        .tabs
                        .iter()
                        .any(|tab| tab.id == id && tab.title != title)
                        || self
                            .workspace
                            .intruder
                            .groups
                            .iter()
                            .any(|group| group.id == id && group.title != title)
                })
            })
            || self
                .intruder
                .editors
                .values()
                .any(|editors| editors.dirty || editors.has_composition(window, cx))
    }
}
