use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const ARCHIVE: &[u8] = b"POST /archived HTTP/1.1\r\nHost: archived.invalid\r\nX-A: \xff\r\nX-A:\tsecond\r\nContent-Length: 4\r\n\r\n\0\xff\r\n";

async fn next(client: &mut Client) -> Event {
    loop {
        let event = client.events.recv().await.expect("runtime stays connected");
        match event {
            Event::ProxyState { address: None, .. }
            | Event::HistoryChanged { .. }
            | Event::WebSocketHistoryChanged { .. }
            | Event::SendProgress { .. }
            | Event::StorageState {
                pending_records: 0,
                message: None,
                ..
            } => {}
            Event::UserPreferences { operation_id, .. } if operation_id.is_nil() => {}
            _ => return event,
        }
    }
}

async fn exchange(client: &mut Client, command: Command) -> Event {
    client.commands.send(command).await.unwrap();
    next(client).await
}

#[tokio::test]
async fn archived_item_to_explicit_loopback_send_survives_save_as_and_runtime_reopen() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("archive.mio");
        let copy = directory.path().join("archive-copy.mio");
        let (message, initial, project_id) = {
            let (mut seed, _, message) = fixture_with_raw(ARCHIVE, "/archived");
            let initial = seed.store.workspace().unwrap();
            let project_id = seed.project.id;
            seed.store.save(Some(&original)).unwrap();
            seed.store.close().unwrap();
            (message, initial, project_id)
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap());
        let origin = async {
            let (mut stream, peer) = listener.accept().await.unwrap();
            assert!(peer.ip().is_loopback());
            let mut raw = Vec::new();
            while !raw.ends_with(b"\r\n\r\n") {
                assert!(raw.len() < 8192, "bounded fixture header");
                raw.push(stream.read_u8().await.unwrap());
            }
            let (_, headers, _) = crate::http::head(&raw).unwrap();
            let lengths: Vec<_> = headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("content-length"))
                .map(|header| header.value.as_slice())
                .collect();
            assert_eq!(lengths, [b"4".as_slice()]);
            let mut body = [0; 4];
            stream.read_exact(&mut body).await.unwrap();
            raw.extend_from_slice(&body);
            let request = parse_request(&raw, &target, HttpVersion::Http11).unwrap();
            assert_eq!(request.method, "POST");
            assert_eq!(request.url, format!("{target}/archived"));
            assert_eq!(request.body, b"\0\xff\r\n");
            let repeated: Vec<_> = request
                .headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("x-a"))
                .map(|header| header.value.as_slice())
                .collect();
            assert_eq!(repeated, [b"\xff".as_slice(), b"second".as_slice()]);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .await
                .unwrap();
            listener.into_std().unwrap()
        };
        let workflow = async {
            let (runtime, mut client) = Runtime::start().unwrap();
            assert!(matches!(next(&mut client).await, Event::ProjectOpened { path: None, .. }));
            let Event::ProjectOpened { mut project, workspace: mut saved, .. } = exchange(
                &mut client,
                Command::OpenProject { path: original.clone() },
            ).await else { panic!("open the synthetic disk archive") };
            assert_eq!(project.id, project_id);
            assert_eq!(saved, initial);
            let Event::ExtItems { result, .. } = exchange(
                &mut client,
                Command::QueryExtItems {
                    project,
                    query_revision: 1,
                    source_id: message.source_id,
                    collection: Some(message.collection.clone()),
                    offset: 0,
                    limit: 1,
                },
            ).await else { panic!("query the archive item through Runtime") };
            let page = result.unwrap();
            assert_eq!((page.total, page.next_offset), (1, None));
            assert_eq!(page.rows.len(), 1);
            let item_id = page.rows[0].id;
            assert_eq!(item_id, message.item_id);
            let Event::ExtItem { result, .. } = exchange(
                &mut client,
                Command::ReadExtItem {
                    project,
                    selection_revision: 2,
                    source_id: message.source_id,
                    item_id,
                    message_offset: 0,
                    message_limit: 1,
                },
            ).await else { panic!("read the selected archive item") };
            let detail = result.unwrap();
            assert_eq!(detail.item.id, item_id);
            assert_eq!(detail.messages.rows.as_slice(), std::slice::from_ref(&message));
            let operation_id = Uuid::new_v4();
            let Event::ExtRepeaterOpened { operation_id: actual, result, .. } = exchange(
                &mut client,
                Command::OpenExtRepeater {
                    project,
                    operation_id,
                    source_id: message.source_id,
                    item_id,
                    message_id: detail.messages.rows[0].id,
                    workspace_revision: saved.revision,
                },
            ).await else { panic!("open only the selected archived request") };
            assert_eq!(actual, operation_id);
            let mut tab = result.unwrap();
            assert_eq!(tab.draft.raw, ARCHIVE);
            assert!(tab.draft.target.is_empty(), "the archived Host is not a connection target");
            assert_eq!(tab.draft.revision, 1);
            tab.draft.target = target.clone();
            tab.draft.revision += 1;
            let draft = tab.draft.clone();
            let tab_id = tab.id;
            saved.active = Some(tab_id);
            saved.tabs.push(tab);
            saved.revision += 1;
            assert!(matches!(
                exchange(&mut client, Command::SaveWorkspace { project, workspace: saved.clone() }).await,
                Event::WorkspaceChanged { project: actual, workspace }
                    if actual == project && workspace == saved
            ));
            assert!(matches!(
                exchange(&mut client, Command::Send { run_id: Uuid::new_v4(), cancel: tokio_util::sync::CancellationToken::new(), project, tab_id, draft: draft.clone(), settings: saved.repeater_settings.clone() }).await,
                Event::SendStarted { project: actual, tab_id: actual_tab, draft_revision: 2, .. }
                    if actual == project && actual_tab == tab_id
            ));
            let Event::SendFinished { project: actual, tab_id: actual_tab, draft_revision: 2, record, persisted } = next(&mut client).await
            else { panic!("the explicit loopback request must reach a terminal result") };
            assert_eq!((actual, actual_tab), (project, tab_id));
            assert!(persisted);
            assert_eq!(record.summary.state, FlowState::Complete);
            assert_eq!(record.request.body, b"\0\xff\r\n");
            assert_eq!(record.response.as_ref().unwrap().body, b"OK");
            saved.tabs.last_mut().unwrap().last_flow = Some(record.summary.id);
            saved.revision += 1;
            assert!(matches!(
                exchange(&mut client, Command::SaveWorkspace { project, workspace: saved.clone() }).await,
                Event::WorkspaceChanged { workspace, .. } if workspace == saved
            ));
            for (path, expected) in [(None, &original), (Some(copy.clone()), &copy)] {
                assert!(matches!(
                    exchange(&mut client, Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project, path, workspace: None }).await,
                    Event::ProjectSaved { project: actual, path, total_bytes, .. }
                        if actual == project && &path == expected && total_bytes > 0
                ));
            }
            for path in [original.clone(), copy.clone()] {
                let Event::ProjectOpened { project: reopened, path: actual, workspace } = exchange(
                    &mut client,
                    Command::OpenProject { path: path.clone() },
                ).await else { panic!("reopen both the saved project and its independent copy") };
                assert_eq!(actual, Some(path));
                assert_eq!(reopened.id, project.id);
                assert!(reopened.generation > project.generation);
                assert_eq!(workspace, saved);
                assert_eq!(workspace.tabs.last().unwrap().draft, draft);
                project = reopened;
                let Event::ExtMessage { result, .. } = exchange(
                    &mut client,
                    Command::ReadExtMessage {
                        project,
                        selection_revision: 3,
                        message_id: message.id,
                        offset: 0,
                        limit: ARCHIVE.len(),
                    },
                ).await else { panic!("the original archive remains readable after reopen") };
                let archived = result.unwrap();
                assert_eq!(archived.summary, message);
                assert_eq!(archived.bytes, ARCHIVE);
                let Event::FlowDetail { record: persisted, .. } = exchange(
                    &mut client,
                    Command::ReadFlow { project, selection_revision: 4, flow_id: record.summary.id },
                ).await else { panic!("the saved tab keeps its completed flow") };
                assert_eq!(persisted, record);
            }
            tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
            assert!(matches!(next(&mut client).await, Event::Stopped));
        };
        let (listener, ()) = tokio::join!(origin, workflow);
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "a completed POST must not leave an automatic retry queued at the origin"
        );
    }).await.expect("archive Runtime workflow and joined cleanup must finish within eight seconds");
}
