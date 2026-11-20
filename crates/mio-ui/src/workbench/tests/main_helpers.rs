use super::*;

pub(crate) struct Fixture {
    pub(crate) window: AnyWindowHandle,
    pub(crate) view: Entity<Workbench>,
    pub(crate) commands: mpsc::Receiver<Command>,
    pub(crate) events: mpsc::Sender<Event>,
    pub(crate) project: ProjectKey,
}

pub(crate) fn tab(name: &str) -> WorkTab {
    WorkTab {
        id: Uuid::new_v4(),
        title: name.into(),
        group: None,
        last_flow: None,
        archive_response: None,
        history: None,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:9001".into(),
            raw: b"GET / HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec(),
            version: HttpVersion::Http11,
        },
    }
}

pub(crate) fn fixture(cx: &mut TestAppContext, tabs: Vec<WorkTab>, capacity: usize) -> Fixture {
    let (commands_tx, mut commands) = mpsc::channel(capacity);
    let (events, events_rx) = mpsc::channel(64);
    let owns_native = cx.update(|cx| {
        let owns_native = cx
            .try_global::<crate::native_terminate::NativeTerminate>()
            .is_none();
        if owns_native {
            cx.set_global(crate::native_terminate::NativeTerminate::for_test());
            gpui_kit::init(cx);
            theme::init(cx);
            crate::init_actions(cx);
        }
        owns_native
    });
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1280.), px(820.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    Workbench::new(
                        Client {
                            commands: commands_tx,
                            events: events_rx,
                            shutdown: CancellationToken::new(),
                        },
                        window,
                        cx,
                    )
                })
            },
        )
        .unwrap()
    });
    if owns_native {
        cx.update(|cx| Workbench::observe_native(&view, window, cx));
    }
    let Command::QueryUserPreferences { operation_id } = commands
        .try_recv()
        .expect("startup reads user preferences once")
    else {
        panic!("the first startup command must query user preferences")
    };
    assert!(
        commands.try_recv().is_err(),
        "preferences initialization must not enqueue unrelated commands"
    );
    events
        .try_send(Event::UserPreferences {
            operation_id,
            state: mio_core::UserPreferencesState {
                preferences: mio_core::UserPreferences::default(),
                load_error: None,
            },
        })
        .unwrap();
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let active = tabs.first().map(|tab| tab.id);
    events
        .try_send(Event::ProjectOpened {
            project,
            path: None,
            workspace: Workspace {
                tabs,
                active,
                ..Workspace::default()
            },
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    Fixture {
        window,
        view,
        commands,
        events,
        project,
    }
}

pub(crate) fn pump_native(cx: &mut TestAppContext) {
    let native = cx.read(|cx| {
        cx.global::<crate::native_terminate::NativeTerminate>()
            .clone()
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        cx.run_until_parked();
        if !native.pump_queued(deadline) {
            break;
        }
    }
}

pub(crate) fn drain(commands: &mut mpsc::Receiver<Command>) -> Vec<Command> {
    std::iter::from_fn(|| commands.try_recv().ok()).collect()
}

pub(crate) fn acknowledge_history_query(
    fixture: &Fixture,
    commands: Vec<Command>,
    cx: &mut TestAppContext,
) -> Vec<Command> {
    let mut remaining = Vec::new();
    for command in commands {
        if let Command::QueryRepeaterHistory {
            project,
            query_revision,
            tab_id,
            start,
            limit,
        } = command
        {
            assert_eq!(project, fixture.project);
            assert_eq!((start, limit), (0, 64));
            cx.read(|cx| {
                let tab = fixture
                    .view
                    .read(cx)
                    .workspace
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .unwrap();
                assert!(
                    tab.history.is_none(),
                    "this fixture has no stored navigation history"
                );
            });
            fixture
                .events
                .try_send(Event::RepeaterHistoryPage {
                    project,
                    query_revision,
                    tab_id,
                    result: Ok(mio_core::RepeaterHistoryPage {
                        total: 0,
                        start,
                        current_position: None,
                        rows: Vec::new(),
                        next_offset: None,
                    }),
                })
                .unwrap();
        } else {
            remaining.push(command);
        }
    }
    cx.run_until_parked();
    remaining
}

pub(crate) fn project_menu(fixture: &Fixture, label: &str, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let menu = cx
            .get_menus()
            .expect("production menus were installed")
            .into_iter()
            .find(|menu| menu.name == "项目")
            .expect("native Project menu");
        assert!(!menu.disabled);
        let action = menu
            .items
            .into_iter()
            .find_map(|item| match item {
                gpui_kit::OwnedMenuItem::Action {
                    name,
                    action,
                    disabled,
                    ..
                } if name == label => {
                    assert!(!disabled, "{label} must be available");
                    Some(action)
                }
                _ => None,
            })
            .expect("requested item is present in the native Project menu");
        assert!(window.is_action_available(action.as_ref(), cx));
        // Headless has no macOS menu surface; dispatch its registered Action directly.
        window.dispatch_action(action, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(crate) fn choose_tab_orientation(
    fixture: &Fixture,
    desired: Orientation,
    cx: &mut TestAppContext,
) {
    let active = cx.read(|cx| fixture.view.read(cx).workspace.active.unwrap());
    cx.update_window(fixture.window, |_, window, cx| {
        window.right_click(SharedString::from(format!("work-tab-{active}")), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let submenu = window.find_by_label("标签视图设置").bounds().center();
        window.pointer_move(submenu, None, cx);
        let label = match desired {
            Orientation::Horizontal => "横向标签",
            Orientation::Vertical => "垂直标签",
        };
        let choice = window.find_by_label(label).bounds().center();
        window.pointer_down(choice, MouseButton::Left, cx);
        window.pointer_up(choice, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace.orientation, desired));
}

pub(crate) fn rendered_status(window: &Window) -> String {
    let footer = window.find("project-status");
    assert!(footer.visible());
    assert_eq!(footer.role(), Some(gpui_kit::accesskit::Role::Status));
    footer
        .label()
        .expect("the rendered footer has a label")
        .into()
}

pub(crate) fn saved_repeater_fixture(cx: &mut TestAppContext) -> (Fixture, Workspace) {
    let mut fixture = fixture(cx, vec![tab("已保存的请求")], 64);
    drain(&mut fixture.commands);
    let stored = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-repeater", cx);
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    let path = PathBuf::from("/tmp/mio-ui-save-status-synthetic.mio");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let commands = acknowledge_history_query(&fixture, commands, cx);
    assert!(matches!(commands.as_slice(),
        [Command::SaveProject { path: Some(destination), workspace: None, .. }] if destination == &path));
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, stored);
        let status = rendered_status(window);
        assert!(status.starts_with("项目已保存") && !status.contains("未保存"));
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    (fixture, stored)
}

pub(crate) fn record(url: &str, body: &str) -> FlowRecord {
    let raw = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nX-Test: a\r\nX-Test: b\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let request = parse_request(raw.as_bytes(), url, HttpVersion::Http11).unwrap();
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: url.into(),
            status: None,
            state: FlowState::Intercepted,
            request_bytes: raw.len() as u64,
            response_bytes: 0,
            started_unix_ms: 0,
            elapsed_ms: 0,
        },
        request,
        response: None,
    }
}

pub(crate) fn websocket_message(
    sequence: u64,
    direction: mio_core::WebSocketDirection,
    kind: mio_core::WebSocketKind,
    payload: &[u8],
) -> mio_core::WebSocketMessage {
    mio_core::WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: Uuid::new_v4(),
        sequence,
        direction,
        kind,
        payload: payload.to_vec(),
        observed_unix_ms: 0,
    }
}
