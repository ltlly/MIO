use super::*;

#[tokio::test]
async fn repeater_send_respects_frozen_update_content_length_and_preserves_repeater_history() {
    let started = Instant::now();
    let deadline = started + FIXTURE_BUDGET;
    let response = response_bytes(None, b"ok");
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("compression.mio");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (runtime, mut client) = Runtime::start().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let received_requests = requests.clone();
    let response_for_origin = response.clone();
    let origin = tokio::spawn(async move {
        timeout_at(deadline, async move {
            for expected_cl in ["99", &REQUEST_BODY.len().to_string()] {
                let (mut stream, peer) = listener.accept().await?;
                ensure!(peer.ip().is_loopback(), "fixture peer is not loopback");
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    ensure!(head.len() < 4096, "fixture request head exceeded its bound");
                    head.push(stream.read_u8().await?);
                }
                let head_str = std::str::from_utf8(&head)?;
                ensure!(
                    head_str.starts_with("POST /repeater-compression HTTP/1.1\r\n"),
                    "origin received the wrong request line"
                );
                let cl = head_str
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .context("origin request lacks Content-Length")?
                    .1
                    .trim();
                ensure!(
                    cl == expected_cl,
                    "origin expected Content-Length {expected_cl}, got {cl}"
                );
                let mut body = [0; REQUEST_BODY.len()];
                stream.read_exact(&mut body).await?;
                ensure!(
                    body.as_slice() == REQUEST_BODY,
                    "origin request bytes changed"
                );
                received_requests.fetch_add(1, Ordering::SeqCst);
                stream.write_all(&response_for_origin).await?;
                ensure!(
                    stream.read(&mut [0]).await? == 0,
                    "origin received another request on the same connection"
                );
            }
            Ok(listener)
        })
        .await
        .context("origin deadline")?
    });

    let observed = timeout_at(started + EXCHANGE_BUDGET, async {
        let (project, mut workspace) = prepare(&mut client, address).await?;
        ensure!(
            workspace.repeater_settings.update_content_length,
            "Repeater must update Content-Length by default"
        );
        let mut manual_raw = format!(
            "POST /repeater-compression HTTP/1.1\r\nHost: {address}\r\nContent-Length: 99\r\n\r\n"
        )
        .into_bytes();
        manual_raw.extend_from_slice(REQUEST_BODY);
        {
            let tab = workspace.tabs.last_mut().context("prepared Repeater tab")?;
            tab.draft.raw = manual_raw.clone();
            tab.draft.revision += 1;
        }
        workspace.repeater_settings.update_content_length = false;
        save_workspace(&mut client, project, &mut workspace).await?;
        save_project(&mut client, project, Some(&source), &source).await?;

        let tab = workspace.tabs.last().context("prepared Repeater tab")?.clone();
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let manual_sent = finished(&mut client, project, &tab).await?;
        ensure!(
            manual_sent.0.summary.state == FlowState::Complete,
            "manual Content-Length send failed"
        );
        let manual_cl: Vec<_> = manual_sent
            .0
            .request
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("content-length"))
            .map(|h| h.value.as_slice())
            .collect();
        ensure!(
            manual_cl == [b"99".as_slice()],
            "stored request lost manual Content-Length: {manual_cl:?}"
        );

        workspace.repeater_settings.update_content_length = true;
        save_workspace(&mut client, project, &mut workspace).await?;
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let updated_sent = finished(&mut client, project, &tab).await?;
        ensure!(
            updated_sent.0.summary.state == FlowState::Complete,
            "updated Content-Length send failed"
        );
        let expected_cl = REQUEST_BODY.len().to_string();
        let updated_cl: Vec<_> = updated_sent
            .0
            .request
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("content-length"))
            .map(|h| h.value.as_slice())
            .collect();
        ensure!(
            updated_cl == [expected_cl.as_bytes()],
            "stored request did not normalize Content-Length: {updated_cl:?}"
        );

        client
            .commands
            .send(Command::QueryRepeaterHistory {
                project,
                query_revision: 11,
                tab_id: tab.id,
                start: 0,
                limit: 64,
            })
            .await?;
        let Event::RepeaterHistoryPage {
            result: Ok(page), ..
        } = event(&mut client, |event| {
            matches!(event, Event::RepeaterHistoryPage { project: owner, query_revision: 11, .. } if *owner == project)
        })
        .await?
        else {
            unreachable!()
        };
        ensure!(page.total == 2, "expected two Repeater history entries");
        for row in &page.rows {
            let operation_id = mio_core::Uuid::new_v4();
            client
                .commands
                .send(Command::SelectRepeaterHistory {
                    project,
                    operation_id,
                    tab_id: tab.id,
                    entry: row.id,
                    workspace_revision: workspace.revision,
                    draft_revision: tab.draft.revision,
                    cancel: tokio_util::sync::CancellationToken::new(),
                })
                .await?;
            let Event::RepeaterHistorySelected {
                result: Ok(selected),
                ..
            } = event(&mut client, |event| {
                matches!(event, Event::RepeaterHistorySelected { project: owner, operation_id: op, .. } if *owner == project && *op == operation_id)
            })
            .await?
            else {
                unreachable!()
            };
            ensure!(
                selected.draft.raw == manual_raw,
                "repeater_history did not preserve original draft.raw"
            );
        }

        workspace
            .tabs
            .last_mut()
            .context("prepared Repeater tab")?
            .last_flow = Some(updated_sent.0.summary.id);
        save_workspace(&mut client, project, &mut workspace).await?;
        save_and_reopen(
            &mut client,
            project,
            &workspace,
            directory.path(),
            &[manual_sent, updated_sent],
        )
        .await?;
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if !matches!(&observed, Ok(Ok(()))) {
        origin.abort();
    }
    let reaped = reap(runtime, client, origin, &requests, 2, deadline).await;
    let removed = directory.close();
    observed
        .expect("exchange deadline")
        .expect("Repeater update_content_length roundtrip");
    reaped.expect("bounded Runtime/origin cleanup without retry");
    removed.expect("remove synthetic projects after Runtime shutdown");
}

#[tokio::test]
async fn corrupt_inner_checksum_keeps_repeated_codings_outer_bytes_and_trailers() {
    let mut inner = compressed_body("gzip").unwrap();
    let checksum = inner.len() - 8;
    inner[checksum] ^= 1;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&inner).unwrap();
    let outer = encoder.finish().unwrap();
    chunked_response_roundtrip(
        "Content-Encoding: gzip\r\ncOnTeNt-EnCoDiNg: GZip\r\nTransfer-Encoding: chunked\r\nTrailer: X-End\r\n",
        &outer,
        "X-End: one\r\nX-End: two\r\n",
        false,
    )
    .await;
}

#[tokio::test]
async fn repeater_send_respects_normalize_http1_line_endings_setting() {
    let started = Instant::now();
    let deadline = started + FIXTURE_BUDGET;
    let response = response_bytes(None, b"ok");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (runtime, mut client) = Runtime::start().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let received_requests = requests.clone();
    let response_for_origin = response.clone();
    let origin = tokio::spawn(async move {
        timeout_at(deadline, async move {
            for expect_crlf in [false, true] {
                let (mut stream, peer) = listener.accept().await?;
                ensure!(peer.ip().is_loopback(), "fixture peer is not loopback");
                let mut head = Vec::new();
                while !(head.ends_with(b"\r\n\r\n") || head.ends_with(b"\n\n")) {
                    ensure!(head.len() < 4096, "fixture request head exceeded its bound");
                    head.push(stream.read_u8().await?);
                }
                let mut body = [0; REQUEST_BODY.len()];
                stream.read_exact(&mut body).await?;
                ensure!(
                    body.as_slice() == REQUEST_BODY,
                    "origin request bytes changed"
                );
                if expect_crlf {
                    ensure!(
                        head.starts_with(b"POST /repeater-compression HTTP/1.1\r\n")
                            && head.ends_with(b"\r\n\r\n")
                            && !head.windows(2).any(|w| w[0] != b'\r' && w[1] == b'\n'),
                        "expected normalized CRLF line endings, got: {:?}",
                        String::from_utf8_lossy(&head)
                    );
                } else {
                    ensure!(
                        head.starts_with(b"POST /repeater-compression HTTP/1.1\n")
                            && head.ends_with(b"\n\n")
                            && !head.contains(&b'\r'),
                        "expected unnormalized LF line endings, got: {:?}",
                        String::from_utf8_lossy(&head)
                    );
                }
                received_requests.fetch_add(1, Ordering::SeqCst);
                stream.write_all(&response_for_origin).await?;
                ensure!(
                    stream.read(&mut [0]).await? == 0,
                    "origin received another request on the same connection"
                );
            }
            Ok(listener)
        })
        .await
        .context("origin deadline")?
    });

    let observed = timeout_at(started + EXCHANGE_BUDGET, async {
        let (project, mut workspace) = prepare(&mut client, address).await?;
        ensure!(
            workspace.repeater_settings.normalize_http1_line_endings,
            "Repeater must normalize HTTP/1 line endings by default"
        );
        let mut lf_raw = format!(
            "POST /repeater-compression HTTP/1.1\nHost: {address}\nContent-Length: {}\n\n",
            REQUEST_BODY.len()
        )
        .into_bytes();
        lf_raw.extend_from_slice(REQUEST_BODY);
        {
            let tab = workspace.tabs.last_mut().context("prepared Repeater tab")?;
            tab.draft.raw = lf_raw;
            tab.draft.revision += 1;
        }
        workspace.repeater_settings.normalize_http1_line_endings = false;
        save_workspace(&mut client, project, &mut workspace).await?;

        let tab = workspace
            .tabs
            .last()
            .context("prepared Repeater tab")?
            .clone();
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let first_sent = finished(&mut client, project, &tab).await?;
        ensure!(
            first_sent.0.summary.state == FlowState::Complete,
            "unnormalized LF send failed"
        );

        workspace.repeater_settings.normalize_http1_line_endings = true;
        save_workspace(&mut client, project, &mut workspace).await?;
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let second_sent = finished(&mut client, project, &tab).await?;
        ensure!(
            second_sent.0.summary.state == FlowState::Complete,
            "normalized CRLF send failed"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if !matches!(&observed, Ok(Ok(()))) {
        origin.abort();
    }
    let reaped = reap(runtime, client, origin, &requests, 2, deadline).await;
    observed
        .expect("exchange deadline")
        .expect("Repeater normalize_http1_line_endings roundtrip");
    reaped.expect("bounded Runtime/origin cleanup without retry");
}
