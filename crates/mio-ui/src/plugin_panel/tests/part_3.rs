use super::*;

#[gpui_kit::test]
fn tabs_emit_child_node_id_and_preserve_editor_identity_across_switches(cx: &mut TestAppContext) {
    let snapshot = PanelSnapshot {
        revision: 1,
        title: "Tabs".into(),
        root: Some(Node::new(
            1,
            "Tabs",
            node::Kind::Tabs(Tabs {
                axis: Axis::Vertical as i32,
                active: 101,
                tabs: vec![
                    Tab {
                        title: "A".into(),
                        content: Some(Box::new(editor(101, 4001, b"A", EditorFormat::Utf8))),
                    },
                    Tab {
                        title: "B".into(),
                        content: Some(Box::new(editor(907, 8002, b"B", EditorFormat::Utf8))),
                    },
                ],
            }),
        )),
    };
    let fixture = fixture(cx, snapshot);
    let original = cx.read(|cx| fixture.view.read(cx).documents[&4001].state.entity_id());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("plugin-tab", 907u64), cx);
        window.click(("plugin-tab", 101u64), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let selected: Vec<_> = fixture
        .events
        .borrow()
        .iter()
        .filter_map(|event| match event.event {
            Some(ui_event::Event::TabSelected(id)) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(selected, [907, 101]);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).documents[&4001].state.entity_id(),
            original
        )
    });
}

#[gpui_kit::test]
fn split_drag_reports_actual_fraction_through_pointer_events(cx: &mut TestAppContext) {
    let snapshot = PanelSnapshot {
        revision: 1,
        title: "Split".into(),
        root: Some(Node::new(
            1,
            "Split",
            node::Kind::Split(Split {
                axis: Axis::Horizontal as i32,
                fraction: 0.5,
                first: Some(Box::new(editor(4, 401, b"A", EditorFormat::Utf8))),
                second: Some(Box::new(editor(5, 902, b"B", EditorFormat::Utf8))),
            }),
        )),
    };
    let fixture = fixture(cx, snapshot);
    cx.update_window(fixture.window, |_, window, cx| {
        let center = window.find(("plugin-split", 1u64)).bounds().center();
        window.pointer_down(center, MouseButton::Left, cx);
        let position = point(px(630.), center.y);
        window.pointer_move(position, Some(MouseButton::Left), cx);
        window.pointer_up(position, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(fixture.events.borrow().iter().any(|event| matches!(event.event, Some(ui_event::Event::SplitResized(fraction)) if (fraction - 0.7).abs() < 0.02)));
    cx.read(|cx| assert!(fixture.view.read(cx).resize.is_none()));
}

#[gpui_kit::test]
fn table_real_selection_keys_filter_and_failed_page_are_retryable(cx: &mut TestAppContext) {
    let snapshot = PanelSnapshot {
        revision: 1,
        title: "Table".into(),
        root: Some(Node::new(
            10,
            "Table",
            node::Kind::VirtualTable(panel::VirtualTable {
                columns: vec![panel::TableColumn {
                    id: "value".into(),
                    title: "Value".into(),
                    width: 400.,
                }],
                total_rows: 1000,
                offset: 0,
                viewport_revision: 0,
                multi_select: true,
                rows: ["a", "b", "c"]
                    .into_iter()
                    .map(|id| panel::TableRow {
                        id: id.into(),
                        cells: vec![format!("Row {id}")],
                    })
                    .collect(),
            }),
        )),
    };
    let fixture = fixture(cx, snapshot);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("plugin-row", 0usize), cx);
        window.click_with_modifiers(("plugin-row", 2usize), crate::secondary_modifiers(true), cx);
        assert_eq!(fixture.view.read(cx).tables[&10].selected, ["a", "c"]);
        window.click_with_modifiers(
            ("plugin-row", 1usize),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            cx,
        );
        assert_eq!(fixture.view.read(cx).tables[&10].selected, ["b", "c"]);
        window.press("up", cx);
        assert_eq!(fixture.view.read(cx).tables[&10].selected, ["a"]);
        window.press("shift-down", cx);
        assert_eq!(fixture.view.read(cx).tables[&10].selected, ["a", "b"]);
        window.press(crate::kbd!("a"), cx);
        assert_eq!(fixture.view.read(cx).tables[&10].selected, ["a", "b", "c"]);
        let filter = fixture.view.read(cx).tables[&10].filter.clone();
        window.click(("input", filter.entity_id()), cx);
        window.input("中文条件", cx);
        window.press("enter", cx);
        window.click(("plugin-next", 10u64), cx);
        assert!(fixture.view.read(cx).tables[&10].requested_offset.is_some());
        fixture.view.update(cx, |view, cx| {
            view.event_failed(Some(10), "deadline expired", cx)
        });
        window.render_frame(cx);
        window.click(("plugin-next", 10u64), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let events = fixture.events.borrow();
    assert!(events.iter().any(
        |event| matches!(&event.event, Some(ui_event::Event::Filter(query)) if query == "中文条件")
    ));
    let pages: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            Some(ui_event::Event::Viewport(page)) => Some((page.revision, page.offset)),
            _ => None,
        })
        .collect();
    assert_eq!(pages, [(1, 3), (2, 3)]);
    assert!(events.iter().any(|event| matches!(&event.event, Some(ui_event::Event::Key(key)) if key.key == "down" && key.modifiers == 2)));
}

#[gpui_kit::test]
fn canvas_capture_keeps_local_coordinates_and_escape_has_one_terminal_event(
    cx: &mut TestAppContext,
) {
    let snapshot = PanelSnapshot {
        revision: 1,
        title: "Canvas".into(),
        root: Some(Node::new(
            77,
            "Canvas",
            node::Kind::Canvas(panel::Canvas {
                width: 400.,
                height: 200.,
                commands: vec![],
                hit_regions: vec![panel::HitRegion {
                    id: 9,
                    bounds: Some(panel::Rect {
                        x: 10.,
                        y: 20.,
                        width: 60.,
                        height: 30.,
                    }),
                    accessibility_label: "drag area".into(),
                    focusable: true,
                    capture_pointer: true,
                }],
            }),
        )),
    };
    let fixture = fixture(cx, snapshot);
    cx.update_window(fixture.window, |_, window, cx| {
        let origin = window.find(("plugin-canvas", 77u64)).bounds().origin;
        let start = origin + point(px(40.), px(35.));
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(
            origin + point(px(320.), px(150.)),
            Some(MouseButton::Left),
            cx,
        );
        window.pointer_up(start, MouseButton::Left, cx);
        window.pointer_down(start, MouseButton::Left, cx);
        window.press("escape", cx);
        window.pointer_up(start, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let events = fixture.events.borrow();
    let pointers: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            Some(ui_event::Event::Pointer(pointer)) => Some(pointer),
            _ => None,
        })
        .collect();
    assert_eq!(
        pointers
            .iter()
            .filter(|event| event.phase == PointerPhase::Down as i32)
            .count(),
        2
    );
    assert_eq!(
        pointers
            .iter()
            .filter(|event| event.phase == PointerPhase::Up as i32)
            .count(),
        1,
        "captured Up cannot be duplicated by the hover handler or emitted after Cancel"
    );
    assert_eq!(
        pointers
            .iter()
            .filter(|event| event.phase == PointerPhase::Cancel as i32)
            .count(),
        1
    );
    assert!(
        pointers
            .iter()
            .any(|event| event.phase == PointerPhase::Move as i32
                && event.x == 320.
                && event.y == 150.)
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, Some(ui_event::Event::Focus(true))))
    );
    cx.read(|cx| assert!(fixture.view.read(cx).capture.is_none()));
}

#[gpui_kit::test]
fn visiting_a_readonly_response_keeps_the_last_writable_send_document(cx: &mut TestAppContext) {
    let mut response = editor(321, 303, b"response", EditorFormat::Utf8);
    let Some(node::Kind::Editor(contents)) = &mut response.kind else {
        unreachable!()
    };
    contents.read_only = true;
    let fixture = fixture(
        cx,
        PanelSnapshot {
            revision: 1,
            title: "Mixed tabs".into(),
            root: Some(Node::new(
                1,
                "tabs",
                node::Kind::Tabs(Tabs {
                    axis: Axis::Horizontal as i32,
                    active: 101,
                    tabs: vec![
                        Tab {
                            title: "one".into(),
                            content: Some(Box::new(editor(
                                101,
                                4001,
                                b"first",
                                EditorFormat::Utf8,
                            ))),
                        },
                        Tab {
                            title: "two".into(),
                            content: Some(Box::new(editor(
                                907,
                                9007,
                                b"second",
                                EditorFormat::Utf8,
                            ))),
                        },
                        Tab {
                            title: "response".into(),
                            content: Some(Box::new(response)),
                        },
                    ],
                }),
            )),
        },
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("plugin-tab", 907u64), cx);
        assert_eq!(fixture.view.read(cx).focused_document(), Some(9007));
        window.click(("plugin-tab", 321u64), cx);
        assert_eq!(
            fixture.view.read(cx).focused_document(),
            Some(9007),
            "read-only tabs must not silently switch Send back to an older request editor"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn scroll_column_checkbox_select_badge_and_progress_render_and_dispatch_ui_events(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::{TestEventExt, TestQueryExt};
    use mio_core::panel::{BadgeNode, CheckboxNode, ProgressNode, SelectNode, Tone};

    let fixture = fixture(
        cx,
        PanelSnapshot {
            revision: 1,
            title: "Rich Interactive Panel".into(),
            root: Some(Node::new(
                1,
                "scroll-root",
                node::Kind::ScrollColumn(Container {
                    children: vec![
                        Node::new(
                            10,
                            "自动标记",
                            node::Kind::Checkbox(CheckboxNode {
                                label: "自动标记 Hook".into(),
                                checked: false,
                                enabled: true,
                            }),
                        ),
                        Node::new(
                            11,
                            "编码选择",
                            node::Kind::Select(SelectNode {
                                label: "编码".into(),
                                options: vec!["Standard".into(), "URLSafe".into()],
                                selected_index: 0,
                                enabled: true,
                            }),
                        ),
                        Node::new(
                            12,
                            "状态徽章",
                            node::Kind::Badge(BadgeNode {
                                text: "READY".into(),
                                tone: Tone::Accent as i32,
                            }),
                        ),
                        Node::new(
                            13,
                            "任务进度",
                            node::Kind::Progress(ProgressNode {
                                label: "完成".into(),
                                percent: 75,
                            }),
                        ),
                    ],
                }),
            )),
        },
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(("plugin-badge", 12u64)).visible());
        assert!(window.find(("plugin-progress-box", 13u64)).visible());
        window.click(("plugin-checkbox", 10u64), cx);
        window.click(("plugin-select", 11u64), cx);
        window.render_frame(cx);
        let pos = window.find_by_label("URLSafe").bounds().center();
        window.pointer_down(pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    let events = fixture.events.borrow();
    assert!(
        events
            .iter()
            .any(|ev| ev.node_id == 10 && matches!(ev.event, Some(ui_event::Event::Toggled(true))))
    );
    assert!(
        events
            .iter()
            .any(|ev| ev.node_id == 11
                && matches!(ev.event, Some(ui_event::Event::SelectedOption(1))))
    );
}

#[gpui_kit::test]
fn table_allocator_separates_from_effective_watermark_across_failure_and_recovery(
    cx: &mut TestAppContext,
) {
    use mio_core::panel::{
        Container, SelectNode, TableColumn, TableRow, VirtualTable, commit_panel_semantically_matches,
    };

    let make_snapshot = |rev: u64, vp_rev: u64, offset: u64, selected_index: u32| PanelSnapshot {
        revision: rev,
        title: "Watermark Test".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    editor(4, 401, b"hello", EditorFormat::Utf8),
                    Node::new(
                        10,
                        "Table",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "value".into(),
                                title: "Value".into(),
                                width: 400.,
                            }],
                            total_rows: 1000,
                            offset,
                            viewport_revision: vp_rev,
                            multi_select: false,
                            rows: ["a", "b", "c"]
                                .into_iter()
                                .map(|id| TableRow {
                                    id: id.into(),
                                    cells: vec![format!("Row {id}")],
                                })
                                .collect(),
                        }),
                    ),
                    Node::new(
                        11,
                        "Choice",
                        node::Kind::Select(SelectNode {
                            label: "Choice".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };

    let doc = EditorSnapshot {
        document_id: 401,
        revision: 1,
        contents: b"hello".to_vec(),
        selection_start: 0,
        selection_end: 0,
    };
    let read_commit_vp = |snap: &PanelSnapshot| -> u64 {
        match snap.root.as_ref().unwrap().find(10).unwrap().kind.as_ref().unwrap() {
            node::Kind::VirtualTable(t) => t.viewport_revision,
            other => panic!("expected VirtualTable, got {other:?}"),
        }
    };

    let fixture = fixture(cx, make_snapshot(1, 1, 0, 0));

    // 1. Page forward: allocator = 2, active watermark = 2.
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("plugin-next", 10u64), cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.tables[&10].next_viewport_revision, 2);
        assert_eq!(view.tables[&10].active_viewport_revision(), 2);
        assert_eq!(read_commit_vp(&view.commit_snapshot()), 2);

        // Direct node event failure resets effective watermark to 1 while keeping allocator at 2.
        fixture.view.update(cx, |view, cx| {
            view.event_failed(Some(10), "pre-dispatch failed", cx);
        });
        let view = fixture.view.read(cx);
        assert_eq!(view.tables[&10].next_viewport_revision, 2);
        assert_eq!(view.tables[&10].active_viewport_revision(), 1);
        assert!(view.tables[&10].requested_offset.is_none());
        let commit_snap = view.commit_snapshot();
        assert!(commit_panel_semantically_matches(
            Some(&make_snapshot(1, 1, 0, 0)),
            &commit_snap,
            &doc,
        ));

        // 2. Retry page allocates 3; optimistic select sets 1 -> recovery Snapshot(rev=2, vp=1, select=0) adopts vp=1 & clears selects.
        window.render_frame(cx);
        window.click(("plugin-next", 10u64), cx);
        fixture.view.update(cx, |view, cx| {
            push_overlay(&mut view.selects, 11, 1, 99);
            assert_eq!(view.tables[&10].next_viewport_revision, 3);
            assert_eq!(view.tables[&10].active_viewport_revision(), 3);
            view.replace(Arc::new(make_snapshot(2, 1, 0, 0)), &[], window, cx)
                .unwrap();
        });
        let view = fixture.view.read(cx);
        assert_eq!(view.tables[&10].next_viewport_revision, 3);
        assert_eq!(view.tables[&10].active_viewport_revision(), 1);
        assert!(view.tables[&10].requested_offset.is_none());
        assert!(view.selects.is_empty());
        assert!(commit_panel_semantically_matches(
            Some(&make_snapshot(2, 1, 0, 0)),
            &view.commit_snapshot(),
            &doc,
        ));

        // 3. Page again -> allocates 4; same-revision PanelSynced Snapshot(rev=2, vp=1, select=0) rolls active back to 1.
        window.render_frame(cx);
        window.click(("plugin-next", 10u64), cx);
        fixture.view.update(cx, |view, cx| {
            assert_eq!(view.tables[&10].next_viewport_revision, 4);
            assert_eq!(view.tables[&10].active_viewport_revision(), 4);
            view.replace(Arc::new(make_snapshot(2, 1, 0, 0)), &[], window, cx)
                .unwrap();
        });
        let view = fixture.view.read(cx);
        assert_eq!(view.tables[&10].next_viewport_revision, 4);
        assert_eq!(view.tables[&10].active_viewport_revision(), 1);
        assert!(view.tables[&10].requested_offset.is_none());
        assert_eq!(read_commit_vp(&view.commit_snapshot()), 1);

        // Next page click uses monotonic revision 5.
        window.render_frame(cx);
        window.click(("plugin-next", 10u64), cx);
    })
    .unwrap();
    cx.run_until_parked();

    let revisions: Vec<_> = fixture
        .events
        .borrow()
        .iter()
        .filter_map(|ev| match &ev.event {
            Some(ui_event::Event::Viewport(v)) => Some(v.revision),
            _ => None,
        })
        .collect();
    assert_eq!(revisions, [2, 3, 4, 5]);
}

#[gpui_kit::test]
fn same_revision_panel_synced_preserves_newer_local_overrides_by_interaction_seq(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::{TestEventExt, TestQueryExt};
    use mio_core::panel::{
        CheckboxNode, Container, SelectNode, TableColumn, TableRow, VirtualTable,
    };

    let make_snapshot =
        |checked: bool, selected_index: u32, vp_rev: u64| PanelSnapshot {
            revision: 1,
            title: "Seq Sync Panel".into(),
            root: Some(Node::new(
                1,
                "col",
                node::Kind::Column(Container {
                    children: vec![
                        editor(4, 401, b"hello", EditorFormat::Utf8),
                        Node::new(
                            10,
                            "checkbox",
                            node::Kind::Checkbox(CheckboxNode {
                                label: "Enable Flag".into(),
                                checked,
                                enabled: true,
                            }),
                        ),
                        Node::new(
                            11,
                            "select",
                            node::Kind::Select(SelectNode {
                                label: "Choice".into(),
                                options: vec!["A".into(), "B".into()],
                                selected_index,
                                enabled: true,
                            }),
                        ),
                        Node::new(
                            12,
                            "table",
                            node::Kind::VirtualTable(VirtualTable {
                                columns: vec![TableColumn {
                                    id: "c1".into(),
                                    title: "Col".into(),
                                    width: 300.,
                                }],
                                total_rows: 100,
                                offset: 0,
                                viewport_revision: vp_rev,
                                multi_select: false,
                                rows: vec![TableRow {
                                    id: "r1".into(),
                                    cells: vec!["Row 1".into()],
                                }],
                            }),
                        ),
                    ],
                }),
            )),
        };

    let read_commit = |snap: &PanelSnapshot| -> (bool, u32, u64) {
        let checked = match snap.root.as_ref().unwrap().find(10).unwrap().kind.as_ref().unwrap() {
            node::Kind::Checkbox(c) => c.checked,
            _ => unreachable!(),
        };
        let selected = match snap.root.as_ref().unwrap().find(11).unwrap().kind.as_ref().unwrap() {
            node::Kind::Select(s) => s.selected_index,
            _ => unreachable!(),
        };
        let vp = match snap.root.as_ref().unwrap().find(12).unwrap().kind.as_ref().unwrap() {
            node::Kind::VirtualTable(t) => t.viewport_revision,
            _ => unreachable!(),
        };
        (checked, selected, vp)
    };

    let fixture = fixture(cx, make_snapshot(false, 0, 1));
    cx.update_window(fixture.window, |_, window, cx| {
        // seq 1: toggle checkbox false -> true
        window.click(("plugin-checkbox", 10u64), cx);
        // seq 2: select option B (1)
        window.click(("plugin-select", 11u64), cx);
        window.render_frame(cx);
        let pos = window.find_by_label("B").bounds().center();
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
        // seq 3: page table -> vp 2
        window.render_frame(cx);
        window.click(("plugin-next", 12u64), cx);

        {
            let view = fixture.view.read(cx);
            assert_eq!(view.checkboxes[&10], vec![(true, 1)]);
            assert_eq!(view.selects[&11], vec![(1, 2)]);
            assert_eq!(view.tables[&12].active_viewport_seq(), 3);
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(read_commit(&view.commit_snapshot()), (true, 1, 2));
        }

        // Older same-rev PanelSynced(synced_ui_seq = Some(1), rejected_ui_seqs = &[33], checked=true, selected=0, vp=1):
        // Clears seq<=1 checkbox (now in snapshot), PRESERVES seq=2 select & seq=3 viewport!
        fixture.view.update(cx, |view, cx| {
            push_overlay(&mut view.selects, 999, 0, 33);
            view.replace_with_sync(
                Arc::new(make_snapshot(true, 0, 1)),
                &[],
                Some(1),
                &[33],
                window,
                cx,
            )
            .unwrap();
        });
        {
            let view = fixture.view.read(cx);
            assert!(!view.checkboxes.contains_key(&10));
            assert!(!view.selects.contains_key(&999));
            assert_eq!(view.selects[&11], vec![(1, 2)]);
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(view.tables[&12].active_viewport_seq(), 3);
            assert!(view.tables[&12].requested_offset.is_some());
            assert_eq!(read_commit(&view.commit_snapshot()), (true, 1, 2));
        }

        // seq 4: toggle checkbox true -> false before sync(cutoff=3) arrives.
        window.render_frame(cx);
        window.click(("plugin-checkbox", 10u64), cx);
        assert_eq!(fixture.view.read(cx).checkboxes[&10], vec![(false, 4)]);

        // PanelSynced(synced_ui_seq = Some(3), checked=true, selected=1, vp=1):
        // Retracts vp -> 1 & clears seq<=3 select, while PRESERVING newer seq=4 checkbox (false)!
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(true, 1, 1)),
                &[],
                Some(3),
                &[],
                window,
                cx,
            )
            .unwrap();
            // Stale node error for seq=1 must NOT clobber seq=4 checkbox.
            view.event_failed_with_seq(Some(10), Some(1), "stale seq=1 error", cx);
        });
        {
            let view = fixture.view.read(cx);
            assert_eq!(view.checkboxes[&10], vec![(false, 4)]);
            assert!(!view.selects.contains_key(&11));
            assert_eq!(view.tables[&12].active_viewport_revision(), 1);
            assert_eq!(view.tables[&12].active_viewport_seq(), 0);
            assert!(view.tables[&12].requested_offset.is_none());
            assert_eq!(read_commit(&view.commit_snapshot()), (false, 1, 1));
        }

        // Next click on checkbox must toggle false -> true (NOT repeat false)!
        window.render_frame(cx);
        window.click(("plugin-checkbox", 10u64), cx);
        assert_eq!(
            fixture.view.read(cx).checkboxes[&10],
            vec![(false, 4), (true, 5)]
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn rejecting_newer_click_on_same_control_restores_earlier_queued_override(cx: &mut TestAppContext) {
    use gpui_kit::test::{TestEventExt, TestQueryExt};
    use mio_core::panel::{
        CheckboxNode, Container, SelectNode, TableColumn, TableRow, VirtualTable,
    };

    let make_snapshot = |checked: bool, selected_index: u32, vp_rev: u64| PanelSnapshot {
        revision: 1,
        title: "History Rollback Panel".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    editor(4, 401, b"hello", EditorFormat::Utf8),
                    Node::new(
                        10,
                        "checkbox",
                        node::Kind::Checkbox(CheckboxNode {
                            label: "Enable Flag".into(),
                            checked,
                            enabled: true,
                        }),
                    ),
                    Node::new(
                        11,
                        "select",
                        node::Kind::Select(SelectNode {
                            label: "Choice".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index,
                            enabled: true,
                        }),
                    ),
                    Node::new(
                        12,
                        "table",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 300.,
                            }],
                            total_rows: 100,
                            offset: 0,
                            viewport_revision: vp_rev,
                            multi_select: false,
                            rows: vec![TableRow {
                                id: "r1".into(),
                                cells: vec!["Row 1".into()],
                            }],
                        }),
                    ),
                ],
            }),
        )),
    };

    let read_commit = |snap: &PanelSnapshot| -> (bool, u32, u64) {
        let checked = match snap.root.as_ref().unwrap().find(10).unwrap().kind.as_ref().unwrap() {
            node::Kind::Checkbox(c) => c.checked,
            _ => unreachable!(),
        };
        let selected = match snap.root.as_ref().unwrap().find(11).unwrap().kind.as_ref().unwrap() {
            node::Kind::Select(s) => s.selected_index,
            _ => unreachable!(),
        };
        let vp = match snap.root.as_ref().unwrap().find(12).unwrap().kind.as_ref().unwrap() {
            node::Kind::VirtualTable(t) => t.viewport_revision,
            _ => unreachable!(),
        };
        (checked, selected, vp)
    };

    let fixture = fixture(cx, make_snapshot(false, 0, 1));
    cx.update_window(fixture.window, |_, window, cx| {
        // Checkbox 10: seq 1 (true) followed by seq 2 (false) while seq 1 is still deferred.
        window.click(("plugin-checkbox", 10u64), cx);
        window.render_frame(cx);
        window.click(("plugin-checkbox", 10u64), cx);

        // Select 11: seq 3 (index 1 = "B")
        window.render_frame(cx);
        window.click(("plugin-select", 11u64), cx);
        window.render_frame(cx);
        let pos_b = window.find_by_label("B").bounds().center();
        window.pointer_down(pos_b, MouseButton::Left, cx);
        window.pointer_up(pos_b, MouseButton::Left, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        // Select 11: seq 4 (index 0 = "A")
        window.render_frame(cx);
        window.click(("plugin-select", 11u64), cx);
        window.render_frame(cx);
        let pos_a = window.find_by_label("A").bounds().center();
        window.pointer_down(pos_a, MouseButton::Left, cx);
        window.pointer_up(pos_a, MouseButton::Left, cx);

        // Table 12: seq 5 (vp=2, offset=50) followed by seq 6 (vp=3, offset=90).
        fixture.view.update(cx, |view, cx| {
            view.page_table(12, 50, cx);
            view.page_table(12, 90, cx);
        });
        assert_eq!(
            read_commit(&fixture.view.read(cx).commit_snapshot()),
            (false, 0, 3)
        );

        // Reject seq=2 (checkbox false) and seq=6 (table page 90) via PanelSynced while synced_ui_seq=0:
        // earlier still-deferred seq=1 (true) and seq=5 (vp=2, offset=50) MUST be restored!
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(false, 0, 1)),
                &[],
                Some(0),
                &[2, 6],
                window,
                cx,
            )
            .unwrap();
        });
        {
            let view = fixture.view.read(cx);
            assert_eq!(view.checkboxes[&10], vec![(true, 1)]);
            assert_eq!(latest_overlay(&view.checkboxes, 10), Some(true));
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(view.tables[&12].active_viewport_seq(), 5);
            assert_eq!(view.tables[&12].requested_offset, Some(50));
            assert_eq!(read_commit(&view.commit_snapshot()), (true, 0, 2));
        }

        // Reject seq=4 (select index 0) directly via event_failed_with_seq -> restores earlier seq=3 (index 1)!
        fixture.view.update(cx, |view, cx| {
            view.event_failed_with_seq(Some(11), Some(4), "select queue full", cx);
        });
        {
            let view = fixture.view.read(cx);
            assert_eq!(view.selects[&11], vec![(1, 3)]);
            assert_eq!(latest_overlay(&view.selects, 11), Some(1));
            assert_eq!(read_commit(&view.commit_snapshot()), (true, 1, 2));
        }

        // Reject remaining table seq=5 -> falls back to confirmed_viewport_revision=1 and clears requested_offset.
        fixture.view.update(cx, |view, cx| {
            view.event_failed_with_seq(Some(12), Some(5), "page seq 5 failed", cx);
        });
        {
            let view = fixture.view.read(cx);
            assert_eq!(view.tables[&12].active_viewport_revision(), 1);
            assert_eq!(view.tables[&12].active_viewport_seq(), 0);
            assert!(view.tables[&12].requested_offset.is_none());
            assert_eq!(read_commit(&view.commit_snapshot()), (true, 1, 1));
        }

        // Acknowledge up to seq=6 -> clears remaining local history cleanly.
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(true, 1, 1)),
                &[],
                Some(6),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        let view = fixture.view.read(cx);
        assert!(view.checkboxes.is_empty());
        assert!(view.selects.is_empty());
        assert_eq!(read_commit(&view.commit_snapshot()), (true, 1, 1));
    })
    .unwrap();
}

#[gpui_kit::test]
fn generic_panel_diagnostic_preserves_pending_table_viewports_until_exact_sync(
    cx: &mut TestAppContext,
) {
    use mio_core::panel::{CheckboxNode, Container, TableColumn, TableRow, VirtualTable};

    let make_snapshot = |checked: bool, vp_rev: u64, offset: u64| PanelSnapshot {
        revision: 1,
        title: "Diagnostic Isolation Panel".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        10,
                        "checkbox",
                        node::Kind::Checkbox(CheckboxNode {
                            label: "Flag".into(),
                            checked,
                            enabled: true,
                        }),
                    ),
                    Node::new(
                        12,
                        "table",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 300.,
                            }],
                            total_rows: 100,
                            offset,
                            viewport_revision: vp_rev,
                            multi_select: false,
                            rows: vec![TableRow {
                                id: "r1".into(),
                                cells: vec!["Row 1".into()],
                            }],
                        }),
                    ),
                ],
            }),
        )),
    };

    let read_table_vp = |snap: &PanelSnapshot| -> u64 {
        match snap.root.as_ref().unwrap().find(12).unwrap().kind.as_ref().unwrap() {
            node::Kind::VirtualTable(t) => t.viewport_revision,
            _ => unreachable!(),
        }
    };

    let fixture = fixture(cx, make_snapshot(false, 1, 0));
    cx.update_window(fixture.window, |_, window, cx| {
        // 1. Pending table page (seq 1, vp 2, offset 50) followed by unrelated checkbox click (seq 2).
        fixture.view.update(cx, |view, cx| {
            view.page_table(12, 50, cx);
        });
        window.render_frame(cx);
        window.click(("plugin-checkbox", 10u64), cx);

        // Generic panel diagnostics (both show_diagnostic and event_failed(None, ...)) MUST NOT
        // wipe pending table viewports or requested_offset!
        fixture.view.update(cx, |view, cx| {
            view.show_diagnostic("unrelated checkbox overflow", cx);
            view.event_failed(None, "generic panel diagnostic", cx);
        });
        {
            let view = fixture.view.read(cx);
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(view.tables[&12].active_viewport_seq(), 1);
            assert_eq!(view.tables[&12].requested_offset, Some(50));
            assert_eq!(read_table_vp(&view.commit_snapshot()), 2);
        }

        // Exact PanelSynced rejecting only checkbox seq=2 clears checkbox while STILL preserving table seq=1.
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(false, 1, 0)),
                &[],
                Some(0),
                &[2],
                window,
                cx,
            )
            .unwrap();
        });
        {
            let view = fixture.view.read(cx);
            assert!(view.checkboxes.is_empty());
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(view.tables[&12].active_viewport_seq(), 1);
            assert_eq!(view.tables[&12].requested_offset, Some(50));
            assert_eq!(read_table_vp(&view.commit_snapshot()), 2);
        }

        // Targeted failure on node 12 / seq 1 rolls back table watermark and clears requested_offset.
        fixture.view.update(cx, |view, cx| {
            view.event_failed_with_seq(Some(12), Some(1), "page failed", cx);
        });
        let view = fixture.view.read(cx);
        assert_eq!(view.tables[&12].active_viewport_revision(), 1);
        assert_eq!(view.tables[&12].active_viewport_seq(), 0);
        assert!(view.tables[&12].requested_offset.is_none());
        assert_eq!(read_table_vp(&view.commit_snapshot()), 1);
    })
    .unwrap();
}

#[gpui_kit::test]
fn newer_revision_panel_preserves_unprocessed_local_overrides_unless_semantically_invalidated(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::{TestEventExt, TestQueryExt};
    use mio_core::panel::{CheckboxNode, Container, SelectNode, TableColumn, TableRow, VirtualTable};

    let make_snapshot = |rev: u64,
                         checked: bool,
                         options: Vec<&str>,
                         selected_index: u32,
                         vp_rev: u64,
                         table_offset: u64| PanelSnapshot {
        revision: rev,
        title: "Causal Newer Revision Panel".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        10,
                        "checkbox",
                        node::Kind::Checkbox(CheckboxNode {
                            label: "Enable Flag".into(),
                            checked,
                            enabled: true,
                        }),
                    ),
                    Node::new(
                        11,
                        "select",
                        node::Kind::Select(SelectNode {
                            label: "Choice".into(),
                            options: options.into_iter().map(str::to_owned).collect(),
                            selected_index,
                            enabled: true,
                        }),
                    ),
                    Node::new(
                        12,
                        "table",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 300.,
                            }],
                            total_rows: 100,
                            offset: table_offset,
                            viewport_revision: vp_rev,
                            multi_select: false,
                            rows: vec![TableRow {
                                id: "r1".into(),
                                cells: vec!["Row 1".into()],
                            }],
                        }),
                    ),
                ],
            }),
        )),
    };

    let read_commit = |snap: &PanelSnapshot| -> (bool, u32, u64) {
        let checked = match snap.root.as_ref().unwrap().find(10).unwrap().kind.as_ref().unwrap() {
            node::Kind::Checkbox(c) => c.checked,
            _ => unreachable!(),
        };
        let selected = match snap.root.as_ref().unwrap().find(11).unwrap().kind.as_ref().unwrap() {
            node::Kind::Select(s) => s.selected_index,
            _ => unreachable!(),
        };
        let vp = match snap.root.as_ref().unwrap().find(12).unwrap().kind.as_ref().unwrap() {
            node::Kind::VirtualTable(t) => t.viewport_revision,
            _ => unreachable!(),
        };
        (checked, selected, vp)
    };

    let fixture = fixture(cx, make_snapshot(1, false, vec!["A", "B"], 0, 1, 0));
    cx.update_window(fixture.window, |_, window, cx| {
        // seq 1: checkbox false -> true
        window.click(("plugin-checkbox", 10u64), cx);
        // seq 2: select option B (1)
        window.click(("plugin-select", 11u64), cx);
        window.render_frame(cx);
        let pos_b = window.find_by_label("B").bounds().center();
        window.pointer_down(pos_b, MouseButton::Left, cx);
        window.pointer_up(pos_b, MouseButton::Left, cx);
        // seq 3: page table (vp 2, offset 50)
        window.render_frame(cx);
        fixture.view.update(cx, |view, cx| {
            view.page_table(12, 50, cx);
        });
        {
            let view = fixture.view.read(cx);
            assert_eq!(view.checkboxes[&10], vec![(true, 1)]);
            assert_eq!(view.selects[&11], vec![(1, 2)]);
            assert_eq!(view.tables[&12].active_viewport_seq(), 3);
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(read_commit(&view.commit_snapshot()), (true, 1, 2));
        }

        // Newer-revision Panel(rev=2, synced_ui_seq=Some(1)) with intact Select/Table semantics:
        // clears seq<=1 checkbox (now true in snapshot), PRESERVES unprocessed seq=2 select & seq=3 table page!
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(2, true, vec!["A", "B"], 0, 1, 0)),
                &[],
                Some(1),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        {
            let view = fixture.view.read(cx);
            assert!(!view.checkboxes.contains_key(&10));
            assert_eq!(view.selects[&11], vec![(1, 2)]);
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(view.tables[&12].active_viewport_seq(), 3);
            assert_eq!(view.tables[&12].requested_offset, Some(50));
            assert_eq!(read_commit(&view.commit_snapshot()), (true, 1, 2));
        }

        // Newer-revision Panel(rev=3, synced_ui_seq=Some(1), offset=0, vp_rev=2) advances watermark to 2
        // while keeping requested_offset=Some(50) during Phase-1 loading, and clears invalidated Select seq=2.
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(3, true, vec!["B", "A"], 0, 2, 0)),
                &[],
                Some(1),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        {
            let view = fixture.view.read(cx);
            assert!(!view.selects.contains_key(&11));
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(view.tables[&12].active_viewport_seq(), 0);
            assert_eq!(view.tables[&12].requested_offset, Some(50));
            assert_eq!(read_commit(&view.commit_snapshot()), (true, 0, 2));
        }

        // Phase-2 target page arrival (rev=4, vp_rev=2, offset=50) clears requested_offset to None.
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(4, true, vec!["B", "A"], 0, 2, 50)),
                &[],
                Some(3),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        let view = fixture.view.read(cx);
        assert!(view.tables[&12].requested_offset.is_none());
        assert_eq!(read_commit(&view.commit_snapshot()), (true, 0, 2));
    })
    .unwrap();
}

#[gpui_kit::test]
fn stale_open_select_menu_cannot_dispatch_reordered_option(cx: &mut TestAppContext) {
    use gpui_kit::test::{TestEventExt, TestQueryExt};
    use mio_core::panel::{Container, SelectNode, Text, Tone};

    let make_snapshot = |rev: u64, label: &str, options: Vec<&str>, selected_index: u32| PanelSnapshot {
        revision: rev,
        title: "Stale Open Menu".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        10,
                        "status",
                        node::Kind::Text(Text {
                            text: label.into(),
                            tone: Tone::Foreground as i32,
                        }),
                    ),
                    Node::new(
                        11,
                        "select",
                        node::Kind::Select(SelectNode {
                            label: "Choice".into(),
                            options: options.into_iter().map(str::to_owned).collect(),
                            selected_index,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };

    // Part A: While Select menu is open on ["A", "B"], panel updates to ["B", "A"];
    // clicking cached menu item "A" MUST be rejected without emitting SelectedOption(0).
    let fixture_a = fixture(cx, make_snapshot(1, "v1", vec!["A", "B"], 1));
    cx.update_window(fixture_a.window, |_, window, cx| {
        window.click(("plugin-select", 11u64), cx);
        window.render_frame(cx);
        let pos_a = window.find_by_label("A").bounds().center();

        fixture_a.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(2, "v2", vec!["B", "A"], 0)),
                &[],
                Some(0),
                &[],
                window,
                cx,
            )
            .unwrap();
        });

        window.pointer_down(pos_a, MouseButton::Left, cx);
        window.pointer_up(pos_a, MouseButton::Left, cx);

        let view = fixture_a.view.read(cx);
        assert!(view.selects.is_empty());
        assert!(
            view.error
                .as_deref()
                .is_some_and(|err| err.contains("下拉选项已更新"))
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert!(fixture_a.events.borrow().is_empty());

    // Part B: If only an unrelated Text node updated while Select remained ["A", "B"],
    // clicking open menu item "A" still dispatches SelectedOption(0).
    let fixture_b = fixture(cx, make_snapshot(1, "v1", vec!["A", "B"], 1));
    cx.update_window(fixture_b.window, |_, window, cx| {
        window.click(("plugin-select", 11u64), cx);
        window.render_frame(cx);
        let pos_a = window.find_by_label("A").bounds().center();

        fixture_b.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(2, "v2-text-only", vec!["A", "B"], 1)),
                &[],
                Some(0),
                &[],
                window,
                cx,
            )
            .unwrap();
        });

        window.pointer_down(pos_a, MouseButton::Left, cx);
        window.pointer_up(pos_a, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let selected_events: Vec<_> = fixture_b
        .events
        .borrow()
        .iter()
        .filter_map(|ev| match ev.event {
            Some(ui_event::Event::SelectedOption(idx)) => Some(idx),
            _ => None,
        })
        .collect();
    assert_eq!(selected_events, [0]);
}

#[gpui_kit::test]
fn shrunk_table_total_rows_drops_out_of_range_pending_viewport_and_re_enables_paging(
    cx: &mut TestAppContext,
) {
    use mio_core::panel::{Container, TableColumn, TableRow, VirtualTable};

    let make_snapshot = |rev: u64, vp_rev: u64, total_rows: u64| PanelSnapshot {
        revision: rev,
        title: "Table Shrink Range".into(),
        root: Some(Node::new(
            1,
            "col",
            node::Kind::Column(Container {
                children: vec![Node::new(
                    12,
                    "table",
                    node::Kind::VirtualTable(VirtualTable {
                        columns: vec![TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 300.,
                        }],
                        total_rows,
                        offset: 0,
                        viewport_revision: vp_rev,
                        multi_select: false,
                        rows: vec![TableRow {
                            id: "r1".into(),
                            cells: vec!["Row 1".into()],
                        }],
                    }),
                )],
            }),
        )),
    };

    let fixture = fixture(cx, make_snapshot(1, 1, 100));
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.page_table(12, 50, cx);
        });
        {
            let view = fixture.view.read(cx);
            assert_eq!(view.tables[&12].active_viewport_revision(), 2);
            assert_eq!(view.tables[&12].requested_offset, Some(50));
        }

        // Worker shrinks total_rows to 20 at rev=2 (still vp_rev=1) with synced_ui_seq=0:
        // out-of-range offset=50 MUST be dropped immediately and paging re-enabled.
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(2, 1, 20)),
                &[],
                Some(0),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        {
            let view = fixture.view.read(cx);
            assert_eq!(view.tables[&12].active_viewport_revision(), 1);
            assert_eq!(view.tables[&12].active_viewport_seq(), 0);
            assert!(view.tables[&12].requested_offset.is_none());
        }

        // Direct out-of-range page_table(12, 50) against shrunk total_rows=20 is rejected locally.
        fixture.view.update(cx, |view, cx| {
            view.page_table(12, 50, cx);
        });
        let view = fixture.view.read(cx);
        assert!(view.tables[&12].requested_offset.is_none());
        assert!(
            view.error
                .as_deref()
                .is_some_and(|err| err.contains("表格行数已更新"))
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn async_loading_viewport_keeps_paging_disabled_until_target_page_or_supersede(
    cx: &mut TestAppContext,
) {
    use mio_core::panel::{Container, TableColumn, TableRow, VirtualTable};

    let make_snapshot = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64| {
        let rows: Vec<_> = (0..64usize.min(total_rows.saturating_sub(offset) as usize))
            .map(|idx| TableRow {
                id: format!("r-{}", offset + idx as u64),
                cells: vec![format!("Row {}", offset + idx as u64)],
            })
            .collect();
        PanelSnapshot {
            revision: rev,
            title: "Async Paging Gate".into(),
            root: Some(Node::new(
                1,
                "col",
                node::Kind::Column(Container {
                    children: vec![Node::new(
                        12,
                        "table",
                        node::Kind::VirtualTable(VirtualTable {
                            columns: vec![TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 300.,
                            }],
                            total_rows,
                            offset,
                            viewport_revision: vp_rev,
                            multi_select: false,
                            rows,
                        }),
                    )],
                }),
            )),
        }
    };

    let fixture = fixture(cx, make_snapshot(1, 1, 0, 150));
    cx.update_window(fixture.window, |_, window, cx| {
        // Click "下一页" -> requests offset=64, vp_rev=2.
        window.render_frame(cx);
        window.click(("plugin-next", 12u64), cx);
        assert_eq!(fixture.view.read(cx).tables[&12].requested_offset, Some(64));

        // Phase-1 loading Snapshot(rev=2, vp=2, offset=0, synced_ui_seq=1):
        // requested_offset MUST stay Some(64) and "下一页" stays disabled!
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(2, 2, 0, 150)),
                &[],
                Some(1),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        assert_eq!(fixture.view.read(cx).tables[&12].requested_offset, Some(64));
        window.render_frame(cx);
        window.click(("plugin-next", 12u64), cx);

        // Unrelated same-rev PanelSynced(rev=2, vp=2, offset=0, synced_ui_seq=2) STILL preserves Some(64)!
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(2, 2, 0, 150)),
                &[],
                Some(2),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        assert_eq!(fixture.view.read(cx).tables[&12].requested_offset, Some(64));

        // Phase-2 target page Snapshot(rev=3, vp=2, offset=64) clears requested_offset and re-enables paging!
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(3, 2, 64, 150)),
                &[],
                Some(2),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        assert!(fixture.view.read(cx).tables[&12].requested_offset.is_none());

        // Supersede path: click "下一页" again (offset=128, vp=3) -> Phase-1 loading Snapshot(rev=4, vp=3, offset=64)
        // keeps Some(128); Worker QueryHistory error / Refresh bumps vp=4 -> clears requested_offset to None!
        window.render_frame(cx);
        window.click(("plugin-next", 12u64), cx);
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(4, 3, 64, 150)),
                &[],
                Some(3),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        assert_eq!(fixture.view.read(cx).tables[&12].requested_offset, Some(128));
        // Worker QueryHistory error / Refresh bumps vp=4 -> clears requested_offset to None!
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(5, 4, 64, 150)),
                &[],
                Some(3),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        assert!(fixture.view.read(cx).tables[&12].requested_offset.is_none());

        // Same-revision rejection after Phase-1 loading Snapshot accepted the watermark:
        // click "下一页" again (offset=128, vp=5, seq=3) -> Phase-1 loading Snapshot(rev=6, vp=5, offset=64)
        // keeps requested_offset == Some(128) even when synced_ui_seq catches up without rejection;
        // then same-rev PanelSynced(rev=6, vp=5, offset=64, synced_ui_seq=3, rejected_ui_seqs=&[3]) clears it!
        window.render_frame(cx);
        window.click(("plugin-next", 12u64), cx);
        let failed_seq = fixture.view.read(cx).tables[&12].active_viewport_seq();
        assert_eq!(fixture.view.read(cx).tables[&12].requested_offset, Some(128));
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(6, 5, 64, 150)),
                &[],
                Some(failed_seq),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        assert_eq!(fixture.view.read(cx).tables[&12].requested_offset, Some(128));
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(6, 5, 64, 150)),
                &[],
                Some(failed_seq),
                &[failed_seq],
                window,
                cx,
            )
            .unwrap();
        });
        assert!(fixture.view.read(cx).tables[&12].requested_offset.is_none());

        // Bare Ack path (SDK Ok(None) without publishing any newer-revision loading Snapshot):
        // Host settles confirmed viewport into same-revision PanelSynced(rev=6, vp=6, offset=64) ->
        // UI MUST clear requested_offset immediately and re-enable paging!
        window.render_frame(cx);
        window.click(("plugin-next", 12u64), cx);
        let acked_seq = fixture.view.read(cx).tables[&12].active_viewport_seq();
        assert_eq!(fixture.view.read(cx).tables[&12].requested_offset, Some(128));
        fixture.view.update(cx, |view, cx| {
            view.replace_with_sync(
                Arc::new(make_snapshot(6, 6, 64, 150)),
                &[],
                Some(acked_seq),
                &[],
                window,
                cx,
            )
            .unwrap();
        });
        assert!(fixture.view.read(cx).tables[&12].requested_offset.is_none());
    })
    .unwrap();
    cx.run_until_parked();
    let offsets: Vec<_> = fixture
        .events
        .borrow()
        .iter()
        .filter_map(|ev| match &ev.event {
            Some(ui_event::Event::Viewport(v)) => Some(v.offset),
            _ => None,
        })
        .collect();
    assert_eq!(offsets, [64, 128, 128, 128]);
}
