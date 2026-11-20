use super::*;
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
};
use tokio::{io::AsyncReadExt, net::TcpListener};

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

#[tokio::test]
async fn result_view_waiting_for_ui_capacity_reclaims_cancelled_run_before_delivery() {
    const BOUND: Duration = Duration::from_secs(3);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (sender, mut receiver) = mpsc::channel(4);
    let mut state = State::new(sender, CancellationToken::new()).unwrap();
    let raw =
        b"POST /view HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 1\r\n\r\nX".to_vec();
    let end = raw.len();
    let input = IntruderSpec {
        revision: 1,
        draft: Draft {
            revision: 1,
            target: format!("http://{}", listener.local_addr().unwrap()),
            raw,
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position {
            start: end - 1,
            end,
        }],
        payload_sets: vec![vec![b"A".to_vec(), b"B".to_vec()]],
        max_concurrent_requests: 1,
        request_timeout_ms: 5000,
        update_host_header: true,
        save_to_project: true,
        baseline_request: false,
        resource_pool: None,
    };
    let tab = Uuid::new_v4();
    let mut workspace = state.store.workspace().unwrap();
    workspace.revision += 1;
    workspace.intruder.tabs.push(IntruderTab {
        id: tab,
        title: "backpressure query".into(),
        group: None,
        spec: input.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(tab);
    state.store.save_workspace(&workspace).unwrap();
    let run = Uuid::new_v4();
    let project = state.project;
    let run_cancel = CancellationToken::new();
    state
        .command(Command::StartIntruder {
            project,
            run_id: run,
            tab_id: tab,
            spec: Box::new(input),
            cancel: run_cancel.clone(),
        })
        .await
        .unwrap();
    let mut started = false;
    while let Ok(event) = receiver.try_recv() {
        if let Event::IntruderStarted { run_id, result, .. } = event {
            assert_eq!(run_id, run);
            result.unwrap();
            started = true;
        }
    }
    assert!(started);
    tokio::time::timeout(BOUND, async {
        while state.store.read_intruder_run(run).unwrap().dispatched == 0 {
            let signal = state.intruder.next_signal(&state.events, false).await;
            state.intruder_signal(signal).await.unwrap();
            while receiver.try_recv().is_ok() {}
        }
    })
    .await
    .expect("the single admitted request must be dispatched");

    let (mut stream, _) = tokio::time::timeout(BOUND, listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut head = Vec::new();
    let payload = tokio::time::timeout(BOUND, async {
        while !head.ends_with(b"\r\n\r\n") {
            assert!(head.len() < 8192);
            head.push(stream.read_u8().await.unwrap());
        }
        stream.read_u8().await.unwrap()
    })
    .await
    .unwrap();
    assert_eq!(payload, b'A');
    assert!(state.intruder.has_resources());
    while receiver.try_recv().is_ok() {}
    for _ in 0..state.events.max_capacity() {
        state.events.try_send(Event::Stopped).unwrap();
    }

    let query = query(project, run, 41);
    let query_cancel = CancellationToken::new();
    let source_path = state.store.test_actual_path();
    let monitor = rusqlite::Connection::open_with_flags(
        &source_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut request = Box::pin(state.command(Command::QueryIntruderResultView {
        query: query.clone(),
        cancel: query_cancel.clone(),
    }));
    assert!(poll_once(request.as_mut()).await.is_pending());
    query_cancel.cancel();
    run_cancel.cancel();

    let mut trailing = [0_u8; 1];
    tokio::time::timeout(BOUND, async {
        tokio::select! {
            read = stream.read(&mut trailing) => assert_eq!(read.unwrap(), 0),
            result = request.as_mut() => panic!("query crossed the full event queue before source EOF: {}", result.is_ok()),
        }
    })
    .await
    .expect("out-of-band cancellation must close the actual request connection");

    let before_delivery = tokio::time::timeout(BOUND, async {
        loop {
            assert!(poll_once(request.as_mut()).await.is_pending());
            let state: (i64, u64, u64) = monitor
                .query_row(
                    "SELECT state,dispatched,completed FROM intruder_runs WHERE id=?1",
                    [run.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            if state.0 == 2 {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the query's capacity wait must keep pumping run reclamation");
    assert_eq!(before_delivery, (2, 1, 1));
    assert_eq!(receiver.len(), receiver.max_capacity());

    let mut delivered = Vec::new();
    tokio::time::timeout(BOUND, async {
        loop {
            tokio::select! {
                result = request.as_mut() => break result.unwrap(),
                event = receiver.recv() => delivered.push(event.expect("event channel remains open")),
            }
        }
    })
    .await
    .expect("opening UI capacity must finish the original query command");
    drop(request);
    while let Ok(event) = receiver.try_recv() {
        delivered.push(event);
    }
    state.stop_intruder().await.unwrap();
    state.finish_intruder_shutdown().await.unwrap();

    let mut replies = delivered.into_iter().filter_map(|event| {
        if let Event::IntruderResultView { query, result } = event {
            Some((query, result))
        } else {
            None
        }
    });
    let (actual, result) = replies
        .next()
        .expect("the admitted query returns one reply");
    assert_eq!(actual, query);
    assert!(result.unwrap_err().contains("取消"));
    assert!(replies.next().is_none());
    assert!(!state.intruder.has_resources());
    let saved = state.store.intruder_results(run, None, 10).unwrap();
    assert_eq!(saved.run.state, IntruderRunState::Stopped);
    assert_eq!((saved.run.dispatched, saved.run.completed), (1, 1));
    assert_eq!(saved.rows.len(), 1);
    assert!(
        matches!(&saved.rows[0].outcome, IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Cancelled)
    );
    assert_eq!(
        listener.into_std().unwrap().accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "the second payload must never start while UI delivery is full"
    );
}
