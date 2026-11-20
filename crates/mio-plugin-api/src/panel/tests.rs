use super::*;

fn editor(read_only: bool) -> PanelSnapshot {
    PanelSnapshot {
        revision: 1,
        title: "test".into(),
        root: Some(Node::new(
            1,
            "editor",
            node::Kind::Editor(Editor {
                document_id: 7,
                revision: 1,
                contents: b"old".to_vec(),
                format: EditorFormat::Utf8 as i32,
                read_only,
            }),
        )),
    }
}

#[test]
fn invalid_patch_is_atomic_and_old_revision_is_rejected() {
    let mut state = PanelState::new(editor(false)).unwrap();
    let original = state.snapshot().clone();
    let patch = PanelPatch {
        base_revision: 1,
        revision: 2,
        operations: vec![
            PatchOperation {
                node_id: 1,
                change: Some(patch_operation::Change::Editor(EditorUpdate {
                    base_revision: 1,
                    snapshot: Some(EditorSnapshot {
                        document_id: 7,
                        revision: 2,
                        contents: b"new".to_vec(),
                        ..Default::default()
                    }),
                })),
            },
            PatchOperation {
                node_id: 999,
                change: None,
            },
        ],
    };
    assert!(state.apply(&patch).is_err());
    assert_eq!(state.snapshot(), &original);
    let mut valid = patch.clone();
    valid.operations.pop();
    state.apply(&valid).unwrap();
    assert!(state.apply(&valid).is_err());
    assert_eq!(state.snapshot().revision, 2);
}

#[test]
fn read_only_and_stale_input_are_rejected() {
    let event = UiEvent {
        node_id: 1,
        event: Some(ui_event::Event::EditorChanged(EditorSnapshot {
            document_id: 7,
            revision: 2,
            contents: "基线".as_bytes().to_vec(),
            selection_start: 0,
            selection_end: 6,
        })),
        editors: vec![],
    };
    assert!(
        PanelState::new(editor(true))
            .unwrap()
            .observe_event(&event)
            .is_err()
    );
    let mut editable = PanelState::new(editor(false)).unwrap();
    editable.observe_event(&event).unwrap();
    assert!(editable.observe_event(&event).is_err());
}

#[test]
fn full_snapshot_cannot_replace_a_newer_table_viewport() {
    let original = PanelSnapshot {
        revision: 1,
        title: "test".into(),
        root: Some(Node::new(
            1,
            "table",
            node::Kind::VirtualTable(VirtualTable {
                columns: vec![TableColumn {
                    id: "id".into(),
                    title: "ID".into(),
                    width: 80.0,
                }],
                total_rows: 4,
                offset: 0,
                viewport_revision: 1,
                rows: vec![TableRow {
                    id: "first".into(),
                    cells: vec!["first".into()],
                }],
                multi_select: false,
            }),
        )),
    };
    let mut state = PanelState::new(original.clone()).unwrap();
    state
        .observe_event(&UiEvent {
            node_id: 1,
            event: Some(ui_event::Event::Viewport(ViewportRequest {
                revision: 2,
                offset: 3,
                limit: 1,
            })),
            editors: vec![],
        })
        .unwrap();
    let latest = state.snapshot().clone();
    let mut late = original;
    late.revision = 2;
    assert!(
        state.replace(late).is_err(),
        "full snapshots must enforce the same viewport guard as table patches"
    );
    assert_eq!(state.snapshot(), &latest);
}

#[test]
fn duplicate_nodes_and_unbounded_canvas_are_rejected() {
    let root = editor(false).root.unwrap();
    let duplicate = PanelSnapshot {
        revision: 1,
        title: "test".into(),
        root: Some(Node::new(
            2,
            "root",
            node::Kind::Column(Container {
                children: vec![root.clone(), root],
            }),
        )),
    };
    assert!(duplicate.validate().is_err());
    let c = Canvas {
        width: f32::NAN,
        height: 10.0,
        ..Default::default()
    };
    assert!(c.validate().is_err());
    let c = Canvas {
        width: 100.0,
        height: 100.0,
        commands: vec![DrawCommand {
            op: Some(draw_command::Op::PopClip(Empty {})),
        }],
        ..Default::default()
    };
    assert!(c.validate().is_err());
}

#[test]
fn scroll_column_checkbox_select_badge_and_progress_validate_and_observe_events() {
    let snap = PanelSnapshot {
        revision: 1,
        title: "rich-widgets".into(),
        root: Some(Node::new(
            1,
            "scroll-root",
            node::Kind::ScrollColumn(Container {
                children: vec![
                    Node::new(
                        2,
                        "自动标记",
                        node::Kind::Checkbox(CheckboxNode {
                            label: "自动标记 Hook".into(),
                            checked: false,
                            enabled: true,
                        }),
                    ),
                    Node::new(
                        3,
                        "编码模式",
                        node::Kind::Select(SelectNode {
                            label: "模式".into(),
                            options: vec!["Standard".into(), "URLSafe".into()],
                            selected_index: 0,
                            enabled: true,
                        }),
                    ),
                    Node::new(
                        4,
                        "状态徽章",
                        node::Kind::Badge(BadgeNode {
                            text: "READY".into(),
                            tone: Tone::Accent as i32,
                        }),
                    ),
                    Node::new(
                        5,
                        "处理进度",
                        node::Kind::Progress(ProgressNode {
                            label: "加载完成".into(),
                            percent: 100,
                        }),
                    ),
                ],
            }),
        )),
    };
    let mut state = PanelState::new(snap).unwrap();
    state
        .observe_event(&UiEvent {
            node_id: 2,
            event: Some(ui_event::Event::Toggled(true)),
            editors: vec![],
        })
        .unwrap();
    state
        .observe_event(&UiEvent {
            node_id: 3,
            event: Some(ui_event::Event::SelectedOption(1)),
            editors: vec![],
        })
        .unwrap();
    assert!(matches!(
        state.node(2).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Checkbox(c)) if c.checked
    ));
    assert!(matches!(
        state.node(3).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 1
    ));
    assert!(
        state
            .observe_event(&UiEvent {
                node_id: 3,
                event: Some(ui_event::Event::SelectedOption(2)),
                editors: vec![],
            })
            .is_err()
    );
}
