use super::*;

impl NativePanel {
    pub(in crate::plugin_panel) fn capture_all(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Vec<EditorSnapshot>, String> {
        let mut captured = Vec::new();
        for document in self.documents.values().filter(|document| document.live) {
            captured.push(capture_document(document, window, cx)?);
        }
        // Validate every editor before advancing any revision or emitting an event.
        for snapshot in &captured {
            self.accept_native(snapshot.document_id, snapshot);
        }
        captured.sort_by_key(|snapshot| snapshot.document_id);
        Ok(captured)
    }

    pub(in crate::plugin_panel) fn accept_native(&mut self, id: u64, snapshot: &EditorSnapshot) {
        let document = self
            .documents
            .get_mut(&id)
            .expect("captured document remains");
        if document.snapshot.contents != snapshot.contents {
            document.unsaved = true;
        }
        document.snapshot = snapshot.clone();
        document.pending_rebase = false;
    }

    pub(in crate::plugin_panel) fn next_ui_seq(&mut self) -> u64 {
        self.next_ui_seq = self.next_ui_seq.saturating_add(1);
        self.next_ui_seq
    }

    pub(in crate::plugin_panel) fn emit(
        &mut self,
        node_id: u64,
        event: ui_event::Event,
        cx: &mut Context<Self>,
    ) {
        self.emit_with_seq(node_id, 0, event, cx);
    }

    pub(in crate::plugin_panel) fn emit_with_seq(
        &mut self,
        node_id: u64,
        seq: u64,
        event: ui_event::Event,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_with_seq(
            seq,
            UiEvent {
                node_id,
                event: Some(event),
                editors: vec![],
            },
            cx,
        );
    }

    pub(in crate::plugin_panel) fn dispatch(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        self.dispatch_with_seq(0, event, cx);
    }

    pub(in crate::plugin_panel) fn dispatch_with_seq(
        &mut self,
        seq: u64,
        event: UiEvent,
        cx: &mut Context<Self>,
    ) {
        if self.enabled {
            cx.emit(PanelEvent::Ui {
                revision: self.snapshot.revision,
                seq,
                event,
            });
            cx.notify();
        }
    }

    pub(in crate::plugin_panel) fn fail(
        &mut self,
        error: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let error = error.into();
        self.error = Some(error.clone());
        cx.emit(PanelEvent::Error(error));
        cx.notify();
    }

    pub(in crate::plugin_panel) fn editor_event(
        &mut self,
        id: u64,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get(&id) else {
            return;
        };
        let node_id = document.node_id;
        if !document.live {
            return;
        }
        match event {
            InputEvent::Focus => {
                self.focused_document = Some(id);
                self.emit(node_id, ui_event::Event::Focus(true), cx);
            }
            InputEvent::Blur => self.emit(node_id, ui_event::Event::Focus(false), cx),
            InputEvent::Change if !document.read_only && self.enabled => {
                let before = document.snapshot.revision;
                // Incomplete Hex and marked text remain native; action-boundary capture reports them.
                match capture_document(document, window, cx) {
                    Ok(snapshot) if snapshot.revision > before => {
                        self.accept_native(id, &snapshot);
                        self.emit(node_id, ui_event::Event::EditorChanged(snapshot), cx);
                        cx.emit(PanelEvent::Edited);
                    }
                    Err(_) => {
                        self.documents.get_mut(&id).unwrap().unsaved = true;
                        cx.emit(PanelEvent::Edited);
                        cx.notify();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub(in crate::plugin_panel) fn activate(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node::Kind::Button(button)) = self.node(id).and_then(|node| node.kind.as_ref())
        else {
            return;
        };
        if !button.enabled || !self.enabled {
            return;
        }
        let editors = if button.commit_editors {
            match self.capture_all(window, cx) {
                Ok(editors) => editors,
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            }
        } else {
            vec![]
        };
        self.error = None;
        self.dispatch(
            UiEvent {
                node_id: id,
                event: Some(ui_event::Event::Click(Empty {})),
                editors,
            },
            cx,
        );
    }

    pub(in crate::plugin_panel) fn node(&self, id: u64) -> Option<&Node> {
        self.snapshot.root.as_ref()?.find(id)
    }

    pub(in crate::plugin_panel) fn pointer_down(
        &mut self,
        id: u64,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let Some(drawing) = self.drawings.get(&id) else {
            return;
        };
        let bounds = drawing.bounds.get();
        let x = (position.x - bounds.origin.x).as_f32();
        let y = (position.y - bounds.origin.y).as_f32();
        let Some(hit) = drawing
            .model
            .hit_regions
            .iter()
            .rev()
            .find(|hit| hit.bounds.as_ref().is_some_and(|rect| contains(rect, x, y)))
        else {
            return;
        };
        let hit_id = hit.id;
        if let Some(focus) = drawing.focus.get(&hit_id) {
            focus.handle.focus(window, cx);
        }
        if hit.capture_pointer {
            self.capture = Some(Capture {
                node_id: id,
                hit_id,
                position: (x, y),
            });
        }
        self.emit(
            id,
            ui_event::Event::Pointer(PointerEvent {
                hit_id,
                phase: PointerPhase::Down as i32,
                x,
                y,
            }),
            cx,
        );
    }

    pub(in crate::plugin_panel) fn drag_pointer(
        &mut self,
        position: Point<Pixels>,
        phase: PointerPhase,
        cx: &mut Context<Self>,
    ) {
        let Some(mut capture) = self.capture else {
            return;
        };
        let Some(drawing) = self.drawings.get(&capture.node_id) else {
            self.capture = None;
            return;
        };
        let bounds = drawing.bounds.get();
        let x = (position.x - bounds.origin.x)
            .as_f32()
            .clamp(-16_384., 16_384.);
        let y = (position.y - bounds.origin.y)
            .as_f32()
            .clamp(-16_384., 16_384.);
        capture.position = (x, y);
        self.capture = if matches!(phase, PointerPhase::Up | PointerPhase::Cancel) {
            None
        } else {
            Some(capture)
        };
        self.emit(
            capture.node_id,
            ui_event::Event::Pointer(PointerEvent {
                hit_id: capture.hit_id,
                phase: phase as i32,
                x,
                y,
            }),
            cx,
        );
    }

    pub(in crate::plugin_panel) fn cancel_pointer(&mut self, cx: &mut Context<Self>) {
        if let Some(capture) = self.capture.take() {
            self.emit(
                capture.node_id,
                ui_event::Event::Pointer(PointerEvent {
                    hit_id: capture.hit_id,
                    phase: PointerPhase::Cancel as i32,
                    x: capture.position.0,
                    y: capture.position.1,
                }),
                cx,
            );
        }
    }

    pub(in crate::plugin_panel) fn hover_pointer(
        &mut self,
        id: u64,
        position: Point<Pixels>,
        phase: PointerPhase,
        cx: &mut Context<Self>,
    ) {
        if self.capture.is_some() || !self.enabled {
            return;
        }
        let Some(drawing) = self.drawings.get(&id) else {
            return;
        };
        let bounds = drawing.bounds.get();
        let x = (position.x - bounds.origin.x).as_f32();
        let y = (position.y - bounds.origin.y).as_f32();
        let Some(hit) = drawing
            .model
            .hit_regions
            .iter()
            .rev()
            .find(|hit| hit.bounds.as_ref().is_some_and(|rect| contains(rect, x, y)))
        else {
            return;
        };
        if phase == PointerPhase::Up && hit.capture_pointer {
            // Captured gestures have exactly one terminal event from the window capture
            // handler. An ordinary hover Up after Cancel must not revive that gesture.
            return;
        }
        self.emit(
            id,
            ui_event::Event::Pointer(PointerEvent {
                hit_id: hit.id,
                phase: phase as i32,
                x,
                y,
            }),
            cx,
        );
    }

    pub(in crate::plugin_panel) fn resize_split(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.resize else { return };
        let Some(node::Kind::Split(split)) = self
            .node(resize.node_id)
            .and_then(|node| node.kind.as_ref())
        else {
            return;
        };
        let bounds = self.geometry[&resize.node_id].get();
        let fraction = if split.axis == Axis::Horizontal as i32 {
            (position.x - bounds.origin.x).as_f32() / bounds.size.width.as_f32().max(1.)
        } else {
            (position.y - bounds.origin.y).as_f32() / bounds.size.height.as_f32().max(1.)
        }
        .clamp(0.05, 0.95);
        let seq = self.next_ui_seq();
        self.overlays
            .push(resize.node_id, ControlOverlay::Split(fraction), seq);
        self.emit_with_seq(resize.node_id, seq, ui_event::Event::SplitResized(fraction), cx);
    }

    pub(in crate::plugin_panel) fn key(
        &mut self,
        id: u64,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" {
            self.cancel_pointer(cx);
            if let Some(resize) = self.resize.take() {
                let seq = self.next_ui_seq();
                self.overlays.push(
                    resize.node_id,
                    ControlOverlay::Split(resize.original),
                    seq,
                );
                self.emit_with_seq(
                    resize.node_id,
                    seq,
                    ui_event::Event::SplitResized(resize.original),
                    cx,
                );
            }
        }
        let modifiers = event.keystroke.modifiers;
        self.emit(
            id,
            ui_event::Event::Key(KeyEvent {
                key: event.keystroke.key.clone(),
                modifiers: u32::from(modifiers.platform)
                    | (u32::from(modifiers.shift) << 1)
                    | (u32::from(modifiers.alt) << 2)
                    | (u32::from(modifiers.control) << 3),
            }),
            cx,
        );
    }
}
