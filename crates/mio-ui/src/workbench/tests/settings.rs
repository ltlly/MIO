use super::*;

mod proxy_helpers;
use proxy_helpers::*;
mod part_01;
mod part_02;
mod part_03;
mod part_04;
mod part_05;
mod part_06;
mod part_07;
mod part_08;
mod part_09;
mod part_10;
mod part_11;
mod part_12;
mod part_13;
mod part_14;
mod part_15;
mod part_16;
mod part_17;
mod part_18;
mod part_19;
mod part_20;

pub(crate) const TLS_TEST_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIBZDCCAQqgAwIBAgIUcdDiD87qgL+T5tRIVfhqVmxmYRgwCgYIKoZIzj0EAwIw\n\
GDEWMBQGA1UEAwwNTUlPLXRlc3Qtcm9vdDAeFw0yNjExMTAwOTUxNThaFw0zNjEx\n\
MDcwOTUxNThaMBgxFjAUBgNVBAMMDU1JTy10ZXN0LXJvb3QwWTATBgcqhkjOPQIB\n\
BggqhkjOPQMBBwNCAAQvPIWzBgIMLCFB9Oyt3/5jFGqABMfVwdtYOqshgOx/mXXs\n\
kKfA+GBX3OBxY54FuuYrGCZthVizMR3L+iYkoOytozIwMDAdBgNVHQ4EFgQUE8gO\n\
Illw54Ypn4cZv8g7BdIYgjowDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNI\n\
ADBFAiEA1/sjkWMyXj8y7FOROt5y7Xcki8mTjtTYgRnYK6knE5MCIG+Yw716etRS\n\
3/t/+Gsg08RDORSISdVfgTvZmJpJOyu7\n\
-----END CERTIFICATE-----";

pub(crate) fn tls_test_certificate() -> Vec<u8> {
    mio_core::UpstreamTlsSettings::parse_pem_or_der_cert(TLS_TEST_PEM.as_bytes()).unwrap()
}

pub(crate) fn activate_empty_repeater(fixture: &mut Fixture, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx)
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    // This draft-only fixture has no persisted Repeater history.
    assert!(acknowledge_history_query(fixture, commands, cx).is_empty());
}

pub(crate) fn menu_action(
    owner: AnyWindowHandle,
    menu_name: &str,
    label: &str,
    cx: &mut TestAppContext,
) {
    pump_native(cx);
    cx.update_window(owner, |_, window, cx| {
        window.render_frame(cx);
        let menu = cx
            .get_menus()
            .expect("production menus were installed")
            .into_iter()
            .find(|menu| menu.name == menu_name)
            .expect("installed application menu");
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
                    assert!(!disabled);
                    Some(action)
                }
                _ => None,
            })
            .expect("requested item is present in the installed application menu");
        assert!(window.is_action_available(action.as_ref(), cx));
        // Headless dispatches the installed menu Action directly.
        window.dispatch_action(action, cx);
    })
    .unwrap();
    pump_native(cx);
}

pub(crate) fn open_settings(fixture: &Fixture, cx: &mut TestAppContext) -> AnyWindowHandle {
    menu_action(fixture.window, "MIO", "设置", cx);
    let windows: Vec<_> = cx
        .windows()
        .into_iter()
        .filter(|window| *window != fixture.window)
        .collect();
    assert_eq!(windows.len(), 1, "Settings has one independent window");
    windows[0]
}

pub(crate) fn checkbox_checked(
    settings: AnyWindowHandle,
    id: &'static str,
    expected: bool,
    cx: &mut TestAppContext,
) {
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let checkbox = window.find(id);
        assert_eq!(checkbox.role(), Some(gpui_kit::accesskit::Role::CheckBox));
        assert_eq!(checkbox.checked(), Some(expected));
    })
    .unwrap();
}

pub(crate) fn checked(settings: AnyWindowHandle, expected: bool, cx: &mut TestAppContext) {
    checkbox_checked(settings, "repeater-unpack-responses", expected, cx);
}

pub(crate) fn toggle_checkbox(
    settings: AnyWindowHandle,
    id: &'static str,
    expected: bool,
    cx: &mut TestAppContext,
) {
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
    checkbox_checked(settings, id, expected, cx);
}

pub(crate) fn toggle(settings: AnyWindowHandle, expected: bool, cx: &mut TestAppContext) {
    toggle_checkbox(settings, "repeater-unpack-responses", expected, cx);
}

pub(crate) fn saved(commands: &mut mpsc::Receiver<Command>) -> Workspace {
    drain(commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("checkbox changes use SaveWorkspace")
}

pub(crate) fn acknowledge(fixture: &Fixture, workspace: Workspace, cx: &mut TestAppContext) {
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
}

pub(crate) fn visible_error(settings: AnyWindowHandle, fragment: &str, cx: &mut TestAppContext) {
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let error = window.find("repeater-settings-error");
        assert!(error.visible());
        assert!(error.label().unwrap().contains(fragment));
    })
    .unwrap();
}

pub(crate) fn disconnect_save_picker(cx: &mut TestAppContext) {
    struct PickerDisconnected;

    assert!(cx.did_prompt_for_new_path());
    // GPUI pops the private sender before this callback; unwinding drops it without a reply.
    let disconnected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cx.simulate_new_path_selection(|_| std::panic::resume_unwind(Box::new(PickerDisconnected)));
    }))
    .expect_err("drop the picker sender to exercise the shipped receiver-error callback");
    assert!(disconnected.is::<PickerDisconnected>());
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
}

pub(crate) fn save_as_ack_while_choosing(
    fixture: &mut Fixture,
    settings: AnyWindowHandle,
    old: Workspace,
    cx: &mut TestAppContext,
) {
    let revision = old.revision;
    assert!(!old.repeater_settings.unpack_compressed_responses);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(revision));
        assert!(view.workspace_dirty);
        assert_eq!(view.pending_sends.len(), 1);
        let send = &view.pending_sends[&view.workspace.active.unwrap()];
        assert!(send.settings.unpack_compressed_responses);
        assert_eq!(send.required_workspace_revision, revision + 1);
        assert!(view.sending.is_empty());
    });
    menu_action(settings, "项目", "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.choosing_save_as);
        assert!(view.transition.is_none() && view.after_save.is_none());
    });
    acknowledge(fixture, old, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "the old ACK cannot send or retry the source while Save As is choosing a destination"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.revision, revision);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.choosing_save_as && view.pending_project_save.is_none());
        assert_eq!(view.pending_sends.len(), 1);
        assert!(view.sending.is_empty());
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("repeater-send").is_none());
        let cancel = window.find("repeater-cancel");
        assert!(cancel.visible());
        assert_eq!(cancel.role(), Some(gpui_kit::accesskit::Role::Button));
        assert_eq!(cancel.disabled(), None);
    })
    .unwrap();
}

pub(crate) fn abandoned_settings_send_stays_unsent(
    fixture: &mut Fixture,
    settings: AnyWindowHandle,
    expected: &Draft,
    cx: &mut TestAppContext,
) {
    assert!(!cx.did_prompt_for_new_path());
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "abandoning ordinary Save As must not retry the source, send, or resume a transition"
    );
    checked(settings, true, cx);
    let (path, revision) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.choosing_save_as);
        assert!(view.transition.is_none() && view.after_save.is_none());
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_project_save.is_none());
        assert!(
            view.pending_sends.is_empty(),
            "an abandoned ordinary Save As must explicitly cancel the deferred unsent request"
        );
        assert!(view.sending.is_empty());
        assert_eq!(&view.workspace.tabs[0].draft, expected);
        let editors = &view.tab_editors[&view.workspace.tabs[0].id];
        assert_eq!(editors.request.bytes_lossless(cx), expected.raw);
        assert_eq!(editors.target.read(cx).value().as_ref(), expected.target);
        (view.project_path.clone().unwrap(), view.workspace.revision)
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let send = window.find("repeater-send");
        assert!(send.visible());
        assert_eq!(send.role(), Some(gpui_kit::accesskit::Role::Button));
        assert_eq!(send.disabled(), None);
        assert!(window.try_find("repeater-cancel").is_none());
        window.click("repeater-edit-target", cx);
        window.press("shift-tab", cx);
        assert_eq!(window.find("repeater-send").focused(), Some(true));
        let status = rendered_status(window);
        assert!(status.contains("未保存"));
        assert!(
            status.contains("未发送") || status.contains("尚未建立网络连接"),
            "the visible status must identify the abandoned request as unsent: {status}"
        );
        assert!(!status.contains("保存成功后发送") && !status.contains("等待确认后发送"));
    })
    .unwrap();
    cx.run_until_parked();
    for dialog in cx.read(|cx| cx.windows()) {
        if dialog != fixture.window && dialog != settings {
            let _ = cx.update_window(dialog, |_, window, cx| {
                window.render_frame(cx);
                window.click("repeater-target-input", cx);
                window.press(crate::kbd!("a"), cx);
                window.input("http://127.0.0.1:19999", cx);
                window.click("repeater-target-toggle-scheme", cx);
                window.click("repeater-target-cancel", cx);
            });
        }
    }
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "cancelling the target dialog must not enqueue any workspace save or mutate the draft"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(&view.workspace.tabs[0].draft, expected);
        assert_eq!(
            view.tab_editors[&view.workspace.tabs[0].id]
                .target
                .read(cx)
                .value()
                .as_ref(),
            expected.target
        );
    });
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let status = window.find("repeater-settings-status");
        assert!(status.visible());
        assert!(status.label().unwrap().contains("尚未保存"));
    })
    .unwrap();

    menu_action(settings, "项目", "保存项目", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("a later explicit Save persists settings without reviving an abandoned request");
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(workspace.revision, revision + 1);
    assert!(workspace.repeater_settings.unpack_compressed_responses);
    assert_eq!(&workspace.tabs[0].draft, expected);
    acknowledge(fixture, workspace.clone(), cx);
    let commands = drain(&mut fixture.commands);
    // This abandoned-send fixture never registered a live history item.
    let commands = acknowledge_history_query(fixture, commands, cx);
    assert!(
        matches!(commands.as_slice(),
            [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }]
                if *project == fixture.project),
        "the later ACK must only save the project, never Send or rebind an abandoned plugin commit"
    );
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
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(&view.workspace.tabs[0].draft, expected);
    });
    checked(settings, true, cx);
}

pub(crate) fn acknowledge_project_saved(fixture: &Fixture, path: PathBuf, cx: &mut TestAppContext) {
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
}

pub(crate) fn fill_command_queue(fixture: &Fixture, cx: &mut TestAppContext) -> usize {
    assert_eq!(fixture.commands.len(), 0);
    let capacity = fixture.commands.max_capacity();
    cx.read(|cx| {
        let commands = &fixture.view.read(cx).commands;
        for _ in 0..capacity {
            commands
                .try_send(Command::QueryHttpHistory {
                    query: mio_core::HttpHistoryQuery {
                        project: fixture.project,
                        query_revision: u64::MAX,
                        offset: 0,
                        limit: 1,
                        url_filter: String::new(),
                        filter: Default::default(),
                        sort: Default::default(),
                        selected_ids: Vec::new(),
                    },
                })
                .unwrap();
        }
        assert_eq!(commands.capacity(), 0);
    });
    assert_eq!(fixture.commands.len(), capacity);
    capacity
}
