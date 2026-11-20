use super::*;

#[tokio::test]
async fn cancelled_proxy_joins_and_persists_before_a_blocked_history_reply_is_delivered() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let mut fixture = Fixture::start().await;
        fixture.block_history_reply().await;
        let connection = Connection::open_with_flags(&fixture.path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(fixture.stored(&connection), fixture.initial);
        fixture.cancel.cancel();
        let mut origin_eof = None;
        let mut downstream_eof = None;
        let persisted_without_drain = tokio::time::timeout(OBSERVATION_BUDGET, async {
            origin_eof = Some(fixture.origin.read(&mut [0]).await.unwrap());
            downstream_eof = Some(fixture.downstream.read(&mut [0]).await.unwrap());
            while fixture.stored(&connection).state != FlowState::Cancelled {
                tokio::task::yield_now().await;
            }
        }).await.is_ok();
        let remained_full = fixture.client.events.len() == 128;
        let before_delivery = fixture.stored(&connection);

        // The queued Stop also cleans up the red path before the failed observation is asserted.
        fixture.client.commands.send(Command::StopProxy { project: fixture.project }).await.unwrap();
        for query_revision in 0..128 {
            assert!(matches!(next(&mut fixture.client).await, Event::HistoryPage {
                project, query_revision: revision, total: 1, rows, ..
            } if project == fixture.project && revision == query_revision && rows == [mio_core::HistoryRow::Live(fixture.initial.clone())]));
        }
        assert!(matches!(next(&mut fixture.client).await, Event::HistoryPage {
            project, query_revision: WAITING_QUERY, total: 1, rows, ..
        } if project == fixture.project && rows == [mio_core::HistoryRow::Live(fixture.initial.clone())]),
            "the already admitted History reply must precede notices discovered during its wait");
        assert!(matches!(next(&mut fixture.client).await,
            Event::HistoryChanged { project } if project == fixture.project));
        assert!(matches!(next(&mut fixture.client).await, Event::ProxyState {
            project, address: None, cancel: None, ..
        } if project == fixture.project));
        let final_summary = page(&mut fixture.client, fixture.project, 200).await.pop().unwrap();
        assert_eq!(final_summary.state, FlowState::Cancelled);
        assert!(final_summary.revision > fixture.initial.revision);
        assert_eq!(fixture.stored(&connection), final_summary);
        let final_record = detail(&mut fixture.client, fixture.project, &final_summary).await;
        assert_eq!(final_record.request.body, BODY);
        assert!(final_record.response.is_none());
        assert!(TcpStream::connect(fixture.address).await.is_err());
        drop(connection);
        save(&mut fixture.client, fixture.project, None).await;
        let copy = fixture.directory.path().join("proxy-backpressure-copy.mio");
        save(&mut fixture.client, fixture.project, Some(copy.clone())).await;
        for path in [fixture.path.clone(), copy] {
            fixture.client.commands.send(Command::NewProject { path: None }).await.unwrap();
            opened(&mut fixture.client).await;
            fixture.client.commands.send(Command::OpenProject { path }).await.unwrap();
            let reopened = opened(&mut fixture.client).await;
            assert_eq!(reopened.id, fixture.project.id);
            assert!(reopened.generation > fixture.project.generation);
            assert_eq!(page(&mut fixture.client, reopened, 201).await.as_slice(), std::slice::from_ref(&final_summary));
            assert_eq!(detail(&mut fixture.client, reopened, &final_summary).await, final_record);
        }
        tokio::task::spawn_blocking(move || fixture.runtime.shutdown()).await.unwrap().unwrap();
        no_extra_connection(fixture.listener);
        assert_eq!(origin_eof, Some(0));
        assert_eq!(downstream_eof, Some(0));
        assert!(remained_full, "reclamation must not consume any UI event");
        assert!(persisted_without_drain,
            "Proxy cancellation must join and persist within 3s while History owns the blocked UI reply");
        assert_eq!(before_delivery, final_summary);
    }).await.expect("real Runtime, both sockets, persistence, save/reopen and cleanup fit the 8s fixture budget");
}

#[tokio::test]
async fn closing_a_full_receiver_reclaims_proxy_before_explicit_runtime_shutdown() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let mut fixture = Fixture::start().await;
        fixture.block_history_reply().await;
        let connection =
            Connection::open_with_flags(&fixture.path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        fixture.client.events.close();
        let reclaimed = tokio::time::timeout(OBSERVATION_BUDGET, async {
            assert_eq!(fixture.origin.read(&mut [0]).await.unwrap(), 0);
            assert_eq!(fixture.downstream.read(&mut [0]).await.unwrap(), 0);
            while fixture.stored(&connection).state != FlowState::Cancelled {
                tokio::task::yield_now().await;
            }
            while !fixture.client.commands.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        assert_eq!(fixture.client.events.len(), 128);
        drop(connection);
        tokio::task::spawn_blocking(move || fixture.runtime.shutdown())
            .await
            .unwrap()
            .unwrap();
        no_extra_connection(fixture.listener);
        assert!(fixture.cancel.is_cancelled());
        assert!(
            reclaimed,
            "receiver closure must join and persist without an explicit Runtime shutdown"
        );
    })
    .await
    .expect("closed receiver, both sockets and cleanup fit the 8s fixture budget");
}

#[tokio::test]
async fn idle_service_reclaims_proxy_and_restart_uses_an_independent_token() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let mut fixture = Fixture::start().await;
        fixture.cancel.cancel();
        tokio::time::timeout(OBSERVATION_BUDGET, async {
            assert_eq!(fixture.origin.read(&mut [0]).await.unwrap(), 0);
            assert_eq!(fixture.downstream.read(&mut [0]).await.unwrap(), 0);
            event(&mut fixture.client, |event| {
                matches!(
                    event,
                    Event::ProxyState {
                        address: None,
                        cancel: None,
                        ..
                    }
                )
            })
            .await;
        })
        .await
        .expect("idle service joins without a Stop command or another UI query");
        let summary = page(&mut fixture.client, fixture.project, 300)
            .await
            .pop()
            .unwrap();
        assert_eq!(summary.state, FlowState::Cancelled);
        fixture
            .client
            .commands
            .send(Command::StartProxy {
                project: fixture.project,
                listen: "127.0.0.1:0".parse().unwrap(),
            })
            .await
            .unwrap();
        let Event::ProxyState {
            address: Some(address),
            cancel: Some(current),
            ..
        } = event(&mut fixture.client, |event| {
            matches!(
                event,
                Event::ProxyState {
                    address: Some(_),
                    ..
                }
            )
        })
        .await
        else {
            panic!("the restarted listener owns its cancellation token")
        };
        fixture.cancel.cancel();
        fixture
            .client
            .commands
            .send(Command::SetIntercept {
                project: fixture.project,
                requests: false,
                responses: false,
            })
            .await
            .unwrap();
        let Event::ProxyState {
            address: Some(reported),
            cancel: Some(updated),
            ..
        } = event(&mut fixture.client, |event| {
            matches!(
                event,
                Event::ProxyState {
                    address: Some(_),
                    ..
                }
            )
        })
        .await
        else {
            panic!("interception updates preserve the current listener token")
        };
        assert_eq!(reported, address);
        assert!(!current.is_cancelled());
        assert!(!updated.is_cancelled());
        updated.cancel();
        tokio::time::timeout(OBSERVATION_BUDGET, async {
            current.cancelled().await;
            event(&mut fixture.client, |event| {
                matches!(
                    event,
                    Event::ProxyState {
                        address: None,
                        cancel: None,
                        ..
                    }
                )
            })
            .await;
        })
        .await
        .expect("the updated state cancels only the restarted runner");
        tokio::task::spawn_blocking(move || fixture.runtime.shutdown())
            .await
            .unwrap()
            .unwrap();
        no_extra_connection(fixture.listener);
    })
    .await
    .expect("idle completion, independent restart and cleanup fit the 8s fixture budget");
}

#[tokio::test]
async fn cookie_and_request_body_rules_use_raw_bytes_hot_update_and_cancel_without_sending() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let source = directory.path().join("cookie-raw.mio");
        let copy = directory.path().join("cookie-raw-copy.mio");
        let request_body = compressed_fixture(REQUEST_PLAIN);
        let response_body = compressed_fixture(RESPONSE_PLAIN);
        assert!(!request_body.windows(b"request-visible".len()).any(|part| part == b"request-visible"));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, mut workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        workspace.revision += 1;
        workspace.proxy_messages.unpack_compressed_responses = false;
        assert!(!workspace.proxy_messages.unpack_compressed_requests);
        workspace.scope.include.push(ScopeRule { id: Uuid::new_v4(), enabled: true, prefix: format!("http://{target}/cookies"), include_subdomains: false });
        workspace.proxy_interception.request.enabled = true;
        workspace.proxy_interception.response.enabled = false;
        workspace.proxy_interception.request.rules = vec![
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::Matches, "^request-only$"),
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::Matches, "^tail$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::Matches, "^%41$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::Matches, r#"^"part$"#),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::Matches, "^two$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::Matches, r"^\xff$"),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::Matches, &body_pattern(&request_body)),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::DoesNotMatch, "request-visible"),
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::DoesNotMatch, "^response-only$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::DoesNotMatch, "^A$"),
            cookie_body_rule(ProxyInterceptType::Url, ProxyInterceptRelation::InScope, ""),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        save(&mut client, project, Some(source.clone())).await;
        client.commands.send(Command::SetIntercept { project, requests: true, responses: true }).await.unwrap();
        client.commands.send(Command::StartProxy { project, listen: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), cancel: Some(cancel), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };

        let downstream = send_cookie_gzip(proxy, target, "/cookies/raw-held", &request_body).await;
        let Event::InterceptPending { project: owner, stage, response_head_only, record: pending } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(owner, project);
        assert_eq!(stage, InterceptStage::Request);
        assert!(!response_head_only);
        assert_eq!(pending.request.body, request_body);
        assert_eq!(pending.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("cookie")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), REQUEST_COOKIES);
        assert!(listener_pending(&listener));
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardRequest(pending.request.clone()) }).await.unwrap();
        let mut origin = respond_cookie_gzip(&listener, "/cookies/raw-held", &request_body, &response_body, true).await;
        cookie_body_response(downstream, &response_body, true).await;
        assert_eq!(origin.read(&mut [0]).await.unwrap(), 0);

        for (path, rule_index, condition) in [
            ("/cookies/not-url-decoded", 2, "^A$"),
            ("/cookies/not-request-unpacked", 6, "request-visible"),
        ] {
            workspace.revision += 1;
            workspace.proxy_interception.request.rules[2].condition = "^%41$".into();
            workspace.proxy_interception.request.rules[6].condition = body_pattern(&request_body);
            workspace.proxy_interception.request.rules[rule_index].condition = condition.into();
            client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
            acknowledged(&mut client, project, &workspace).await;
            let downstream = send_cookie_gzip(proxy, target, path, &request_body).await;
            let mut origin = respond_cookie_gzip(&listener, path, &request_body, &response_body, true).await;
            cookie_body_response(downstream, &response_body, true).await;
            assert_eq!(origin.read(&mut [0]).await.unwrap(), 0);
        }

        workspace.revision += 1;
        workspace.proxy_interception.request.rules[6].condition = body_pattern(&request_body);
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let mut cancelled = send_cookie_gzip(proxy, target, "/cookies/cancelled", &request_body).await;
        let Event::InterceptPending { stage, record: cancelled_record, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(stage, InterceptStage::Request);
        assert_eq!(cancelled_record.request.body, request_body);
        assert!(listener_pending(&listener));
        cancel.cancel();
        client.commands.send(Command::StopProxy { project }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProxyState { address: None, cancel: None, .. })).await;
        let mut cancelled_wire = Vec::new();
        cancelled.read_to_end(&mut cancelled_wire).await.unwrap();
        assert!(listener_pending(&listener), "Stop must never send the held compressed POST");
        assert!(TcpStream::connect(proxy).await.is_err());
        assert!(cancel.is_cancelled());

        client.commands.send(Command::QueryHistory { project, query_revision: 800, offset: 0, limit: 10, filter: String::new() }).await.unwrap();
        let Event::HistoryPage { total, rows, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 800, .. })).await else { unreachable!() };
        assert_eq!(total, 4);
        assert_eq!(rows.len(), 4);
        let mut observed = Vec::new();
        for row in rows {
            let mio_core::HistoryRow::Live(summary) = row else { panic!("only loopback Proxy traffic expected") };
            let record = detail(&mut client, project, &summary).await;
            assert_eq!(record.request.body, request_body);
            assert_eq!(record.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("cookie")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), REQUEST_COOKIES);
            if summary.id == cancelled_record.summary.id {
                assert_eq!(summary.state, FlowState::Cancelled);
                assert!(record.response.is_none());
            } else {
                assert_eq!(summary.state, FlowState::Complete);
                let response = record.response.as_ref().unwrap();
                assert_eq!(response.body, response_body);
                assert_eq!(response.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("set-cookie")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), RESPONSE_COOKIES);
            }
            observed.push(record);
        }
        save(&mut client, project, None).await;
        save(&mut client, project, Some(copy.clone())).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        opened(&mut client).await;
        for path in [source, copy] {
            client.commands.send(Command::OpenProject { path }).await.unwrap();
            let Event::ProjectOpened { project: reopened, workspace: restored, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
            assert_eq!(restored, workspace);
            assert!(!restored.proxy_messages.unpack_compressed_requests);
            assert!(!restored.proxy_messages.unpack_compressed_responses);
            assert_eq!(reopened.id, project.id);
            assert!(reopened.generation > project.generation);
            let Event::ProxyState { address, cancel, .. } = next(&mut client).await else { panic!("reopen must report stopped Proxy") };
            assert!(address.is_none() && cancel.is_none());
            for expected in &observed {
                assert_eq!(detail(&mut client, reopened, &expected.summary).await, *expected);
            }
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        no_extra_connection(listener);
    }).await.expect("raw Cookie/body rules, hot updates, no-send Stop and both project copies fit the original eight second budget");
}
