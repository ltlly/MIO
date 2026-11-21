use super::*;

#[tokio::test]
async fn repeater_send_follows_redirects_according_to_redirect_mode() {
    let started = Instant::now();
    let deadline = started + FIXTURE_BUDGET;
    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address1 = listener1.local_addr().unwrap();
    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address2 = listener2.local_addr().unwrap();
    let (runtime, mut client) = Runtime::start().unwrap();
    let requests1 = Arc::new(AtomicUsize::new(0));
    let received1 = requests1.clone();
    let requests2 = Arc::new(AtomicUsize::new(0));
    let received2 = requests2.clone();

    let origin1 = tokio::spawn(async move {
        timeout_at(deadline, async move {
            let same_site_302 =
                b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 4\r\n\r\nstep".to_vec();
            let final_200 =
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 9\r\n\r\nsame-site"
                    .to_vec();
            let cross_site_302 = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{address2}/cross-final\r\nContent-Length: 5\r\n\r\ncross"
            )
            .into_bytes();
            let expectations: [(&[u8], bool, Vec<u8>); 5] = [
                (
                    b"POST /repeater-compression HTTP/1.1\r\n",
                    true,
                    same_site_302.clone(),
                ),
                (
                    b"POST /repeater-compression HTTP/1.1\r\n",
                    true,
                    same_site_302,
                ),
                (b"GET /final HTTP/1.1\r\n", false, final_200),
                (
                    b"POST /repeater-compression HTTP/1.1\r\n",
                    true,
                    cross_site_302.clone(),
                ),
                (
                    b"POST /repeater-compression HTTP/1.1\r\n",
                    true,
                    cross_site_302,
                ),
            ];
            for (expected_line, has_body, resp) in expectations {
                let (mut stream, peer) = listener1.accept().await?;
                ensure!(peer.ip().is_loopback(), "fixture peer is not loopback");
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    ensure!(head.len() < 4096, "fixture request head exceeded its bound");
                    head.push(stream.read_u8().await?);
                }
                ensure!(
                    head.starts_with(expected_line),
                    "origin1 expected {:?}, got {:?}",
                    String::from_utf8_lossy(expected_line),
                    String::from_utf8_lossy(&head)
                );
                let head_str = std::str::from_utf8(&head)?;
                ensure!(
                    head_str.lines().any(|line| {
                        line.split_once(':').is_some_and(|(n, v)| {
                            n.eq_ignore_ascii_case("authorization") && v.trim() == "Bearer secret"
                        })
                    }),
                    "origin1 must preserve Authorization on same-origin requests: {head_str}"
                );
                if has_body {
                    let mut body = [0; REQUEST_BODY.len()];
                    stream.read_exact(&mut body).await?;
                    ensure!(body.as_slice() == REQUEST_BODY, "origin1 request body changed");
                }
                received1.fetch_add(1, Ordering::SeqCst);
                stream.write_all(&resp).await?;
                ensure!(
                    stream.read(&mut [0]).await? == 0,
                    "origin1 received another request on the same connection"
                );
            }
            Ok(listener1)
        })
        .await
        .context("origin1 deadline")?
    });

    let origin2 = tokio::spawn(async move {
        timeout_at(deadline, async move {
            let (mut stream, peer) = listener2.accept().await?;
            ensure!(peer.ip().is_loopback(), "fixture peer is not loopback");
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                ensure!(head.len() < 4096, "fixture request head exceeded its bound");
                head.push(stream.read_u8().await?);
            }
            let head_str = std::str::from_utf8(&head)?;
            ensure!(
                head.starts_with(b"GET /cross-final HTTP/1.1\r\n"),
                "origin2 received unexpected request line: {head_str}"
            );
            ensure!(
                !head_str.lines().any(|line| {
                    line.split_once(':')
                        .is_some_and(|(n, _)| n.eq_ignore_ascii_case("authorization"))
                }),
                "cross-origin redirect must strip Authorization header: {head_str}"
            );
            received2.fetch_add(1, Ordering::SeqCst);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\r\ncross-site",
                )
                .await?;
            ensure!(
                stream.read(&mut [0]).await? == 0,
                "origin2 received another request on the same connection"
            );
            Ok(listener2)
        })
        .await
        .context("origin2 deadline")?
    });

    let observed = timeout_at(started + EXCHANGE_BUDGET, async {
        let (project, mut workspace) = prepare(&mut client, address1).await?;
        ensure!(
            workspace.repeater_settings.follow_redirects == RepeaterRedirectMode::Never,
            "Repeater must not follow redirects by default"
        );
        let mut auth_raw = format!(
            "POST /repeater-compression HTTP/1.1\r\nHost: {address1}\r\nAuthorization: Bearer secret\r\nContent-Length: {}\r\n\r\n",
            REQUEST_BODY.len()
        )
        .into_bytes();
        auth_raw.extend_from_slice(REQUEST_BODY);
        {
            let tab = workspace.tabs.last_mut().context("prepared Repeater tab")?;
            tab.draft.raw = auth_raw;
            tab.draft.revision += 1;
        }
        save_workspace(&mut client, project, &mut workspace).await?;
        let tab = workspace
            .tabs
            .last()
            .context("prepared Repeater tab")?
            .clone();

        // 1. Never: stops at 302.
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let never_sent = finished(&mut client, project, &tab).await?;
        let never_resp = never_sent.0.response.as_ref().context("never response")?;
        ensure!(
            never_resp.status == 302 && never_resp.body == b"step",
            "Never mode should stop at 302: {never_resp:?}"
        );

        // 2. OnSiteOnly with same-site redirect: follows 302 -> 200 /final.
        workspace.repeater_settings.follow_redirects = RepeaterRedirectMode::OnSiteOnly;
        save_workspace(&mut client, project, &mut workspace).await?;
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let onsite_same = finished(&mut client, project, &tab).await?;
        let onsite_same_resp = onsite_same
            .0
            .response
            .as_ref()
            .context("onsite same response")?;
        ensure!(
            onsite_same_resp.status == 200 && onsite_same_resp.body == b"same-site",
            "OnSiteOnly should follow same-site redirect: {onsite_same_resp:?}"
        );

        // 3. OnSiteOnly with cross-site redirect: stops at 302.
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let onsite_cross = finished(&mut client, project, &tab).await?;
        let onsite_cross_resp = onsite_cross
            .0
            .response
            .as_ref()
            .context("onsite cross response")?;
        ensure!(
            onsite_cross_resp.status == 302 && onsite_cross_resp.body == b"cross",
            "OnSiteOnly should not follow cross-site redirect: {onsite_cross_resp:?}"
        );

        // 4. Always with cross-site redirect: follows to origin2 and strips Authorization.
        workspace.repeater_settings.follow_redirects = RepeaterRedirectMode::Always;
        save_workspace(&mut client, project, &mut workspace).await?;
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let always_cross = finished(&mut client, project, &tab).await?;
        let always_cross_resp = always_cross
            .0
            .response
            .as_ref()
            .context("always cross response")?;
        ensure!(
            always_cross_resp.status == 200 && always_cross_resp.body == b"cross-site",
            "Always should follow cross-site redirect: {always_cross_resp:?}"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if !matches!(&observed, Ok(Ok(()))) {
        origin1.abort();
        origin2.abort();
    }
    let origin2_res = origin2.await;
    let reaped = reap(runtime, client, origin1, &requests1, 5, deadline).await;
    observed
        .expect("exchange deadline")
        .expect("Repeater follow_redirects roundtrip");
    reaped.expect("bounded Runtime/origin1 cleanup without retry");
    let listener2 = origin2_res
        .expect("join origin2")
        .expect("origin2 result")
        .into_std()
        .unwrap();
    assert_eq!(requests2.load(Ordering::SeqCst), 1);
    assert!(
        matches!(listener2.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
        "origin2 must have no queued connections"
    );
}

#[tokio::test]
async fn repeater_send_uses_workspace_upstream_proxy_and_request_timeout() {
    let started = Instant::now();
    let deadline = started + FIXTURE_BUDGET;
    let origin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_address = origin_listener.local_addr().unwrap();
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = proxy_listener.local_addr().unwrap();
    let dead_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = dead_listener.local_addr().unwrap().port();
    drop(dead_listener);

    let (runtime, mut client) = Runtime::start().unwrap();
    let origin_requests = Arc::new(AtomicUsize::new(0));
    let origin_received = origin_requests.clone();
    let proxy_requests = Arc::new(AtomicUsize::new(0));
    let proxy_received = proxy_requests.clone();
    let (hold_tx, hold_rx) = oneshot::channel::<()>();

    let proxy_task = tokio::spawn(async move {
        timeout_at(deadline, async move {
            let (mut stream, peer) = proxy_listener.accept().await?;
            ensure!(peer.ip().is_loopback(), "proxy peer is not loopback");
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                ensure!(head.len() < 4096, "proxy request head exceeded its bound");
                head.push(stream.read_u8().await?);
            }
            let head_str = std::str::from_utf8(&head)?;
            let expected_start =
                format!("POST http://{origin_address}/repeater-compression HTTP/1.1\r\n");
            ensure!(
                head_str.starts_with(&expected_start),
                "proxy expected absolute-URI request line {expected_start:?}, got {head_str:?}"
            );
            let mut body = [0; REQUEST_BODY.len()];
            stream.read_exact(&mut body).await?;
            ensure!(
                body.as_slice() == REQUEST_BODY,
                "proxy request body changed"
            );
            proxy_received.fetch_add(1, Ordering::SeqCst);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 11\r\nConnection: close\r\n\r\nvia-up-http",
                )
                .await?;
            ensure!(
                stream.read(&mut [0]).await? == 0,
                "proxy received another request on the same connection"
            );
            Ok(proxy_listener)
        })
        .await
        .context("proxy deadline")?
    });

    let origin_task = tokio::spawn(async move {
        timeout_at(deadline, async move {
            let (mut stream, peer) = origin_listener.accept().await?;
            ensure!(peer.ip().is_loopback(), "origin peer is not loopback");
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                ensure!(head.len() < 4096, "origin request head exceeded its bound");
                head.push(stream.read_u8().await?);
            }
            let mut body = [0; REQUEST_BODY.len()];
            stream.read_exact(&mut body).await?;
            origin_received.fetch_add(1, Ordering::SeqCst);
            let _ = hold_rx.await;
            Ok(origin_listener)
        })
        .await
        .context("origin deadline")?
    });

    let observed = timeout_at(started + Duration::from_secs(5), async {
        let (project, mut workspace) = prepare(&mut client, origin_address).await?;
        ensure!(
            workspace.repeater_settings.request_timeout_secs == 30,
            "Repeater default request_timeout_secs must be 30"
        );
        let tab = workspace
            .tabs
            .last()
            .context("prepared Repeater tab")?
            .clone();

        // 1. Send via reachable loopback HTTP upstream proxy.
        workspace.upstream_proxy = UpstreamProxySettings {
            rules: vec![mio_core::UpstreamProxyRule {
                protocol: UpstreamProxyProtocol::Http,
                host: proxy_address.ip().to_string(),
                port: proxy_address.port(),
                ..mio_core::UpstreamProxyRule::default()
            }],
            ..UpstreamProxySettings::default()
        };
        save_workspace(&mut client, project, &mut workspace).await?;
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let proxied = finished(&mut client, project, &tab).await?;
        ensure!(
            proxied.0.summary.state == FlowState::Complete,
            "proxied Send failed: {:?}",
            proxied.0.summary.state
        );
        let proxied_resp = proxied.0.response.as_ref().context("proxied response")?;
        ensure!(
            proxied_resp.status == 200 && proxied_resp.body == b"via-up-http",
            "unexpected proxied response: {proxied_resp:?}"
        );
        ensure!(
            proxy_requests.load(Ordering::SeqCst) == 1
                && origin_requests.load(Ordering::SeqCst) == 0,
            "proxied Send must route through upstream proxy without touching origin directly"
        );

        // 2. Unreachable upstream proxy fails explicitly without falling back to direct origin.
        workspace.upstream_proxy = UpstreamProxySettings {
            rules: vec![mio_core::UpstreamProxyRule {
                protocol: UpstreamProxyProtocol::Http,
                host: "127.0.0.1".into(),
                port: dead_port,
                ..mio_core::UpstreamProxyRule::default()
            }],
            ..UpstreamProxySettings::default()
        };
        save_workspace(&mut client, project, &mut workspace).await?;
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let Event::SendFinished {
            record: dead_record,
            ..
        } = event(&mut client, |event| {
            matches!(event, Event::SendFinished { project: owner, tab_id, .. }
                if *owner == project && *tab_id == tab.id)
        })
        .await?
        else {
            unreachable!()
        };
        ensure!(
            matches!(dead_record.summary.state, FlowState::Failed(_))
                && dead_record.response.is_none(),
            "unreachable upstream proxy must fail explicitly, got {:?}",
            dead_record.summary.state
        );
        ensure!(
            origin_requests.load(Ordering::SeqCst) == 0,
            "unreachable upstream proxy must never silently fall back to direct origin"
        );

        // 3. Direct route with request_timeout_secs = 1 times out in ~1s against silent origin.
        workspace.upstream_proxy = UpstreamProxySettings::default();
        workspace.repeater_settings.request_timeout_secs = 1;
        save_workspace(&mut client, project, &mut workspace).await?;
        let timeout_started = Instant::now();
        send(&mut client, project, &tab, &workspace.repeater_settings).await?;
        let Event::SendFinished {
            record: timed_out, ..
        } = event(&mut client, |event| {
            matches!(event, Event::SendFinished { project: owner, tab_id, .. }
                if *owner == project && *tab_id == tab.id)
        })
        .await?
        else {
            unreachable!()
        };
        let elapsed = timeout_started.elapsed();
        let _ = hold_tx.send(());
        ensure!(
            matches!(&timed_out.summary.state, FlowState::Failed(msg) if msg.contains("超时") || msg.contains("超过期限")),
            "expected timeout failure state, got {:?}",
            timed_out.summary.state
        );
        ensure!(
            elapsed >= Duration::from_millis(800) && elapsed < Duration::from_millis(2500),
            "expected ~1s timeout, got {elapsed:?}"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if !matches!(&observed, Ok(Ok(()))) {
        proxy_task.abort();
        origin_task.abort();
    }
    let proxy_res = proxy_task.await;
    let reaped = reap(runtime, client, origin_task, &origin_requests, 1, deadline).await;
    observed
        .expect("exchange deadline")
        .expect("Repeater upstream proxy and request timeout roundtrip");
    reaped.expect("bounded Runtime/origin cleanup");
    let proxy_listener = proxy_res
        .expect("join proxy")
        .expect("proxy result")
        .into_std()
        .unwrap();
    assert_eq!(proxy_requests.load(Ordering::SeqCst), 1);
    assert!(
        matches!(proxy_listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
        "proxy must have no queued connections"
    );
}
