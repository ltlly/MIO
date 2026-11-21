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

#[test]
fn repeated_same_value_optimistic_requests_retain_identity_across_retraction() {
    let snap = PanelSnapshot {
        revision: 1,
        title: "same-value-optimistic".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![Node::new(
                    3,
                    "编码模式",
                    node::Kind::Select(SelectNode {
                        label: "模式".into(),
                        options: vec!["Standard".into(), "URLSafe".into()],
                        selected_index: 0,
                        enabled: true,
                    }),
                )],
            }),
        )),
    };
    let mut state = PanelState::new(snap).unwrap();
    let select_b = UiEvent {
        node_id: 3,
        event: Some(ui_event::Event::SelectedOption(1)),
        editors: vec![],
    };
    state.observe_event_for_request(&select_b, 101).unwrap();
    state.observe_event_for_request(&select_b, 102).unwrap();
    assert!(!state.retract_optimistic_request(101));
    assert!(matches!(
        state.node(3).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 1
    ));
    assert!(state.retract_optimistic_request(102));
    assert!(matches!(
        state.node(3).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 0
    ));

    // If req1 is retracted and req2 settles, value B stays committed.
    state.observe_event_for_request(&select_b, 201).unwrap();
    state.observe_event_for_request(&select_b, 202).unwrap();
    assert!(!state.retract_optimistic_request(201));
    state.settle_optimistic_request(202);
    assert!(matches!(
        state.node(3).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 1
    ));
}

#[test]
fn replace_and_apply_for_request_preserve_subsequent_optimistic_entries() {
    let make_snap = |rev: u64, vp_rev: u64, selected: u32| PanelSnapshot {
        revision: rev,
        title: "pipeline".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        2,
                        "select",
                        node::Kind::Select(SelectNode {
                            label: "Choice".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index: selected,
                            enabled: true,
                        }),
                    ),
                    Node::new(
                        3,
                        "tbl",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 10,
                            offset: 0,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                ],
            }),
        )),
    };

    // 1. replace_for_request for req 10 preserves & rebases pipelined Select(1) and Viewport(2) at req 11 & 12.
    let mut state = PanelState::new(make_snap(1, 1, 0)).unwrap();
    state
        .observe_event_for_request(
            &UiEvent {
                node_id: 2,
                event: Some(ui_event::Event::SelectedOption(1)),
                editors: vec![],
            },
            11,
        )
        .unwrap();
    state
        .observe_event_for_request(
            &UiEvent {
                node_id: 3,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 2,
                    offset: 0,
                    limit: 5,
                })),
                editors: vec![],
            },
            12,
        )
        .unwrap();
    // Snapshot from req 10 arrives carrying older vp_rev=1 and Select=0 -> MUST pass and keep req 11 & 12!
    state.replace_for_request(make_snap(2, 1, 0), 10).unwrap();
    assert!(matches!(
        state.node(2).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 1
    ));
    assert!(matches!(
        state.node(3).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 2
    ));
    assert!(state.retract_optimistic_request(11));
    assert!(state.retract_optimistic_request(12));
    assert!(matches!(
        state.node(2).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 0
    ));
    assert!(matches!(
        state.node(3).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 1
    ));

    // 2. apply_for_request (Patch replacing table & select for req 20) preserves pipelined req 21!
    let mut patched = PanelState::new(make_snap(1, 1, 0)).unwrap();
    patched
        .observe_event_for_request(
            &UiEvent {
                node_id: 2,
                event: Some(ui_event::Event::SelectedOption(1)),
                editors: vec![],
            },
            21,
        )
        .unwrap();
    patched
        .apply_for_request(
            &PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![PatchOperation {
                    node_id: 2,
                    change: Some(patch_operation::Change::Replace(Node::new(
                        2,
                        "select",
                        node::Kind::Select(SelectNode {
                            label: "Choice".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index: 0,
                            enabled: true,
                        }),
                    ))),
                }],
            },
            20,
        )
        .unwrap();
    assert!(matches!(
        patched.node(2).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 1
    ));
    assert!(patched.retract_optimistic_request(21));
    assert!(matches!(
        patched.node(2).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 0
    ));
}

#[test]
fn apply_for_request_uses_replaced_table_watermark_within_same_patch() {
    let make_table = |vp_rev: u64, total_rows: u64| {
        Node::new(
            3,
            "tbl",
            node::Kind::VirtualTable(VirtualTable {
                columns: vec![TableColumn {
                    id: "c1".into(),
                    title: "Col".into(),
                    width: 80.0,
                }],
                total_rows,
                offset: 0,
                viewport_revision: vp_rev,
                rows: vec![],
                multi_select: false,
            }),
        )
    };
    let snap = PanelSnapshot {
        revision: 1,
        title: "patch-table-replace".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![make_table(1, 10)],
            }),
        )),
    };

    let mut state = PanelState::new(snap).unwrap();
    state
        .observe_event_for_request(
            &UiEvent {
                node_id: 3,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 2,
                    offset: 0,
                    limit: 5,
                })),
                editors: vec![],
            },
            11,
        )
        .unwrap();

    // Replacing table (vp=1 -> vp=10) and updating it with Change::Table(vp=10) in the SAME patch
    // MUST compare against the replaced node's viewport_revision (10), not pre-patch optimistic watermark (2).
    let invalidated = state
        .apply_for_request(
            &PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![
                    PatchOperation {
                        node_id: 3,
                        change: Some(patch_operation::Change::Replace(make_table(10, 20))),
                    },
                    PatchOperation {
                        node_id: 3,
                        change: Some(patch_operation::Change::Table(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 25,
                            offset: 0,
                            viewport_revision: 10,
                            rows: vec![],
                            multi_select: false,
                        })),
                    },
                ],
            },
            10,
        )
        .unwrap();
    // Stale Viewport(2) at req 11 is <= replaced vp=10 -> returned as invalidated!
    assert_eq!(invalidated, vec![11]);
    assert!(!state.has_optimistic_request(11));
    assert!(matches!(
        state.node(3).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 10 && t.total_rows == 25
    ));

    // Contrast: Replace(vp=10) followed by Change::Table(vp=11) MUST still fail because 11 != replaced vp=10.
    assert!(
        state
            .apply_for_request(
                &PanelPatch {
                    base_revision: 2,
                    revision: 3,
                    operations: vec![
                        PatchOperation {
                            node_id: 3,
                            change: Some(patch_operation::Change::Replace(make_table(10, 30))),
                        },
                        PatchOperation {
                            node_id: 3,
                            change: Some(patch_operation::Change::Table(VirtualTable {
                                columns: vec![TableColumn {
                                    id: "c1".into(),
                                    title: "Col".into(),
                                    width: 80.0,
                                }],
                                total_rows: 30,
                                offset: 0,
                                viewport_revision: 11,
                                rows: vec![],
                                multi_select: false,
                            })),
                        },
                    ],
                },
                12,
            )
            .is_err()
    );
}

#[test]
fn replace_and_apply_for_request_return_invalidated_semantically_changed_requests() {
    let make_snap = |rev: u64, options: Vec<&str>, sel: u32, chk_label: &str, tab_title: &str| {
        PanelSnapshot {
            revision: rev,
            title: "semantic-check".into(),
            root: Some(Node::new(
                1,
                "tabs",
                node::Kind::Tabs(Tabs {
                    axis: Axis::Horizontal as i32,
                    active: 10,
                    tabs: vec![
                        Tab {
                            title: "Main".into(),
                            content: Some(Box::new(Node::new(
                                10,
                                "col",
                                node::Kind::Column(Container {
                                    children: vec![
                                        Node::new(
                                            2,
                                            "select",
                                            node::Kind::Select(SelectNode {
                                                label: "Choice".into(),
                                                options: options.into_iter().map(str::to_owned).collect(),
                                                selected_index: sel,
                                                enabled: true,
                                            }),
                                        ),
                                        Node::new(
                                            3,
                                            "chk",
                                            node::Kind::Checkbox(CheckboxNode {
                                                label: chk_label.into(),
                                                checked: false,
                                                enabled: true,
                                            }),
                                        ),
                                    ],
                                }),
                            ))),
                        },
                        Tab {
                            title: tab_title.into(),
                            content: Some(Box::new(Node::new(
                                11,
                                "tab2",
                                node::Kind::Text(Text {
                                    text: "Detail".into(),
                                    tone: Tone::Foreground as i32,
                                }),
                            ))),
                        },
                    ],
                }),
            )),
        }
    };

    // Select ["A", "B", "C"], req 11 -> idx 0 ("A"), req 12 -> idx 1 ("B"),
    // Checkbox req 13 -> "OldFlag", Tab req 14 -> "Settings".
    let mut state = PanelState::new(make_snap(1, vec!["A", "B", "C"], 2, "OldFlag", "Settings")).unwrap();
    for (req_id, node_id, event) in [
        (11, 2, ui_event::Event::SelectedOption(0)),
        (12, 2, ui_event::Event::SelectedOption(1)),
        (13, 3, ui_event::Event::Toggled(true)),
        (14, 1, ui_event::Event::TabSelected(11)),
    ] {
        state
            .observe_event_for_request(
                &UiEvent {
                    node_id,
                    event: Some(event),
                    editors: vec![],
                },
                req_id,
            )
            .unwrap();
    }

    // Snapshot for req 10 changes options to ["X", "B", "C"] (idx 0 "A" -> "X" invalid, idx 1 "B" still matches!),
    // relabels checkbox to "NewFlag" (invalid), renames tab 11 to "OtherTab" (invalid).
    let invalidated = state
        .replace_for_request(
            make_snap(2, vec!["X", "B", "C"], 0, "NewFlag", "OtherTab"),
            10,
        )
        .unwrap();
    assert_eq!(invalidated, vec![11, 13, 14]);
    assert!(!state.has_optimistic_request(11));
    assert!(state.has_optimistic_request(12));
    assert!(!state.has_optimistic_request(13));
    assert!(!state.has_optimistic_request(14));
    assert!(matches!(
        state.node(2).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 1
    ));

    // Patch for req 11 replacing select options to ["B", "X", "C"] invalidates surviving req 12!
    let patch_invalidated = state
        .apply_for_request(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 2,
                    change: Some(patch_operation::Change::Replace(Node::new(
                        2,
                        "select",
                        node::Kind::Select(SelectNode {
                            label: "Choice".into(),
                            options: vec!["B".into(), "X".into(), "C".into()],
                            selected_index: 0,
                            enabled: true,
                        }),
                    ))),
                }],
            },
            11,
        )
        .unwrap();
    assert_eq!(patch_invalidated, vec![12]);
    assert!(!state.has_optimistic_request(12));
}

#[test]
fn uncertain_viewport_does_not_block_confirmed_base_snapshot_or_retraction() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: "uncertain-vp".into(),
        root: Some(Node::new(
            1,
            "tbl",
            node::Kind::VirtualTable(VirtualTable {
                columns: vec![TableColumn {
                    id: "c1".into(),
                    title: "Col".into(),
                    width: 80.0,
                }],
                total_rows: 20,
                offset: 0,
                viewport_revision: vp_rev,
                rows: vec![],
                multi_select: false,
            }),
        )),
    };

    let mut state = PanelState::new(make_snap(1, 1)).unwrap();
    state
        .observe_event_for_request(
            &UiEvent {
                node_id: 1,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 2,
                    offset: 10,
                    limit: 10,
                })),
                editors: vec![],
            },
            10,
        )
        .unwrap();

    // Active request 10 hides optimistic vp=2 from snapshot_with_confirmed_viewports.
    let active = HashSet::from([10]);
    let confirmed_view = state.snapshot_with_confirmed_viewports(&active);
    assert!(matches!(
        confirmed_view.root.as_ref().and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 1
    ));

    // Marking req 10 uncertain hides vp=2 even without active set and lets worker base vp=1 Snapshot succeed.
    assert!(state.mark_optimistic_request_uncertain(10));
    let confirmed_after_timeout = state.snapshot_with_confirmed_viewports(&HashSet::new());
    assert!(matches!(
        confirmed_after_timeout.root.as_ref().and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 1
    ));

    // Unsolicited (completed=0) or follow-up (completed=20) Snapshot with vp=1 MUST be accepted!
    let mut after_unsolicited = state.clone();
    after_unsolicited
        .replace_for_request(make_snap(2, 1), 0)
        .unwrap();
    assert!(matches!(
        after_unsolicited.node(1).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 1
    ));

    // Calling retract_uncertain_optimistic_requests removes req 10 and reverts self.snapshot to vp=1.
    assert!(state.retract_uncertain_optimistic_requests());
    assert!(!state.has_optimistic_request(10));
    assert!(matches!(
        state.node(1).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 1
    ));
}

#[test]
fn unclaimed_viewport_allows_unsolicited_base_snapshot_and_survives_rebase() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: "unclaimed-vp-rebase".into(),
        root: Some(Node::new(
            1,
            "tbl",
            node::Kind::VirtualTable(VirtualTable {
                columns: vec![TableColumn {
                    id: "c1".into(),
                    title: "Col".into(),
                    width: 80.0,
                }],
                total_rows: 20,
                offset: 0,
                viewport_revision: vp_rev,
                rows: vec![],
                multi_select: false,
            }),
        )),
    };

    let mut state = PanelState::new(make_snap(1, 1)).unwrap();
    state
        .observe_event_for_request(
            &UiEvent {
                node_id: 1,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 2,
                    offset: 10,
                    limit: 10,
                })),
                editors: vec![],
            },
            10,
        )
        .unwrap();

    // Unsolicited Snapshot(rev=2, vp=1) succeeds when req 10 is unclaimed, AND preserves req 10!
    let unclaimed = HashSet::from([10]);
    let invalidated = state
        .replace_for_request_with_unclaimed(make_snap(2, 1), 0, &unclaimed)
        .unwrap();
    assert!(invalidated.is_empty());
    assert!(state.has_optimistic_request(10));
    assert!(matches!(
        state.node(1).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 2
    ));

    // Unsolicited Snapshot(rev=3, vp=5) invalidates unclaimed req 10 (2 <= 5).
    let invalidated_superseded = state
        .replace_for_request_with_unclaimed(make_snap(3, 5), 0, &unclaimed)
        .unwrap();
    assert_eq!(invalidated_superseded, vec![10]);
    assert!(!state.has_optimistic_request(10));

    // settle_uncertain_optimistic_requests_through settles uncertain req 20 into base so confirmed view shows 6.
    state
        .observe_event_for_request(
            &UiEvent {
                node_id: 1,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 6,
                    offset: 10,
                    limit: 10,
                })),
                editors: vec![],
            },
            20,
        )
        .unwrap();
    assert!(state.mark_optimistic_request_uncertain(20));
    assert!(state.settle_uncertain_optimistic_requests_through(25));
    assert!(!state.has_optimistic_request(20));
    let view = state.snapshot_with_confirmed_viewports(&HashSet::new());
    assert!(matches!(
        view.root.as_ref().and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 6
    ));
}

#[test]
fn viewport_rebase_invalidates_out_of_range_offset_when_total_rows_shrinks() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64, total_rows: u64| PanelSnapshot {
        revision: rev,
        title: "vp-range-rebase".into(),
        root: Some(Node::new(
            1,
            "tbl",
            node::Kind::VirtualTable(VirtualTable {
                columns: vec![TableColumn {
                    id: "c1".into(),
                    title: "Col".into(),
                    width: 80.0,
                }],
                total_rows,
                offset: 0,
                viewport_revision: vp_rev,
                rows: vec![],
                multi_select: false,
            }),
        )),
    };

    // Unclaimed Viewport(2, offset=50) is invalidated when total_rows shrinks 100 -> 20
    // on both unsolicited Snapshot(completed=0) and causal Snapshot(completed=5).
    for completed in [0, 5] {
        let mut state = PanelState::new(make_snap(1, 1, 100)).unwrap();
        state
            .observe_event_for_request(
                &UiEvent {
                    node_id: 1,
                    event: Some(ui_event::Event::Viewport(ViewportRequest {
                        revision: 2,
                        offset: 50,
                        limit: 10,
                    })),
                    editors: vec![],
                },
                10,
            )
            .unwrap();
        let unclaimed = HashSet::from([10]);
        let invalidated = state
            .replace_for_request_with_unclaimed(make_snap(2, 1, 20), completed, &unclaimed)
            .unwrap();
        assert_eq!(invalidated, vec![10]);
        assert!(!state.has_optimistic_request(10));
    }

    // Also on apply_for_request_with_unclaimed (Patch replacing table to total_rows=20),
    // out-of-range offset=50 is invalidated whereas in-range offset=10 survives.
    let mut patched = PanelState::new(make_snap(1, 1, 100)).unwrap();
    patched
        .observe_event_for_request(
            &UiEvent {
                node_id: 1,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 2,
                    offset: 10,
                    limit: 10,
                })),
                editors: vec![],
            },
            11,
        )
        .unwrap();
    patched
        .observe_event_for_request(
            &UiEvent {
                node_id: 1,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 3,
                    offset: 50,
                    limit: 10,
                })),
                editors: vec![],
            },
            12,
        )
        .unwrap();
    let unclaimed = HashSet::from([11, 12]);
    let patch_invalidated = patched
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![PatchOperation {
                    node_id: 1,
                    change: Some(patch_operation::Change::Replace(
                        make_snap(2, 1, 20).root.unwrap(),
                    )),
                }],
            },
            0,
            &unclaimed,
        )
        .unwrap();
    assert_eq!(patch_invalidated, vec![12]);
    assert!(patched.has_optimistic_request(11));
    assert!(!patched.has_optimistic_request(12));
    assert!(matches!(
        patched.node(1).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 2 && t.total_rows == 20
    ));
}

#[test]
fn observe_event_rejects_non_zero_offset_equal_to_total_rows_and_detached_table_patch_validates() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64, total_rows: u64| PanelSnapshot {
        revision: rev,
        title: "vp-range-and-detached-patch".into(),
        root: Some(Node::new(
            1,
            "tbl",
            node::Kind::VirtualTable(VirtualTable {
                columns: vec![TableColumn {
                    id: "c1".into(),
                    title: "Col".into(),
                    width: 80.0,
                }],
                total_rows,
                offset: 0,
                viewport_revision: vp_rev,
                rows: vec![],
                multi_select: false,
            }),
        )),
    };

    // observe_event_for_request rejects non-zero offset == total_rows, allows offset == 0 on total_rows == 0,
    // and allows offset = 19 on total_rows = 20.
    let mut empty_state = PanelState::new(make_snap(1, 1, 0)).unwrap();
    empty_state
        .observe_event_for_request(
            &UiEvent {
                node_id: 1,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 2,
                    offset: 0,
                    limit: 10,
                })),
                editors: vec![],
            },
            1,
        )
        .unwrap();

    let mut non_empty = PanelState::new(make_snap(1, 1, 20)).unwrap();
    assert!(
        non_empty
            .observe_event_for_request(
                &UiEvent {
                    node_id: 1,
                    event: Some(ui_event::Event::Viewport(ViewportRequest {
                        revision: 2,
                        offset: 20,
                        limit: 10,
                    })),
                    editors: vec![],
                },
                2,
            )
            .is_err()
    );
    non_empty
        .observe_event_for_request(
            &UiEvent {
                node_id: 1,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 2,
                    offset: 19,
                    limit: 10,
                })),
                editors: vec![],
            },
            3,
        )
        .unwrap();

    // Claimed Viewport(2) detached by unsolicited Snapshot(rev=2, vp=1) still permits causal
    // Change::Table(vp=2) patch when req 3 finishes, and patch_confirms_detached_viewport returns true.
    let invalidated = non_empty
        .replace_for_request_with_unclaimed(make_snap(2, 1, 20), 0, &HashSet::new())
        .unwrap();
    assert!(invalidated.is_empty());
    let table_patch = PanelPatch {
        base_revision: 2,
        revision: 3,
        operations: vec![PatchOperation {
            node_id: 1,
            change: Some(patch_operation::Change::Table(VirtualTable {
                columns: vec![TableColumn {
                    id: "c1".into(),
                    title: "Col".into(),
                    width: 80.0,
                }],
                total_rows: 20,
                offset: 19,
                viewport_revision: 2,
                rows: vec![],
                multi_select: false,
            })),
        }],
    };
    assert!(non_empty.patch_confirms_detached_viewport(&table_patch, 3));
    let patch_invalidated = non_empty
        .apply_for_request_with_unclaimed(&table_patch, 3, &HashSet::new())
        .unwrap();
    assert!(patch_invalidated.is_empty());
    assert!(matches!(
        non_empty.node(1).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::VirtualTable(t)) if t.viewport_revision == 2 && t.offset == 19
    ));
}

#[test]
fn unsolicited_snapshot_covering_claimed_viewport_confirms_without_invalidation() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64| PanelSnapshot {
        revision: rev,
        title: "vp-confirm-test".into(),
        root: Some(Node::new(
            1,
            "tbl",
            node::Kind::VirtualTable(VirtualTable {
                columns: vec![TableColumn {
                    id: "c1".into(),
                    title: "Col".into(),
                    width: 80.0,
                }],
                total_rows,
                offset,
                viewport_revision: vp_rev,
                rows: vec![],
                multi_select: false,
            }),
        )),
    };
    let vp_event = UiEvent {
        node_id: 1,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 2,
            offset: 50,
            limit: 25,
        })),
        editors: vec![],
    };

    // 1. Claimed Viewport(2) still in optimistic_fields -> unsolicited Snapshot(vp=2, offset=50)
    //    confirms it cleanly (invalidated == []) and allows settle_optimistic_request to return true.
    let mut direct = PanelState::new(make_snap(1, 1, 0, 100)).unwrap();
    direct.observe_event_for_request(&vp_event, 10).unwrap();
    let invalidated = direct
        .replace_for_request_with_unclaimed(make_snap(2, 2, 50, 100), 0, &HashSet::new())
        .unwrap();
    assert!(invalidated.is_empty());
    assert!(!direct.has_optimistic_request(10));
    assert!(direct.has_confirmed_viewport_request(10));
    assert!(direct.has_tracked_optimistic_request(10));
    assert!(direct.settle_optimistic_request(10));
    assert!(!direct.has_tracked_optimistic_request(10));

    // 2. Claimed Viewport(2) detached by unsolicited Snapshot(vp=1) -> second unsolicited Snapshot(vp=2, offset=50)
    //    confirms the detached entry (invalidated == []) and allows settle_optimistic_request to return true.
    let mut detached = PanelState::new(make_snap(1, 1, 0, 100)).unwrap();
    detached.observe_event_for_request(&vp_event, 11).unwrap();
    assert!(detached
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(!detached.has_optimistic_request(11));
    assert!(detached.has_tracked_optimistic_request(11));
    assert!(detached
        .replace_for_request_with_unclaimed(make_snap(3, 2, 50, 100), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(detached.has_confirmed_viewport_request(11));
    assert!(detached.settle_optimistic_request(11));
    assert!(!detached.has_tracked_optimistic_request(11));

    // 3. Contrast: UNCLAIMED Viewport(2) when unsolicited Snapshot(vp=2) arrives is still returned as
    //    invalidated so the host retracts the obsolete unsent outbound frame.
    let mut unclaimed = PanelState::new(make_snap(1, 1, 0, 100)).unwrap();
    unclaimed.observe_event_for_request(&vp_event, 12).unwrap();
    let unclaimed_invalidated = unclaimed
        .replace_for_request_with_unclaimed(
            make_snap(2, 2, 50, 100),
            0,
            &HashSet::from([12]),
        )
        .unwrap();
    assert_eq!(unclaimed_invalidated, vec![12]);
    assert!(!unclaimed.has_confirmed_viewport_request(12));

    // 4. Contrast: Claimed detached Viewport(2, offset=50) when unsolicited Snapshot(vp=1, total_rows=20)
    //    shrinks rows below offset=50 still reports true range invalidation.
    let mut shrunk = PanelState::new(make_snap(1, 1, 0, 100)).unwrap();
    shrunk.observe_event_for_request(&vp_event, 13).unwrap();
    assert!(shrunk
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    let shrunk_invalidated = shrunk
        .replace_for_request_with_unclaimed(make_snap(3, 1, 0, 20), 0, &HashSet::new())
        .unwrap();
    assert_eq!(shrunk_invalidated, vec![13]);
    assert!(!shrunk.has_tracked_optimistic_request(13));
}

#[test]
fn watermark_only_loading_snapshot_and_out_of_order_causal_reply_preserve_viewport_tracking() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, text: &str| PanelSnapshot {
        revision: rev,
        title: "vp-loading-and-ooo".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        2,
                        "status",
                        node::Kind::Text(Text {
                            text: text.into(),
                            tone: Tone::Foreground as i32,
                        }),
                    ),
                    Node::new(
                        3,
                        "tbl",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows,
                            offset,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                ],
            }),
        )),
    };
    let vp_event = UiEvent {
        node_id: 1 + 2,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 2,
            offset: 50,
            limit: 25,
        })),
        editors: vec![],
    };

    // 1. Two-phase async worker publishes loading Snapshot(vp=2, offset=0) before target page arrives:
    //    MUST NOT target-confirm req 10 early or trigger invalidation; keeps it detached until target offset=50 arrives,
    //    while recognizing watermark-confirmed detached status so a timely Ack settles cleanly.
    let mut state = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    state.observe_event_for_request(&vp_event, 10).unwrap();
    assert!(state
        .replace_for_request_with_unclaimed(make_snap(2, 2, 0, 100, "loading"), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(!state.has_optimistic_request(10));
    assert!(!state.has_confirmed_viewport_request(10));
    assert!(state.has_detached_viewport(10));
    assert!(state.has_tracked_optimistic_request(10));
    assert!(state.settle_optimistic_request(10));
    assert!(!state.has_tracked_optimistic_request(10));

    // Any detached Viewport(2) (even detached on stale-watermark vp=1) settles cleanly on bare Ack
    // and preserves acked ceiling vp=2 so a later async Change::Table(vp=2) Patch succeeds (while vp=3 fails).
    let mut stale_detach = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    stale_detach.observe_event_for_request(&vp_event, 10).unwrap();
    assert!(stale_detach
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100, "stale"), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(stale_detach.has_detached_viewport(10));
    assert!(stale_detach.settle_optimistic_request(10));
    assert!(!stale_detach.has_tracked_optimistic_request(10));
    assert!(stale_detach
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 3,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .is_err());
    assert!(stale_detach
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());

    // Phase-1 loading followed by Phase-2 Snapshot(vp=2, offset=50) confirms target page and ignores later uncertain marking.
    let mut loading_then_target = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    loading_then_target.observe_event_for_request(&vp_event, 10).unwrap();
    assert!(loading_then_target
        .replace_for_request_with_unclaimed(make_snap(2, 2, 0, 100, "loading"), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(loading_then_target
        .replace_for_request_with_unclaimed(make_snap(3, 2, 50, 100, "ready"), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(loading_then_target.has_confirmed_viewport_request(10));
    assert!(!loading_then_target.mark_optimistic_request_uncertain(10));
    assert!(!loading_then_target.has_uncertain_optimistic_requests());

    // 2. Out-of-order later causal Patch/Snapshot (completed_request_id = 15 > 10) MUST NOT erase
    //    confirmed req 10 or earlier detached req 9 still waiting at vp=1.
    assert!(loading_then_target
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 3,
                revision: 4,
                operations: vec![PatchOperation {
                    node_id: 2,
                    change: Some(patch_operation::Change::Replace(Node::new(
                        2,
                        "status",
                        node::Kind::Text(Text {
                            text: "other-click".into(),
                            tone: Tone::Foreground as i32,
                        }),
                    ))),
                }],
            },
            15,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());
    assert!(loading_then_target.has_confirmed_viewport_request(10));
    assert!(loading_then_target.settle_optimistic_request(10));
    assert!(!loading_then_target.has_tracked_optimistic_request(10));

    let mut ooo_detached = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    ooo_detached.observe_event_for_request(&vp_event, 9).unwrap();
    assert!(ooo_detached
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100, "stale"), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(ooo_detached
        .replace_for_request_with_unclaimed(make_snap(3, 1, 0, 100, "later-req"), 15, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(ooo_detached.has_tracked_optimistic_request(9));

    // 3. Out-of-order later causal full Snapshot(rev=2, completed_request_id=15) while req 10 is STILL
    //    in optimistic_fields: both target-page Snapshot(vp=2, offset=50) and loading Snapshot(vp=2, offset=0)
    //    preserve tracking so req 10's subsequent bare Ack settles cleanly.
    let mut ooo_direct_target = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    ooo_direct_target.observe_event_for_request(&vp_event, 10).unwrap();
    assert!(ooo_direct_target
        .replace_for_request_with_unclaimed(make_snap(2, 2, 50, 100, "ready"), 15, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(ooo_direct_target.has_confirmed_viewport_request(10));
    assert!(ooo_direct_target.settle_optimistic_request(10));

    let mut ooo_direct_loading = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    ooo_direct_loading.observe_event_for_request(&vp_event, 10).unwrap();
    assert!(ooo_direct_loading
        .replace_for_request_with_unclaimed(make_snap(2, 2, 0, 100, "loading"), 15, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(ooo_direct_loading.has_detached_viewport(10));
    assert!(ooo_direct_loading.settle_optimistic_request(10));

    // 4. Out-of-order later causal full Snapshot(rev=2, completed_request_id=15) carrying the Worker's
    //    true older viewport vp=1 MUST pass watermark check, move req 10 to detached_viewports, and accept
    //    req 10's subsequent Change::Table(vp=2, offset=50) Patch; while req 10's OWN causal Snapshot(vp=1)
    //    MUST still fail!
    let mut ooo_old_vp = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    ooo_old_vp.observe_event_for_request(&vp_event, 10).unwrap();
    assert!(ooo_old_vp
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100, "by-15"), 15, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(!ooo_old_vp.has_optimistic_request(10));
    assert!(ooo_old_vp.has_tracked_optimistic_request(10));
    assert!(ooo_old_vp
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            10,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());
    assert!(!ooo_old_vp.has_tracked_optimistic_request(10));

    let mut own_causal = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    own_causal.observe_event_for_request(&vp_event, 10).unwrap();
    assert!(own_causal
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100, "own-stale"), 10, &HashSet::new())
        .is_err());
}

#[test]
fn bare_ack_viewport_rolls_back_floor_and_reverse_ooo_causal_snapshot_confirms_later_viewport() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, text: &str| PanelSnapshot {
        revision: rev,
        title: "ack-rollback-and-reverse-ooo".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        2,
                        "status",
                        node::Kind::Text(Text {
                            text: text.into(),
                            tone: Tone::Foreground as i32,
                        }),
                    ),
                    Node::new(
                        3,
                        "tbl",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows,
                            offset,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                ],
            }),
        )),
    };
    let read_vp = |state: &PanelState| match state.node(3).and_then(|n| n.kind.as_ref()) {
        Some(node::Kind::VirtualTable(t)) => t.viewport_revision,
        other => panic!("expected table, got {other:?}"),
    };
    let vp2_event = UiEvent {
        node_id: 3,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 2,
            offset: 50,
            limit: 25,
        })),
        editors: vec![],
    };
    let vp3_event = UiEvent {
        node_id: 3,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 3,
            offset: 75,
            limit: 25,
        })),
        editors: vec![],
    };

    // Part 1: Bare Ack on unconfirmed Viewport(2) settles without promoting ledger.base -> rolls host
    // viewport back to 1 so a later unrelated Snapshot(rev=2, vp=1) succeeds, while ALSO recording
    // acked ceiling vp=2 so a subsequent async Change::Table(vp=2) Patch succeeds (while forged vp=3 fails)!
    let mut bare_ack = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    bare_ack.observe_event_for_request(&vp2_event, 10).unwrap();
    assert_eq!(read_vp(&bare_ack), 2);
    assert!(bare_ack.settle_optimistic_request(10));
    assert_eq!(read_vp(&bare_ack), 1);
    assert!(bare_ack
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100, "after-ack"), 11, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(bare_ack
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 3,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .is_err());
    assert!(bare_ack
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());

    // Pipelined vp=2 (req 10) + vp=3 (req 11): settling req 10 via bare Ack keeps vp=3 & base=1;
    // settling req 11 via bare Ack rolls back to base=1.
    let mut pipelined = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    pipelined.observe_event_for_request(&vp2_event, 10).unwrap();
    pipelined.observe_event_for_request(&vp3_event, 11).unwrap();
    assert!(pipelined.settle_optimistic_request(10));
    assert_eq!(read_vp(&pipelined), 3);
    assert!(pipelined.settle_optimistic_request(11));
    assert_eq!(read_vp(&pipelined), 1);

    // Part 2: Reverse out-of-order: earlier causal request completed_request_id = 5 < 10 publishes
    // later claimed Viewport(2, offset=50) target page (via Snapshot OR Patch) -> confirms req 10 cleanly!
    let mut rev_snap = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    rev_snap.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(rev_snap
        .replace_for_request_with_unclaimed(make_snap(2, 2, 50, 100, "ready-by-5"), 5, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(rev_snap.has_confirmed_viewport_request(10));
    assert!(rev_snap.settle_optimistic_request(10));

    let mut rev_patch = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    rev_patch.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(rev_patch
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            5,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());
    assert!(rev_patch.has_confirmed_viewport_request(10));
    assert!(rev_patch.settle_optimistic_request(10));

    // And earlier completed_request_id = 5 delivering loading Snapshot(vp=2, offset=0) detaches req 10
    // with watermark confirmation so bare Ack settles cleanly.
    let mut rev_loading = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    rev_loading.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(rev_loading
        .replace_for_request_with_unclaimed(make_snap(2, 2, 0, 100, "loading-by-5"), 5, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(rev_loading.has_detached_viewport(10));
    assert!(rev_loading.settle_optimistic_request(10));
}

#[test]
fn ancestor_replace_causal_old_vp_patch_and_discard_handle_acked_viewports_accurately() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, text: &str| PanelSnapshot {
        revision: rev,
        title: "acked-viewport-edge-cases".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        2,
                        "status",
                        node::Kind::Text(Text {
                            text: text.into(),
                            tone: Tone::Foreground as i32,
                        }),
                    ),
                    Node::new(
                        3,
                        "tbl",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows,
                            offset,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                ],
            }),
        )),
    };
    let vp2_event = UiEvent {
        node_id: 3,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 2,
            offset: 50,
            limit: 25,
        })),
        editors: vec![],
    };

    // 1. P2-①: After bare Ack settles Viewport(2, offset=50), an ancestor Replace(Column 1) updating only
    //    status text MUST preserve acked_viewports(3 -> (2, 50)), whereas shrinking total_rows=20 prunes it.
    let mut state = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    state.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(state.settle_optimistic_request(10));
    assert!(state
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![PatchOperation {
                    node_id: 1,
                    change: Some(patch_operation::Change::Replace(
                        make_snap(2, 1, 0, 100, "status-updated").root.unwrap(),
                    )),
                }],
            },
            0,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());
    assert!(state
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());

    let mut shrunk = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    shrunk.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(shrunk.settle_optimistic_request(10));
    assert!(shrunk
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![PatchOperation {
                    node_id: 1,
                    change: Some(patch_operation::Change::Replace(
                        make_snap(2, 1, 0, 20, "shrunk").root.unwrap(),
                    )),
                }],
            },
            0,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());
    assert!(shrunk
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 20,
                        offset: 0,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .is_err());

    // 1b. P2-① (gap check): If BOTH vp=2 (offset=50) and vp=3 (offset=10) are bare-Acked and then
    //     Snapshot(rev=2, vp=1, total_rows=20) prunes (2, 50) while keeping (3, 10), stale Change::Table(vp=2)
    //     MUST still be rejected (membership check, not floor..=ceiling!), while valid Change::Table(vp=3) passes!
    let vp3_event = UiEvent {
        node_id: 3,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 3,
            offset: 10,
            limit: 25,
        })),
        editors: vec![],
    };
    let mut gapped = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    gapped.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(gapped.settle_optimistic_request(10));
    gapped.observe_event_for_request(&vp3_event, 11).unwrap();
    assert!(gapped.settle_optimistic_request(11));
    assert!(gapped
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 20, "shrunk-20"), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(gapped
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .is_err());
    assert!(gapped
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 20,
                        offset: 10,
                        viewport_revision: 3,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());

    // 2. P2-②: Detached Viewport(2, offset=50) replying with a causal Patch that still keeps vp=1
    //    (e.g., total_rows update while async query runs) MUST record (2, 50) in acked_viewports
    //    before removing the detached entry so a follow-up Change::Table(vp=2, offset=50) Patch succeeds.
    let mut causal_old = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    causal_old.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(causal_old
        .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100, "unsolicited"), 0, &HashSet::new())
        .unwrap()
        .is_empty());
    assert!(causal_old
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 120,
                        offset: 0,
                        viewport_revision: 1,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            10,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());
    assert!(causal_old
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 3,
                revision: 4,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 120,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());

    // 3. P2-③: retract_optimistic_request removes req 10 and rolls table back to vp=1 WITHOUT
    //    recording (2, 50) in acked_viewports (and rolls non-Viewport Toggle/Select back to ledger.base).
    let mut retracted = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
    retracted.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(retracted.retract_optimistic_request(10));
    assert!(retracted
        .apply_for_request_with_unclaimed(
            &PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![PatchOperation {
                    node_id: 3,
                    change: Some(patch_operation::Change::Table(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            },
            0,
            &HashSet::new(),
        )
        .is_err());

    let mut sel_retract = PanelState::new(PanelSnapshot {
        revision: 1,
        title: "select-retract".into(),
        root: Some(Node::new(
            1,
            "sel",
            node::Kind::Select(SelectNode {
                label: "Mode".into(),
                options: vec!["A".into(), "B".into()],
                selected_index: 0,
                enabled: true,
            }),
        )),
    })
    .unwrap();
    sel_retract
        .observe_event_for_request(
            &UiEvent {
                node_id: 1,
                event: Some(ui_event::Event::SelectedOption(1)),
                editors: vec![],
            },
            20,
        )
        .unwrap();
    assert!(sel_retract.retract_optimistic_request(20));
    assert!(matches!(
        sel_retract.node(1).and_then(|n| n.kind.as_ref()),
        Some(node::Kind::Select(s)) if s.selected_index == 0
    ));
}

#[test]
fn pruned_viewport_rejected_on_unsolicited_snapshot_and_ancestor_replace_but_allowed_on_resync() {
    use std::collections::HashSet;

    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64| PanelSnapshot {
        revision: rev,
        title: "pruned-vp-snapshot-replace".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        2,
                        "status",
                        node::Kind::Text(Text {
                            text: format!("rev-{rev}"),
                            tone: Tone::Foreground as i32,
                        }),
                    ),
                    Node::new(
                        3,
                        "tbl",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows,
                            offset,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                ],
            }),
        )),
    };
    let vp2_event = UiEvent {
        node_id: 3,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 2,
            offset: 50,
            limit: 25,
        })),
        editors: vec![],
    };
    let vp3_event = UiEvent {
        node_id: 3,
        event: Some(ui_event::Event::Viewport(ViewportRequest {
            revision: 3,
            offset: 10,
            limit: 25,
        })),
        editors: vec![],
    };

    // 1. After vp=2 (offset=50) and vp=3 (offset=10) are bare-Acked and Snapshot(rev=2, vp=1, total_rows=20)
    //    prunes vp=2, stale vp=2 carrying total_rows=100 MUST be rejected via:
    //    - unsolicited Snapshot (replace_for_request_with_unclaimed)
    //    - ancestor Replace(Column) Patch
    //    - direct Replace(VirtualTable) Patch
    //    - observe_event_for_request(Viewport(2, offset=10))
    //    While unrelated Resync Snapshot(rev=3, vp=1, total_rows=20) succeeds AND preserves pruned vp=2,
    //    and valid Change::Table(vp=3, offset=10, total_rows=20) succeeds and retires pruned vp=2!
    let mut state = PanelState::new(make_snap(1, 1, 0, 100)).unwrap();
    state.observe_event_for_request(&vp2_event, 10).unwrap();
    assert!(state.settle_optimistic_request(10));
    state.observe_event_for_request(&vp3_event, 11).unwrap();
    assert!(state.settle_optimistic_request(11));
    assert!(
        state
            .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 20), 0, &HashSet::new())
            .unwrap()
            .is_empty()
    );

    assert!(
        state
            .replace_for_request_with_unclaimed(make_snap(3, 2, 50, 100), 0, &HashSet::new())
            .is_err()
    );
    assert!(
        state
            .apply_for_request_with_unclaimed(
                &PanelPatch {
                    base_revision: 2,
                    revision: 3,
                    operations: vec![PatchOperation {
                        node_id: 1,
                        change: Some(patch_operation::Change::Replace(
                            make_snap(3, 2, 50, 100).root.unwrap(),
                        )),
                    }],
                },
                0,
                &HashSet::new(),
            )
            .is_err()
    );
    let table_vp2_100 = Node::new(
        3,
        "tbl",
        node::Kind::VirtualTable(VirtualTable {
            columns: vec![TableColumn {
                id: "c1".into(),
                title: "Col".into(),
                width: 80.0,
            }],
            total_rows: 100,
            offset: 50,
            viewport_revision: 2,
            rows: vec![],
            multi_select: false,
        }),
    );
    assert!(
        state
            .apply_for_request_with_unclaimed(
                &PanelPatch {
                    base_revision: 2,
                    revision: 3,
                    operations: vec![PatchOperation {
                        node_id: 3,
                        change: Some(patch_operation::Change::Replace(table_vp2_100)),
                    }],
                },
                0,
                &HashSet::new(),
            )
            .is_err()
    );
    assert!(
        state
            .observe_event_for_request(
                &UiEvent {
                    node_id: 3,
                    event: Some(ui_event::Event::Viewport(ViewportRequest {
                        revision: 2,
                        offset: 10,
                        limit: 25,
                    })),
                    editors: vec![],
                },
                12,
            )
            .is_err()
    );

    // Unrelated Resync Snapshot(rev=3, vp=1, total_rows=20) succeeds and still keeps vp=2 pruned,
    // whereas a Resync Snapshot trying to restore the pruned (vp=2, offset=50) is REJECTED!
    let mut with_resync = state.clone();
    assert!(
        with_resync
            .replace_for_resync_with_unclaimed(make_snap(3, 1, 0, 20), 99, &HashSet::new())
            .unwrap()
            .is_empty()
    );
    assert!(
        with_resync
            .replace_for_resync_with_unclaimed(make_snap(4, 2, 50, 100), 99, &HashSet::new())
            .is_err()
    );
    assert!(
        with_resync
            .replace_for_request_with_unclaimed(make_snap(4, 2, 50, 100), 0, &HashSet::new())
            .is_err()
    );
    // Authoritative Resync Snapshot(rev=4, vp=2, offset=0, total_rows=20) is allowed and marks vp=2
    // clamped at offset=0 (rather than deleting the pruned record for (vp=2, offset=50))!
    assert!(
        with_resync
            .replace_for_resync_with_unclaimed(make_snap(4, 2, 0, 20), 100, &HashSet::new())
            .unwrap()
            .is_empty()
    );
    // Even a subsequent Resync Snapshot trying to restore pruned offset=50 at vp=2 MUST be rejected
    // and keep the isolation record intact, while clamped (vp=2, offset=0) Resync still succeeds!
    assert!(
        with_resync
            .replace_for_resync_with_unclaimed(make_snap(5, 2, 50, 100), 101, &HashSet::new())
            .is_err()
    );
    assert!(
        with_resync
            .replace_for_resync_with_unclaimed(make_snap(5, 2, 0, 20), 102, &HashSet::new())
            .unwrap()
            .is_empty()
    );
    // Stale (vp=2, offset=50, total_rows=100) MUST still be rejected across Change::Table,
    // unsolicited Snapshot, AND ancestor Replace(Column) even after clamped Resync!
    assert!(
        with_resync
            .apply_for_request_with_unclaimed(
                &PanelPatch {
                    base_revision: 5,
                    revision: 6,
                    operations: vec![PatchOperation {
                        node_id: 3,
                        change: Some(patch_operation::Change::Table(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 100,
                            offset: 50,
                            viewport_revision: 2,
                            rows: vec![],
                            multi_select: false,
                        })),
                    }],
                },
                0,
                &HashSet::new(),
            )
            .is_err()
    );
    assert!(
        with_resync
            .replace_for_request_with_unclaimed(make_snap(6, 2, 50, 100), 0, &HashSet::new())
            .is_err()
    );
    assert!(
        with_resync
            .apply_for_request_with_unclaimed(
                &PanelPatch {
                    base_revision: 5,
                    revision: 6,
                    operations: vec![PatchOperation {
                        node_id: 1,
                        change: Some(patch_operation::Change::Replace(
                            make_snap(6, 2, 50, 100).root.unwrap(),
                        )),
                    }],
                },
                0,
                &HashSet::new(),
            )
            .is_err()
    );
    // Meanwhile, valid updates at the clamped (vp=2, offset=0) pass while stale (vp=2, offset=50)
    // remains rejected, and advancing to vp=3 retires the clamped pruned entry.
    assert!(
        with_resync
            .apply_for_request_with_unclaimed(
                &PanelPatch {
                    base_revision: 5,
                    revision: 6,
                    operations: vec![PatchOperation {
                        node_id: 3,
                        change: Some(patch_operation::Change::Table(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 25,
                            offset: 0,
                            viewport_revision: 2,
                            rows: vec![],
                            multi_select: false,
                        })),
                    }],
                },
                0,
                &HashSet::new(),
            )
            .unwrap()
            .is_empty()
    );
    assert!(
        with_resync
            .replace_for_request_with_unclaimed(make_snap(7, 2, 0, 25), 0, &HashSet::new())
            .unwrap()
            .is_empty()
    );
    assert!(
        with_resync
            .replace_for_request_with_unclaimed(make_snap(8, 2, 50, 100), 0, &HashSet::new())
            .is_err()
    );
    assert!(
        with_resync
            .apply_for_request_with_unclaimed(
                &PanelPatch {
                    base_revision: 7,
                    revision: 8,
                    operations: vec![PatchOperation {
                        node_id: 3,
                        change: Some(patch_operation::Change::Table(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 25,
                            offset: 10,
                            viewport_revision: 3,
                            rows: vec![],
                            multi_select: false,
                        })),
                    }],
                },
                0,
                &HashSet::new(),
            )
            .unwrap()
            .is_empty()
    );

    // On original state, valid Change::Table(vp=3, offset=10, total_rows=20) passes and retires vp=2.
    assert!(
        state
            .apply_for_request_with_unclaimed(
                &PanelPatch {
                    base_revision: 2,
                    revision: 3,
                    operations: vec![PatchOperation {
                        node_id: 3,
                        change: Some(patch_operation::Change::Table(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 20,
                            offset: 10,
                            viewport_revision: 3,
                            rows: vec![],
                            multi_select: false,
                        })),
                    }],
                },
                0,
                &HashSet::new(),
            )
            .unwrap()
            .is_empty()
    );

    // 2. Claimed detached/pending Viewport(2, offset=50) pruned by Snapshot(vp=1, total_rows=20)
    //    also blocks a subsequent unsolicited Snapshot(vp=2, offset=50, total_rows=100).
    let mut claimed_pruned = PanelState::new(make_snap(1, 1, 0, 100)).unwrap();
    claimed_pruned.observe_event_for_request(&vp2_event, 20).unwrap();
    assert!(
        claimed_pruned
            .replace_for_request_with_unclaimed(make_snap(2, 1, 0, 100), 0, &HashSet::new())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        claimed_pruned
            .replace_for_request_with_unclaimed(make_snap(3, 1, 0, 20), 0, &HashSet::new())
            .unwrap(),
        vec![20]
    );
    assert!(
        claimed_pruned
            .replace_for_request_with_unclaimed(make_snap(4, 2, 50, 100), 0, &HashSet::new())
            .is_err()
    );

    // 3. settle_uncertain_optimistic_requests_through_with_promoted distinguishes promoted
    //    optimistic ledger entries from detached uncertain viewport entries.
    let mut mixed = PanelState::new(PanelSnapshot {
        revision: 1,
        title: "mixed-settle".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        2,
                        "tbl",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 100,
                            offset: 0,
                            viewport_revision: 1,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                    Node::new(
                        3,
                        "sel",
                        node::Kind::Select(SelectNode {
                            label: "Mode".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index: 0,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    })
    .unwrap();
    mixed
        .observe_event_for_request(
            &UiEvent {
                node_id: 2,
                event: Some(ui_event::Event::Viewport(ViewportRequest {
                    revision: 2,
                    offset: 50,
                    limit: 25,
                })),
                editors: vec![],
            },
            30,
        )
        .unwrap();
    // Unsolicited Snapshot detaches req 30.
    let mut detach_snap = mixed.snapshot().clone();
    detach_snap.revision = 2;
    if let Some(node::Kind::Column(col)) = detach_snap.root.as_mut().and_then(|r| r.kind.as_mut())
        && let Some(node::Kind::VirtualTable(t)) = col.children[0].kind.as_mut()
    {
        t.viewport_revision = 1;
        t.offset = 0;
    }
    mixed.replace(detach_snap).unwrap();
    mixed
        .observe_event_for_request(
            &UiEvent {
                node_id: 3,
                event: Some(ui_event::Event::SelectedOption(1)),
                editors: vec![],
            },
            31,
        )
        .unwrap();
    assert!(mixed.mark_optimistic_request_uncertain(30));
    assert!(mixed.mark_optimistic_request_uncertain(31));
    let (settled, promoted) = mixed.settle_uncertain_optimistic_requests_through_with_promoted(32);
    assert!(settled);
    assert!(promoted.contains(&31));
    assert!(!promoted.contains(&30));
    assert!(!mixed.has_tracked_optimistic_request(30));
    assert!(!mixed.has_tracked_optimistic_request(31));
}
