use std::cell::RefCell;

use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{AnyWindowHandle, TestAppContext, WindowBounds, WindowOptions, point, size};
use mio_core::panel::{
    Button as PanelButton, Container, Editor as PanelEditor, Layout, Split, Tab, Tabs,
};

use super::*;

mod part_1;
mod part_2;
mod part_3;

pub(super) struct Fixture {
    pub(super) window: AnyWindowHandle,
    pub(super) view: Entity<NativePanel>,
    pub(super) events: Rc<RefCell<Vec<UiEvent>>>,
    pub(super) revisions: Rc<RefCell<Vec<u64>>>,
    pub(super) _subscription: Subscription,
}

pub(super) fn editor(id: u64, document: u64, contents: &[u8], format: EditorFormat) -> Node {
    Node::new(
        id,
        format!("编辑器 {document}"),
        node::Kind::Editor(PanelEditor {
            document_id: document,
            revision: 1,
            contents: contents.into(),
            format: format as i32,
            read_only: false,
        }),
    )
}

pub(super) fn model() -> PanelSnapshot {
    PanelSnapshot {
        revision: 1,
        title: "通用面板测试".into(),
        root: Some(Node::new(
            1,
            "面板",
            node::Kind::Column(Container {
                children: vec![
                    Node::new(
                        2,
                        "提交所有编辑",
                        node::Kind::Button(PanelButton {
                            label: "提交所有编辑".into(),
                            enabled: true,
                            commit_editors: true,
                        }),
                    ),
                    Node {
                        layout: Some(Layout {
                            min_height: 160.,
                            flex: 1.,
                            ..Default::default()
                        }),
                        ..Node::new(
                            3,
                            "两个编辑器",
                            node::Kind::Row(Container {
                                children: vec![
                                    editor(4, 401, b"initial", EditorFormat::Utf8),
                                    editor(5, 902, &[0, 255], EditorFormat::Binary),
                                ],
                            }),
                        )
                    },
                ],
            }),
        )),
    }
}

pub(super) fn changed_raw(revision: u64, document_revision: u64, contents: &[u8]) -> PanelSnapshot {
    let mut snapshot = model();
    snapshot.revision = revision;
    let node::Kind::Column(column) = snapshot.root.as_mut().unwrap().kind.as_mut().unwrap() else {
        unreachable!()
    };
    let node::Kind::Row(row) = column.children[1].kind.as_mut().unwrap() else {
        unreachable!()
    };
    let node::Kind::Editor(editor) = row.children[0].kind.as_mut().unwrap() else {
        unreachable!()
    };
    editor.contents = contents.to_vec();
    editor.revision = document_revision;
    snapshot
}

pub(super) fn fixture(cx: &mut TestAppContext, snapshot: PanelSnapshot) -> Fixture {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(900.), px(600.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| NativePanel::new(Arc::new(snapshot), window, cx).unwrap()),
        )
        .unwrap()
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let revisions = Rc::new(RefCell::new(Vec::new()));
    let emitted = events.clone();
    let emitted_revisions = revisions.clone();
    let subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let PanelEvent::Ui { revision, event } = event {
                emitted.borrow_mut().push(event.clone());
                emitted_revisions.borrow_mut().push(*revision);
            }
        })
    });
    // GPUI focus observers intentionally report no focused element for inactive windows.
    // Activate this headless TestPlatform window before dispatching native input.
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    Fixture {
        window,
        view,
        events,
        revisions,
        _subscription: subscription,
    }
}
