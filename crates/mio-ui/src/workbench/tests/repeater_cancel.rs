use super::*;

struct Sent {
    project: ProjectKey,
    tab_id: TabId,
    run_id: FlowId,
    revision: u64,
    cancel: CancellationToken,
}

fn send(fixture: &mut Fixture, cx: &mut TestAppContext) -> Sent {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("tool-repeater", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(acknowledge_history_query(fixture, commands, cx).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("repeater-cancel").is_none());
        let send_bounds = window.find("repeater-send").bounds();
        window.click("repeater-send", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-send").is_none());
        let cancel_bounds = window.find("repeater-cancel").bounds();
        assert_eq!(cancel_bounds.origin, send_bounds.origin);
        assert_eq!(cancel_bounds.size, send_bounds.size);
    })
    .unwrap();
    let command = drain(&mut fixture.commands)
        .into_iter()
        .find(|command| matches!(command, Command::Send { .. }))
        .expect("the production Send button enqueues its saved draft");
    let Command::Send {
        project,
        tab_id,
        run_id,
        draft,
        cancel,
        ..
    } = command
    else {
        unreachable!()
    };
    assert!(!cancel.is_cancelled());
    Sent {
        project,
        tab_id,
        run_id,
        revision: draft.revision,
        cancel,
    }
}

fn started(fixture: &Fixture, sent: &Sent, cx: &mut TestAppContext) {
    fixture
        .events
        .try_send(Event::SendStarted {
            project: sent.project,
            tab_id: sent.tab_id,
            run_id: sent.run_id,
            draft_revision: sent.revision,
        })
        .unwrap();
    cx.run_until_parked();
}

fn finished(fixture: &Fixture, sent: &Sent, state: FlowState, cx: &mut TestAppContext) {
    let mut terminal = record("http://127.0.0.1:9001/", "result");
    terminal.summary.id = sent.run_id;
    terminal.summary.state = state;
    fixture
        .events
        .try_send(Event::SendFinished {
            project: sent.project,
            tab_id: sent.tab_id,
            draft_revision: sent.revision,
            record: Box::new(terminal),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
}

fn acknowledge_workspace(fixture: &mut Fixture, cx: &mut TestAppContext) {
    let workspace = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("terminal flow linkage is saved without sending again");
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        !drain(&mut fixture.commands)
            .iter()
            .any(|command| matches!(command, Command::Send { .. }))
    );
}

fn fill_commands(fixture: &Fixture, cx: &mut TestAppContext) {
    cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .commands
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
            .unwrap()
    });
    assert_eq!(fixture.commands.len(), fixture.commands.max_capacity());
}

fn assert_cancelling(fixture: &Fixture, sent: &Sent, cx: &mut TestAppContext) {
    assert!(sent.cancel.is_cancelled());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(
            view.sending.contains_key(&sent.tab_id),
            "only a matching terminal event releases busy state"
        );
        assert!(window.try_find("repeater-send").is_none());
        let cancel = window.find("repeater-cancel");
        assert!(cancel.visible());
        assert_eq!(cancel.disabled(), None);
        assert!(
            view.status.contains("取消") && view.status.contains("确认"),
            "{}",
            view.status
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn cancel_click_reaches_the_send_token_before_started_even_when_commands_are_full(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("未收到开始回包")], 1);
    drain(&mut fixture.commands);
    let sent = send(&mut fixture, cx);
    fill_commands(&fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-cancel", cx);
        assert!(sent.cancel.is_cancelled(), "Cancel must signal the actual Send token synchronously, without SendStarted or command capacity");
    }).unwrap();
    assert_cancelling(&fixture, &sent, cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery {
                query_revision: u64::MAX,
                ..
            }
        }]
    ));
    started(&fixture, &sent, cx);
    assert_cancelling(&fixture, &sent, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-cancel", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-send").is_none());
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        acknowledge_history_query(&fixture, commands, cx).is_empty(),
        "free capacity cannot retry Send or an already signalled Cancel"
    );
    finished(&fixture, &sent, FlowState::Cancelled, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.sending.is_empty());
        assert_eq!(view.workspace.tabs[0].last_flow, Some(sent.run_id));
    });
    acknowledge_workspace(&mut fixture, cx);
}

#[gpui_kit::test]
fn cancel_click_with_closed_commands_keeps_the_unconfirmed_send_owned(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("队列断开")], 4);
    drain(&mut fixture.commands);
    let sent = send(&mut fixture, cx);
    fixture.commands.close();
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-cancel", cx);
        assert!(
            sent.cancel.is_cancelled(),
            "the closed command queue cannot suppress cancellation"
        );
    })
    .unwrap();
    assert_cancelling(&fixture, &sent, cx);
    started(&fixture, &sent, cx);
    assert_cancelling(&fixture, &sent, cx);
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .unwrap()
                .contains("后台服务")
        )
    });
}

#[gpui_kit::test]
fn project_change_and_backend_disconnect_cancel_tokens_obtained_from_real_send_clicks(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("项目归属")], 8);
    drain(&mut fixture.commands);
    let old = send(&mut fixture, cx);
    let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    fixture.project.generation += 1;
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        old.cancel.is_cancelled(),
        "replacing a project must cancel its owned Send token"
    );
    drain(&mut fixture.commands);
    let current = send(&mut fixture, cx);
    started(&fixture, &old, cx);
    finished(&fixture, &old, FlowState::Cancelled, cx);
    assert!(!current.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.sending.contains_key(&current.tab_id));
        assert_eq!(view.workspace.tabs[0].last_flow, None);
    });
    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    assert!(
        current.cancel.is_cancelled(),
        "a disconnected backend cannot retain an uncancelled send"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected);
        assert!(view.sending.contains_key(&current.tab_id));
        assert_eq!(view.workspace.tabs[0].last_flow, None);
    });
}

#[gpui_kit::test]
fn late_run_and_revision_events_cannot_replace_a_new_send_of_the_same_draft(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("同版本再次发送")], 32);
    drain(&mut fixture.commands);
    let old = send(&mut fixture, cx);
    finished(&fixture, &old, FlowState::Complete, cx);
    acknowledge_workspace(&mut fixture, cx);
    let current = send(&mut fixture, cx);
    assert_eq!(old.revision, current.revision);
    assert_ne!(old.run_id, current.run_id);
    for (project, run_id, revision) in [
        (old.project, old.run_id, old.revision),
        (current.project, current.run_id, current.revision + 1),
        (
            ProjectKey {
                generation: current.project.generation + 1,
                ..current.project
            },
            current.run_id,
            current.revision,
        ),
    ] {
        let stale = Sent {
            project,
            tab_id: current.tab_id,
            run_id,
            revision,
            cancel: current.cancel.clone(),
        };
        started(&fixture, &stale, cx);
        fixture
            .events
            .try_send(Event::SendRejected {
                project,
                tab_id: current.tab_id,
                run_id,
                draft_revision: revision,
                message: "stale rejection must not affect the current request".into(),
            })
            .unwrap();
        finished(&fixture, &stale, FlowState::Cancelled, cx);
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.sending.contains_key(&current.tab_id));
            assert_eq!(view.workspace.tabs[0].last_flow, Some(old.run_id));
            assert!(view.error.is_none());
        });
        assert!(!current.cancel.is_cancelled());
        assert!(drain(&mut fixture.commands).is_empty());
    }
    started(&fixture, &current, cx);
    finished(&fixture, &current, FlowState::Complete, cx);
    acknowledge_workspace(&mut fixture, cx);
    started(&fixture, &old, cx);
    finished(&fixture, &old, FlowState::Cancelled, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.sending.is_empty(),
            "late Started cannot resurrect a completed run"
        );
        assert_eq!(view.workspace.tabs[0].last_flow, Some(current.run_id));
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

fn close_cancels_with_full_commands(bulk: bool, cx: &mut TestAppContext) {
    let first = tab("关闭在途请求");
    let second = tab("保留的标签");
    let mut fixture = fixture(cx, vec![first, second.clone()], 1);
    drain(&mut fixture.commands);
    let sent = send(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        if bulk {
            window.right_click(SharedString::from(format!("work-tab-{}", second.id)), cx);
        } else {
            window.click(SharedString::from(format!("close-tab-{}", sent.tab_id)), cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
    if bulk {
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let close = window.find_by_label("关闭其它标签").bounds().center();
            window.pointer_down(close, MouseButton::Left, cx);
            window.pointer_up(close, MouseButton::Left, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    assert!(cx.has_pending_prompt());
    drain(&mut fixture.commands);
    fill_commands(&fixture, cx);
    cx.simulate_prompt_answer("取消发送并关闭");
    cx.run_until_parked();
    assert!(
        sent.cancel.is_cancelled(),
        "confirmed close must cancel the actual send even without command capacity"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.sending.contains_key(&sent.tab_id));
        assert!(view.close_after_send.contains(&sent.tab_id));
        assert!(view.workspace.tabs.iter().any(|tab| tab.id == sent.tab_id));
    });
    drain(&mut fixture.commands);
    fixture
        .events
        .try_send(Event::SendRejected {
            project: sent.project,
            tab_id: sent.tab_id,
            run_id: sent.run_id,
            draft_revision: sent.revision,
            message: "已取消；尚未建立连接".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.sending.is_empty() && view.close_after_send.is_empty());
        assert_eq!(view.workspace.tabs, vec![second]);
        assert_eq!(view.workspace.closed.len(), 1);
    });
    assert!(
        !drain(&mut fixture.commands)
            .iter()
            .any(|command| matches!(command, Command::Send { .. }))
    );
}

#[gpui_kit::test]
fn close_tab_cancels_queued_send_even_when_command_delivery_is_full(cx: &mut TestAppContext) {
    close_cancels_with_full_commands(false, cx);
}

#[gpui_kit::test]
fn close_other_tabs_menu_cancels_queued_send_even_when_command_delivery_is_full(
    cx: &mut TestAppContext,
) {
    close_cancels_with_full_commands(true, cx);
}
