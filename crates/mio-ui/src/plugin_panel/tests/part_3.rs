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
