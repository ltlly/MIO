use super::*;

async fn default_response_is_unpacked(encoding: &'static str, change_while_in_flight: bool) {
    let started = Instant::now();
    let deadline = started + FIXTURE_BUDGET;
    let compressed = compressed_body(encoding).unwrap();
    let response = response_bytes(Some(encoding), &compressed);
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("compression.mio");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (runtime, mut client) = Runtime::start().unwrap();
    let (reached, received) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (origin, requests) = start_origin(
        listener,
        vec![response.clone(), response],
        change_while_in_flight.then_some((reached, released)),
        deadline,
    );
    let observed = timeout_at(started + EXCHANGE_BUDGET, async {
        let (project, mut workspace) = prepare(&mut client, address).await?;
        let tab = workspace
            .tabs
            .last()
            .context("prepared Repeater tab")?
            .clone();
        save_project(&mut client, project, Some(&source), &source).await?;
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        if change_while_in_flight {
            received
                .await
                .context("origin did not receive the in-flight request")?;
            ensure!(
                requests.load(Ordering::SeqCst) == 1,
                "first Send was retried"
            );
            workspace.repeater_settings = RepeaterSettings {
                unpack_compressed_responses: false,
                ..RepeaterSettings::default()
            };
            save_workspace(&mut client, project, &mut workspace).await?;
            release
                .send(())
                .map_err(|_| anyhow::anyhow!("origin response gate closed"))?;
        }
        let unpacked = finished(&mut client, project, &tab).await?;
        ensure!(
            requests.load(Ordering::SeqCst) == 1,
            "first Send was retried"
        );
        check_unpacked(&unpacked, encoding, &compressed)?;
        if !change_while_in_flight {
            workspace.repeater_settings = RepeaterSettings {
                unpack_compressed_responses: false,
                ..RepeaterSettings::default()
            };
            save_workspace(&mut client, project, &mut workspace).await?;
        }
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let untouched = finished(&mut client, project, &tab).await?;
        ensure!(
            untouched.0.summary.state == FlowState::Complete,
            "disabled unpacking failed"
        );
        check_untouched(&untouched, Some(encoding), &compressed)?;
        ensure!(
            untouched.0.response == unpacked.1.response,
            "disabled unpacking changed the complete received response"
        );
        ensure!(
            unpacked.0.summary.id != untouched.0.summary.id,
            "two Sends shared a flow ID"
        );
        ensure!(
            unpacked.0.request == untouched.0.request,
            "settings changed the sent request"
        );
        ensure!(
            requests.load(Ordering::SeqCst) == 2,
            "origin must receive exactly two explicit Sends"
        );
        workspace
            .tabs
            .last_mut()
            .context("prepared Repeater tab")?
            .last_flow = Some(untouched.0.summary.id);
        save_workspace(&mut client, project, &mut workspace).await?;
        save_and_reopen(
            &mut client,
            project,
            &workspace,
            directory.path(),
            &[unpacked, untouched],
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

    // Both workers are joined before a response regression can panic.
    observed
        .expect("exchange deadline")
        .expect("Runtime compression/settings roundtrip");
    reaped.expect("bounded Runtime/origin cleanup without retry");
    removed.expect("remove synthetic projects after Runtime shutdown");
    assert!(
        Instant::now() <= deadline,
        "fixture and cleanup exceeded eight seconds"
    );
}

#[tokio::test]
async fn default_repeater_unpacks_gzip_in_event_and_stored_flow() {
    default_response_is_unpacked("gzip", false).await;
}

#[tokio::test]
async fn default_repeater_unpacks_deflate_in_event_and_stored_flow() {
    default_response_is_unpacked("deflate", false).await;
}

#[tokio::test]
async fn default_repeater_unpacks_br_with_frozen_settings_and_persistent_original() {
    default_response_is_unpacked("br", true).await;
}

#[tokio::test]
async fn unknown_coding_falls_back_wholly_and_unencoded_response_stays_unmodified() {
    let started = Instant::now();
    let deadline = started + FIXTURE_BUDGET;
    let compressed = compressed_body("gzip").unwrap();
    // Decoding gzip before encountering the unknown inner coding must not expose a partial transform.
    let encoding = "x-mio-unknown, gzip";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (runtime, mut client) = Runtime::start().unwrap();
    let (origin, requests) = start_origin(
        listener,
        vec![
            response_bytes(Some(encoding), &compressed),
            response_bytes(None, BODY),
        ],
        None,
        deadline,
    );
    let observed = timeout_at(started + EXCHANGE_BUDGET, async {
        let (project, workspace) = prepare(&mut client, address).await?;
        let tab = workspace.tabs.last().context("prepared Repeater tab")?;
        send(&mut client, project, tab, &workspace.repeater_settings).await?;
        let failed = finished(&mut client, project, tab).await?;
        ensure!(
            failed.0.summary.state
                == FlowState::Failed("Repeater 响应解压失败；已保留完整收到的原文".into()),
            "unknown Content-Encoding lost its decoding failure reason: {:?}",
            failed.0.summary.state
        );
        check_untouched(&failed, Some(encoding), &compressed)?;
        ensure!(
            requests.load(Ordering::SeqCst) == 1,
            "failed decoding retried the POST"
        );
        send(&mut client, project, tab, &workspace.repeater_settings).await?;
        let unencoded = finished(&mut client, project, tab).await?;
        ensure!(
            unencoded.0.summary.state == FlowState::Complete,
            "unencoded response failed"
        );
        check_untouched(&unencoded, None, BODY)?;
        ensure!(
            failed.0.summary.id != unencoded.0.summary.id,
            "two Sends shared a flow ID"
        );
        ensure!(
            requests.load(Ordering::SeqCst) == 2,
            "no-transform Send was retried"
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
        .expect("Runtime fallback/no-transform roundtrip");
    reaped.expect("bounded Runtime/origin cleanup without retry");
}

#[tokio::test]
async fn transfer_gzip_is_not_mistaken_for_content_gzip() {
    let inner = compressed_body("gzip").unwrap();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&inner).unwrap();
    let outer = encoder.finish().unwrap();
    for fields in [
        "Content-Encoding: gzip\r\nTransfer-Encoding: gzip, chunked\r\n",
        "cOnTeNt-EnCoDiNg: GZip\r\ntRaNsFeR-EnCoDiNg:\tGZip ,\tCHUNKED \t\r\n",
        "Content-Encoding: gzip\r\nTransfer-Encoding: gzip\r\ntransfer-encoding:\tChUnKeD \r\n",
    ] {
        chunked_response_roundtrip(fields, &outer, "", false).await;
    }
}

#[tokio::test]
async fn repeated_chunked_fields_do_not_authorize_content_decoding() {
    let body = compressed_body("gzip").unwrap();
    chunked_response_roundtrip(
        "Content-Encoding: gzip\r\nTransfer-Encoding: chunked\r\nTRANSFER-ENCODING: CHUNKED\r\n",
        &body,
        "",
        false,
    )
    .await;
}

#[tokio::test]
async fn single_chunked_with_http_whitespace_still_unpacks_and_preserves_trailers() {
    let body = compressed_body("gzip").unwrap();
    for (fields, trailers) in [
        (
            "Content-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n",
            "",
        ),
        (
            "cOnTeNt-EnCoDiNg: GZip\r\ntRaNsFeR-EnCoDiNg:\tChUnKeD \t\r\nTrailer: X-End\r\n",
            "X-End: one\r\nX-End: two\r\n",
        ),
    ] {
        chunked_response_roundtrip(fields, &body, trailers, true).await;
    }
}
