use super::*;

pub(crate) fn check_unpacked(
    observed: &(FlowRecord, OriginalMessages),
    encoding: &str,
    compressed: &[u8],
) -> Result<()> {
    let (record, original) = observed;
    ensure!(
        record.summary.state == FlowState::Complete,
        "send did not complete: {:?}",
        record.summary.state
    );
    ensure!(
        record.summary.revision == 3,
        "successful unpack needs a distinct terminal revision"
    );
    check_response(
        record.response.as_ref().context("unpacked response")?,
        None,
        BODY,
    )?;
    ensure!(
        original.request.is_none(),
        "unpacking must not invent an original request"
    );
    ensure!(
        original.response_body_complete.is_none(),
        "buffered Repeater response is not a streamed original"
    );
    check_response(
        original
            .response
            .as_ref()
            .context("missing encoded Original response")?,
        Some(encoding),
        compressed,
    )
}

pub(crate) fn check_untouched(
    observed: &(FlowRecord, OriginalMessages),
    encoding: Option<&str>,
    body: &[u8],
) -> Result<()> {
    let (record, original) = observed;
    ensure!(
        record.summary.revision == 2,
        "unmodified response gained a transform revision"
    );
    ensure!(
        *original == OriginalMessages::default(),
        "unmodified response gained an Original snapshot"
    );
    check_response(
        record.response.as_ref().context("untouched response")?,
        encoding,
        body,
    )
}

pub(crate) async fn save_project(
    client: &mut Client,
    project: ProjectKey,
    path: Option<&Path>,
    expected: &Path,
) -> Result<()> {
    client
        .commands
        .send(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path: path.map(Path::to_path_buf),
            workspace: None,
        })
        .await?;
    let Event::ProjectSaved {
        path, total_bytes, ..
    } = event(
        client,
        |event| matches!(event, Event::ProjectSaved { project: owner, .. } if *owner == project),
    )
    .await?
    else {
        unreachable!()
    };
    ensure!(path == expected, "SaveProject ACK has the wrong path");
    ensure!(total_bytes > 0, "saved project is empty");
    Ok(())
}

pub(crate) async fn save_and_reopen(
    client: &mut Client,
    mut project: ProjectKey,
    workspace: &Workspace,
    directory: &Path,
    observations: &[(FlowRecord, OriginalMessages)],
) -> Result<()> {
    let source = directory.join("compression.mio");
    let copy = directory.join("compression-copy.mio");
    for (path, expected) in [
        (None, source.as_path()),
        (Some(copy.as_path()), copy.as_path()),
    ] {
        save_project(client, project, path, expected).await?;
        for (record, original) in observations {
            ensure!(
                read_flow(client, project, record).await? == *original,
                "Save/SaveAs changed Original"
            );
        }
    }
    for path in [source, copy] {
        client
            .commands
            .send(Command::OpenProject { path: path.clone() })
            .await?;
        let Event::ProjectOpened {
            project: reopened,
            path: actual,
            workspace: restored,
        } = event(client, |event| matches!(event, Event::ProjectOpened { .. })).await?
        else {
            unreachable!()
        };
        ensure!(actual.as_ref() == Some(&path), "reopened the wrong path");
        ensure!(
            reopened.id == project.id && reopened.generation > project.generation,
            "reopen lost project identity/generation"
        );
        ensure!(
            restored == *workspace,
            "reopen lost settings, draft or last flow"
        );
        project = reopened;
        for (record, original) in observations {
            ensure!(
                read_flow(client, project, record).await? == *original,
                "reopen changed Original"
            );
        }
    }
    Ok(())
}

pub(crate) fn chunked_response_bytes(
    fields: &str,
    body: &[u8],
    trailers: &str,
) -> (Vec<u8>, HttpResponse) {
    let fields = format!(
        "Content-Type: application/octet-stream\r\n{fields}X-Fixture: one\r\nX-Fixture: two\r\n"
    );
    let parse_fields = |fields: &str| {
        fields
            .lines()
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap();
                Header {
                    name: name.into(),
                    value: value.trim_ascii().as_bytes().to_vec(),
                }
            })
            .collect()
    };
    let received = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: parse_fields(&fields),
        body: body.to_vec(),
        trailers: parse_fields(trailers),
    };
    let mut wire = format!("HTTP/1.1 200 OK\r\n{fields}\r\n").into_bytes();
    for chunk in body.chunks(7) {
        wire.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        wire.extend_from_slice(chunk);
        wire.extend_from_slice(b"\r\n");
    }
    wire.extend_from_slice(format!("0\r\n{trailers}\r\n").as_bytes());
    (wire, received)
}

pub(crate) async fn chunked_response_roundtrip(
    fields: &str,
    body: &[u8],
    trailers: &str,
    unpacked: bool,
) {
    let started = Instant::now();
    let deadline = started + FIXTURE_BUDGET;
    let (wire, received) = chunked_response_bytes(fields, body, trailers);
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("compression.mio");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (runtime, mut client) = Runtime::start().unwrap();
    let (origin, requests) = start_origin(listener, vec![wire], None, deadline);
    let observed = timeout_at(started + EXCHANGE_BUDGET, async {
        let (project, mut workspace) = prepare(&mut client, address).await?;
        let tab = workspace.tabs.last().context("prepared Repeater tab")?;
        save_project(&mut client, project, Some(&source), &source).await?;
        send(&mut client, project, tab, &workspace.repeater_settings).await?;
        let observation = finished(&mut client, project, tab).await?;
        let (record, original) = &observation;
        let state = if unpacked {
            FlowState::Complete
        } else {
            FlowState::Failed("Repeater 响应解压失败；已保留完整收到的原文".into())
        };
        ensure!(
            record.summary.state == state,
            "wrong chunked decoding state: {:?}; response: {:?}",
            record.summary.state,
            record.response
        );
        ensure!(
            record.summary.revision == if unpacked { 3 } else { 2 },
            "chunked response gained or lost a transform revision"
        );
        let mut expected = received.clone();
        let expected_original = if unpacked {
            expected.body = BODY.to_vec();
            expected.headers.retain(|header| {
                !header.name.eq_ignore_ascii_case("content-encoding")
                    && !header.name.eq_ignore_ascii_case("content-length")
            });
            let mut provenance = mio_core::FlowProvenance::default();
            provenance.push_response_step(
                &received,
                mio_core::ProvenanceActor::UnpackCompressed,
                &expected,
            );
            OriginalMessages {
                response: Some(received.clone()),
                provenance,
                ..OriginalMessages::default()
            }
        } else {
            OriginalMessages::default()
        };
        ensure!(
            record.response.as_ref() == Some(&expected),
            "chunked response changed body, headers or trailers: {:?}",
            record.response
        );
        ensure!(
            *original == expected_original,
            "chunked response lost its Original or invented a transform"
        );
        ensure!(
            requests.load(Ordering::SeqCst) == 1,
            "decoding must never retry the POST"
        );
        workspace.tabs.last_mut().unwrap().last_flow = Some(record.summary.id);
        save_workspace(&mut client, project, &mut workspace).await?;
        save_and_reopen(
            &mut client,
            project,
            &workspace,
            directory.path(),
            &[observation],
        )
        .await?;
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if !matches!(&observed, Ok(Ok(()))) {
        origin.abort();
    }
    let reaped = reap(runtime, client, origin, &requests, 1, deadline).await;
    let removed = directory.close();
    observed
        .expect("exchange deadline")
        .expect("Runtime chunked response roundtrip");
    reaped.expect("bounded Runtime/origin cleanup without retry");
    removed.expect("remove synthetic projects after Runtime shutdown");
    assert!(Instant::now() <= deadline, "fixture exceeded eight seconds");
}
