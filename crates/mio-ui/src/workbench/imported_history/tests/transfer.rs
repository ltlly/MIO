use super::*;

pub(in crate::workbench) fn transfer_command(f: &mut Fixture) -> Command {
    let mut commands = drain(&mut f.commands);
    assert_eq!(commands.len(), 1);
    let command = commands.pop().unwrap();
    assert!(matches!(command, Command::OpenExtRepeater { .. }));
    command
}
pub(in crate::workbench) fn transfer_proposal(
    f: &Fixture,
    command: &Command,
    raw: &[u8],
    cx: &mut TestAppContext,
) -> WorkTab {
    let Command::OpenExtRepeater {
        project,
        operation_id,
        source_id,
        item_id,
        message_id,
        workspace_revision,
    } = command
    else {
        unreachable!()
    };
    assert_eq!(*project, f.project);
    let selected = cx.read(|cx| f.view.read(cx).imported_history_request().unwrap().clone());
    assert_eq!(
        (*source_id, *item_id, *message_id),
        (selected.source_id, selected.item_id, selected.id)
    );
    assert_eq!(
        *workspace_revision,
        cx.read(|cx| f.view.read(cx).workspace.revision)
    );
    let tab = WorkTab {
        id: Uuid::new_v4(),
        title: "归档 Proxy 请求".into(),
        group: None,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:62701".into(),
            raw: raw.to_vec(),
            version: HttpVersion::Http11,
        },
        last_flow: None,
        archive_response: None,
        history: None,
    };
    f.events
        .try_send(Event::ExtRepeaterOpened {
            project: *project,
            operation_id: *operation_id,
            source_id: *source_id,
            item_id: *item_id,
            message_id: *message_id,
            workspace_revision: *workspace_revision,
            result: Ok(tab.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    tab
}

#[gpui_kit::test]
fn imported_proxy_request_transfer_to_repeater_preserves_bytes_and_rejects_late_selection_or_ime(
    cx: &mut TestAppContext,
) {
    let raw = b"GET /proxy229-probe-a HTTP/1.1\r\nX-A: one\r\nX-A: two\r\n\r\n\0\xff";
    for invalid in [None, Some("selection"), Some("ime")] {
        let old = crate::workbench::tests::tab("旧草稿保留");
        let old_id = old.id;
        let mut f = fixture(cx, vec![old.clone()], 64);
        let selected = row(1, raw.len() as u64, None);
        let other = row(0, raw.len() as u64, None);
        let HistoryRow::ExtProxy(archive_row_5) = &selected else {
            unreachable!()
        };
        let ExtHistoryRow {
            request: Some(summary),
            ..
        } = archive_row_5.as_ref()
        else {
            unreachable!()
        };
        page(&mut f, vec![selected.clone(), other.clone()], 2, 0, cx);
        click_row(&f, 0, cx);
        let request = read(&mut f);
        fragment(&f, request, summary.clone(), raw.to_vec(), cx);
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("history-request-message-actions", cx);
                window.render_frame(cx);
                window.click("history-request-to-repeater", cx);
            })
            .unwrap();
        cx.run_until_parked();
        let command = transfer_command(&mut f);
        if invalid == Some("selection") {
            click_row(&f, 1, cx);
            let request = read(&mut f);
            let HistoryRow::ExtProxy(archive_row_6) = &other else {
                unreachable!()
            };
            let ExtHistoryRow {
                request: Some(summary),
                ..
            } = archive_row_6.as_ref()
            else {
                unreachable!()
            };
            fragment(&f, request, summary.clone(), raw.to_vec(), cx);
        } else if invalid == Some("ime") {
            f.window
                .update(cx, |_, window, cx| {
                    let editor = f.view.read(cx).tab_editors[&old_id].request.raw.clone();
                    editor.update(cx, |editor, cx| {
                        editor.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
                    });
                })
                .unwrap();
        }
        let added = if invalid == Some("selection") {
            let Command::OpenExtRepeater {
                project,
                operation_id,
                source_id,
                item_id,
                message_id,
                workspace_revision,
            } = command
            else {
                unreachable!()
            };
            let added = WorkTab {
                id: Uuid::new_v4(),
                title: "旧选择".into(),
                group: None,
                draft: Draft {
                    revision: 1,
                    target: "http://127.0.0.1:62701".into(),
                    raw: raw.to_vec(),
                    version: HttpVersion::Http11,
                },
                last_flow: None,
                archive_response: None,
                history: None,
            };
            f.events
                .try_send(Event::ExtRepeaterOpened {
                    project,
                    operation_id,
                    source_id,
                    item_id,
                    message_id,
                    workspace_revision,
                    result: Ok(added.clone()),
                })
                .unwrap();
            cx.run_until_parked();
            added
        } else {
            transfer_proposal(&f, &command, raw, cx)
        };
        let commands = drain(&mut f.commands);
        if invalid.is_none() {
            let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
                panic!("archive transfer saves one Repeater tab without Send")
            };
            assert_eq!(workspace.tabs, [old.clone(), added.clone()]);
            assert_eq!(workspace.active, Some(added.id));
            assert!(workspace.intruder.tabs.is_empty());
        } else {
            assert!(commands.is_empty());
        }
        f.window
            .update(cx, |_, window, cx| {
                let view = f.view.read(cx);
                assert!(view.sending.is_empty() && view.pending_sends.is_empty());
                if invalid.is_some() {
                    assert_eq!(view.workspace.tabs.as_slice(), std::slice::from_ref(&old));
                }
                if invalid == Some("ime") {
                    let editor = view.tab_editors[&old_id].request.raw.clone();
                    assert!(editor.update(cx, |editor, cx| {
                        editor.marked_text_range(window, cx).is_some()
                    }));
                }
            })
            .unwrap();
    }
}

#[gpui_kit::test]
fn imported_proxy_request_cmd_i_uses_default_pool_admission_without_phantom_repeater_or_start(
    cx: &mut TestAppContext,
) {
    let raw = b"GET /proxy229-probe-a HTTP/1.1\r\nHost: header.invalid\r\n\r\n";
    let mut f = fixture(cx, vec![], 64);
    let selected = row(0, raw.len() as u64, None);
    let HistoryRow::ExtProxy(archive_row_7) = &selected else {
        unreachable!()
    };
    let ExtHistoryRow {
        request: Some(summary),
        ..
    } = archive_row_7.as_ref()
    else {
        unreachable!()
    };
    page(&mut f, vec![selected.clone()], 1, 0, cx);
    click_row(&f, 0, cx);
    let request = read(&mut f);
    fragment(&f, request, summary.clone(), raw.to_vec(), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("i"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let command = transfer_command(&mut f);
    let proposal = transfer_proposal(&f, &command, raw, cx);
    let commands = drain(&mut f.commands);
    let [
        Command::EnsureIntruderDefaultPool {
            project,
            operation_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Intruder transfer must ensure a real default pool before creating its tab")
    };
    assert_eq!(*project, f.project);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.workspace.tabs.is_empty());
        assert!(view.workspace.intruder.tabs.is_empty());
    });
    let pool = Uuid::from_u128(42);
    let catalog = mio_core::IntruderResourcePools {
        revision: 1,
        default_pool_id: Some(pool),
        pools: vec![mio_core::IntruderResourcePool {
            retained_inputs: None,
            id: pool,
            name: "Default resource pool".into(),
            max_concurrent_requests: Some(10),
            delay: None,
            automatic_throttling: mio_core::IntruderAutoThrottle {
                enabled: true,
                ..Default::default()
            },
        }],
    };
    f.events
        .try_send(Event::IntruderDefaultPoolReady {
            project: f.project,
            operation_id: *operation_id,
            result: Ok(catalog),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("admission saves only the requested Intruder draft, never Start or Send")
    };
    assert!(workspace.tabs.is_empty());
    assert_eq!(workspace.intruder.tabs.len(), 1);
    let spec = &workspace.intruder.tabs[0].spec;
    assert_eq!(spec.draft, proposal.draft);
    assert_eq!(
        spec.resource_pool,
        Some(mio_core::IntruderPoolSelection::Existing(pool))
    );
    assert!(spec.positions.is_empty());
}
