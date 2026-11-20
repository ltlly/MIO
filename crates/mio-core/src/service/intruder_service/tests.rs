use std::io::ErrorKind;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;
use crate::intruder::{AttackType, Position};
pub(super) use crate::service::tests::completed_flow;

mod backpressure;
mod baseline;
mod dispatch_workspace;
mod faults_recovery;
mod pause;
mod query_notification;
mod resource_pools;
mod retention;
mod session;
mod terminal_discard;
mod title;

fn spec(target: String, payloads: usize) -> IntruderSpec {
    let raw =
        b"POST /intruder HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 1\r\n\r\nX".to_vec();
    let start = raw.len() - 1;
    IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        save_to_project: true,
        revision: 3,
        draft: Draft {
            revision: 7,
            target,
            raw,
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position {
            start,
            end: start + 1,
        }],
        payload_sets: vec![vec![vec![0xff]; payloads]],
        max_concurrent_requests: 2,
        request_timeout_ms: 5000,
        update_host_header: true,
    }
}

fn state() -> (State, mpsc::Receiver<Event>) {
    let (sender, receiver) = mpsc::channel(32);
    (
        State::new(sender, CancellationToken::new()).unwrap(),
        receiver,
    )
}

fn bind(store: &mut ProjectStore, input: &IntruderSpec) -> TabId {
    let id = Uuid::new_v4();
    let mut workspace = store.workspace().unwrap();
    workspace.intruder.tabs.push(crate::IntruderTab {
        id,
        title: "fixture".into(),
        group: None,
        spec: input.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(id);
    workspace.revision += 1;
    store.save_workspace(&workspace).unwrap();
    id
}

fn admit(state: &mut State, input: IntruderSpec) -> Uuid {
    let id = Uuid::new_v4();
    let tab = bind(&mut state.store, &input);
    state
        .admit_intruder(state.project, id, tab, input, CancellationToken::new())
        .unwrap();
    id
}

async fn request(stream: &mut tokio::net::TcpStream) -> u8 {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 8192);
        head.push(stream.read_u8().await.unwrap());
    }
    assert!(
        std::str::from_utf8(&head)
            .unwrap()
            .contains("content-length: 1")
            || std::str::from_utf8(&head)
                .unwrap()
                .contains("Content-Length: 1")
    );
    stream.read_u8().await.unwrap()
}

async fn save(state: &mut State, path: Option<std::path::PathBuf>) -> Result<()> {
    state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path,
            workspace: None,
        })
        .await
}

fn terminal_discard_run(state: &mut State) -> (IntruderRunSummary, FlowRecord) {
    let input = spec("http://127.0.0.1:1".into(), 1);
    let tab = bind(&mut state.store, &input);
    let id = Uuid::new_v4();
    state.store.create_intruder_run(id, tab, &input, 1).unwrap();
    let mut flow = crate::service::tests::completed_flow();
    flow.summary.source = TrafficSource::Intruder;
    state.store.put_intruder_result(id, 0, Ok(&flow)).unwrap();
    let run = state
        .store
        .set_intruder_state(id, IntruderRunState::Completed)
        .unwrap();
    (run, flow)
}

fn discard_workspace(state: &State) -> Workspace {
    let mut workspace = state.store.workspace().unwrap();
    workspace.revision += 1;
    workspace
}

fn discard_reply(
    events: &mut mpsc::Receiver<Event>,
    project: ProjectKey,
    run_id: Uuid,
    operation_id: Uuid,
) -> std::result::Result<IntruderDiscardReceipt, String> {
    while let Ok(event) = events.try_recv() {
        match event {
            Event::IntruderDiscarded {
                project: owner,
                run_id: run,
                operation_id: operation,
                result,
            } => {
                assert_eq!((owner, run, operation), (project, run_id, operation_id));
                return result;
            }
            Event::Error { .. } => panic!("discard errors must retain their operation identity"),
            _ => {}
        }
    }
    panic!("discard must return a correlated result")
}
