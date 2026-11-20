use super::*;

#[tokio::test]
async fn response_cookie_body_relations_preserve_gzip_and_do_not_negate_decoder_errors() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let source = directory.path().join("response-cookie.mio");
        let copy = directory.path().join("response-cookie-copy.mio");
        let request_body = compressed_fixture(REQUEST_PLAIN);
        let response_body = compressed_fixture(RESPONSE_PLAIN);
        assert!(!response_body.windows(b"response-visible".len()).any(|part| part == b"response-visible"));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, mut workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        assert!(!workspace.proxy_messages.unpack_compressed_requests);
        assert!(workspace.proxy_messages.unpack_compressed_responses);
        workspace.revision += 1;
        workspace.proxy_interception.request.enabled = false;
        workspace.proxy_interception.response.enabled = true;
        workspace.proxy_interception.response.rules = vec![
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::RequestMatches, "^request-only$"),
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::RequestDoesNotMatch, "^response-only$"),
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::ResponseMatches, "^response-only$"),
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::ResponseDoesNotMatch, "^request-only$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::RequestMatches, "^client$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::RequestDoesNotMatch, "^server$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::ResponseMatches, "^server$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::ResponseDoesNotMatch, "^client$"),
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::ResponseDoesNotMatch, "^Path$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::ResponseDoesNotMatch, "^/request-only$"),
            cookie_body_rule(ProxyInterceptType::CookieName, ProxyInterceptRelation::RequestMatches, "^tail$"),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::RequestMatches, r#"^"part$"#),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::RequestMatches, &body_pattern(&request_body)),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::RequestDoesNotMatch, "response-visible"),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::RequestDoesNotMatch, "request-visible"),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::ResponseMatches, r"\x00\xffresponse-visible"),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::ResponseDoesNotMatch, &body_pattern(&request_body)),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::ResponseDoesNotMatch, &body_pattern(&response_body)),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        save(&mut client, project, Some(source.clone())).await;
        client.commands.send(Command::SetIntercept { project, requests: true, responses: true }).await.unwrap();
        client.commands.send(Command::StartProxy { project, listen: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };
        let downstream = send_cookie_gzip(proxy, target, "/response-cookies/held", &request_body).await;
        let mut origin = respond_cookie_gzip(&listener, "/response-cookies/held", &request_body, &response_body, true).await;
        let Event::InterceptPending { project: owner, stage, response_head_only, record: pending } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(owner, project);
        assert_eq!(stage, InterceptStage::Response);
        assert!(!response_head_only);
        assert_eq!(pending.request.body, request_body);
        assert_eq!(pending.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("cookie")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), REQUEST_COOKIES);
        let expected_response = pending.response.clone().unwrap();
        assert_cookie_response(&expected_response, RESPONSE_PLAIN, false);
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardResponse(expected_response) }).await.unwrap();
        cookie_body_response(downstream, RESPONSE_PLAIN, false).await;
        assert_eq!(origin.read(&mut [0]).await.unwrap(), 0);

        workspace.revision += 1;
        workspace.proxy_interception.response.rules[0].condition = "^response-only$".into();
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let downstream = send_cookie_gzip(proxy, target, "/response-cookies/not-request", &request_body).await;
        let mut origin = respond_cookie_gzip(&listener, "/response-cookies/not-request", &request_body, &response_body, true).await;
        cookie_body_response(downstream, RESPONSE_PLAIN, false).await;
        assert_eq!(origin.read(&mut [0]).await.unwrap(), 0);

        workspace.revision += 1;
        workspace.proxy_interception.response.rules[0].condition = "^request-only$".into();
        workspace.proxy_messages.unpack_compressed_responses = false;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let downstream = send_cookie_gzip(proxy, target, "/response-cookies/unpack-off", &request_body).await;
        let mut origin = respond_cookie_gzip(&listener, "/response-cookies/unpack-off", &request_body, &response_body, true).await;
        cookie_body_response(downstream, &response_body, true).await;
        assert_eq!(origin.read(&mut [0]).await.unwrap(), 0);

        workspace.revision += 1;
        workspace.proxy_messages.unpack_compressed_responses = true;
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let downstream = send_cookie_gzip(proxy, target, "/response-cookies/unpack-on-again", &request_body).await;
        let mut origin = respond_cookie_gzip(&listener, "/response-cookies/unpack-on-again", &request_body, &response_body, true).await;
        let Event::InterceptPending { project: owner, stage, response_head_only, record: pending } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(owner, project);
        assert_eq!(stage, InterceptStage::Response);
        assert!(!response_head_only);
        assert_eq!(pending.request.body, request_body);
        let expected_response = pending.response.clone().unwrap();
        assert_cookie_response(&expected_response, RESPONSE_PLAIN, false);
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardResponse(expected_response) }).await.unwrap();
        cookie_body_response(downstream, RESPONSE_PLAIN, false).await;
        assert_eq!(origin.read(&mut [0]).await.unwrap(), 0);

        workspace.revision += 1;
        workspace.proxy_interception.response.rules = vec![cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::ResponseDoesNotMatch, "not-present")];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let mut invalid_body = response_body.clone();
        let checksum = invalid_body.len() - 8;
        invalid_body[checksum] ^= 1;
        let mut downstream = send_cookie_gzip(proxy, target, "/response-cookies/bad-gzip", &request_body).await;
        let mut origin = respond_cookie_gzip(&listener, "/response-cookies/bad-gzip", &request_body, &invalid_body, true).await;
        let mut invalid_wire = Vec::new();
        downstream.read_to_end(&mut invalid_wire).await.unwrap();
        assert!(invalid_wire.starts_with(b"HTTP/1.1 400"), "bad gzip must fail visibly instead of being negated or automatically forwarded");
        assert_eq!(origin.read(&mut [0]).await.unwrap(), 0);
        client.commands.send(Command::StopProxy { project }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProxyState { address: None, cancel: None, .. })).await;
        assert!(TcpStream::connect(proxy).await.is_err());
        client.commands.send(Command::QueryHistory { project, query_revision: 801, offset: 0, limit: 10, filter: String::new() }).await.unwrap();
        let Event::HistoryPage { total, rows, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 801, .. })).await else { unreachable!() };
        assert_eq!(total, 5);
        assert_eq!(rows.len(), 5);
        let mut observed = Vec::new();
        for row in rows {
            let mio_core::HistoryRow::Live(summary) = row else { panic!("only actual Proxy records expected") };
            let (record, original) = detail_with_original(&mut client, project, &summary).await;
            assert_eq!(record.request.body, request_body);
            assert_eq!(record.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("cookie")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), REQUEST_COOKIES);
            assert_eq!(summary.status, Some(200));
            let response = record.response.as_ref().unwrap();
            if summary.url.ends_with("/bad-gzip") {
                let FlowState::Failed(message) = &summary.state else { panic!("bad gzip cannot be a successful negative match") };
                assert!(message.contains("解包"), "{message}");
                assert_cookie_response(response, &invalid_body, true);
                assert_eq!(original, OriginalMessages::default());
            } else if summary.url.ends_with("/unpack-off") {
                assert_eq!(summary.state, FlowState::Complete);
                assert_cookie_response(response, &response_body, true);
                assert_eq!(original, OriginalMessages::default());
            } else {
                assert_eq!(summary.state, FlowState::Complete);
                assert_cookie_response(response, RESPONSE_PLAIN, false);
                assert!(original.request.is_none());
                assert!(original.response_body_complete.is_none());
                assert_cookie_response(original.response.as_ref().unwrap(), &response_body, true);
            }
            observed.push((record, original));
        }
        let raw_response = observed.iter().find(|(record, _)| record.summary.url.ends_with("/unpack-off"))
            .unwrap().0.response.as_ref().unwrap();
        for (_, original) in &observed {
            if let Some(response) = &original.response {
                assert_eq!(response, raw_response, "the complete original gzip response must remain identical to the unpack-disabled capture");
            }
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
            assert!(restored.proxy_messages.unpack_compressed_responses);
            assert_eq!(reopened.id, project.id);
            assert!(reopened.generation > project.generation);
            let Event::ProxyState { address, cancel, .. } = next(&mut client).await else { panic!("reopen must report stopped Proxy") };
            assert!(address.is_none() && cancel.is_none());
            for (expected, expected_original) in &observed {
                let (record, original) = detail_with_original(&mut client, reopened, &expected.summary).await;
                assert_eq!(record, *expected);
                assert_eq!(original, *expected_original);
            }
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        no_extra_connection(listener);
    }).await.expect("request/response Cookie and body selection, gzip preservation, visible negative-match error and both reopens fit the eight second budget");
}

#[tokio::test]
async fn request_unpack_switch_changes_actual_post_bytes_and_retains_originals_through_stop_and_reopen()
 {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let source = directory.path().join("request-unpack.mio");
        let copy = directory.path().join("request-unpack-copy.mio");
        let request_body = compressed_fixture(REQUEST_PLAIN);
        let response_body = compressed_fixture(RESPONSE_PLAIN);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, mut workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        workspace.revision += 1;
        workspace.proxy_messages.unpack_compressed_requests = true;
        workspace.proxy_messages.unpack_compressed_responses = false;
        workspace.proxy_interception.request.enabled = true;
        workspace.proxy_interception.response.enabled = false;
        workspace.proxy_interception.request.rules = vec![
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::Matches, &body_pattern(REQUEST_PLAIN)),
            cookie_body_rule(ProxyInterceptType::Body, ProxyInterceptRelation::DoesNotMatch, &body_pattern(&request_body)),
            cookie_body_rule(ProxyInterceptType::CookieValue, ProxyInterceptRelation::Matches, "^two$"),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        save(&mut client, project, Some(source.clone())).await;
        client.commands.send(Command::SetIntercept { project, requests: true, responses: true }).await.unwrap();
        client.commands.send(Command::StartProxy { project, listen: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), cancel: Some(cancel), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };
        let mut held_revisions = Vec::new();
        for (index, unpack) in [true, false, true].into_iter().enumerate() {
            if index > 0 {
                workspace.revision += 1;
                workspace.proxy_messages.unpack_compressed_requests = unpack;
                client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
                acknowledged(&mut client, project, &workspace).await;
            }
            let path = format!("/request-unpack/{index}");
            let downstream = send_cookie_gzip(proxy, target, &path, &request_body).await;
            let expected_body = if unpack { REQUEST_PLAIN } else { &request_body };
            if unpack {
                let Event::InterceptPending { project: owner, stage, response_head_only, record: pending } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
                assert_eq!(owner, project);
                assert_eq!(stage, InterceptStage::Request);
                assert!(!response_head_only);
                assert_eq!(pending.request.body, REQUEST_PLAIN);
                assert_eq!(pending.summary.request_bytes, REQUEST_PLAIN.len() as u64);
                assert!(pending.summary.revision > 1);
                assert!(listener_pending(&listener), "decoded POST must still wait for its actual Forward");
                let (saved, original) = detail_with_original(&mut client, project, &pending.summary).await;
                assert_eq!(saved, *pending);
                let raw = original.request.as_ref().unwrap();
                assert_eq!(raw.body, request_body);
                let mut expected = pending.request.clone();
                expected.body = raw.body.clone();
                expected.headers = raw.headers.clone();
                assert_eq!(&expected, raw);
                held_revisions.push((pending.summary.id, pending.summary.revision));
                client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardRequest(pending.request.clone()) }).await.unwrap();
            }
            let mut origin = respond_cookie_gzip(&listener, &path, expected_body, &response_body, !unpack).await;
            cookie_body_response(downstream, &response_body, true).await;
            assert_eq!(origin.read(&mut [0]).await.unwrap(), 0);
        }

        let mut invalid_body = request_body.clone();
        let checksum = invalid_body.len() - 8;
        invalid_body[checksum] ^= 1;
        let mut downstream = send_cookie_gzip(proxy, target, "/request-unpack/bad-gzip", &invalid_body).await;
        let mut invalid_wire = Vec::new();
        downstream.read_to_end(&mut invalid_wire).await.unwrap();
        assert!(invalid_wire.starts_with(b"HTTP/1.1 400"), "bad request gzip must fail before any source connection or negated body match");
        assert!(listener_pending(&listener));

        let mut downstream = send_cookie_gzip(proxy, target, "/request-unpack/cancelled", &request_body).await;
        let Event::InterceptPending { stage, record: pending, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(stage, InterceptStage::Request);
        assert_eq!(pending.request.body, REQUEST_PLAIN);
        held_revisions.push((pending.summary.id, pending.summary.revision));
        assert!(listener_pending(&listener));
        cancel.cancel();
        client.commands.send(Command::StopProxy { project }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProxyState { address: None, cancel: None, .. })).await;
        let mut cancelled_wire = Vec::new();
        downstream.read_to_end(&mut cancelled_wire).await.unwrap();
        assert!(listener_pending(&listener), "Stop must not forward its already-decoded non-idempotent POST");
        assert!(TcpStream::connect(proxy).await.is_err());
        assert!(cancel.is_cancelled());

        client.commands.send(Command::QueryHistory { project, query_revision: 802, offset: 0, limit: 10, filter: String::new() }).await.unwrap();
        let Event::HistoryPage { total, rows, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 802, .. })).await else { unreachable!() };
        assert_eq!(total, 5);
        assert_eq!(rows.len(), 5);
        let mut observed = Vec::new();
        for row in rows {
            let mio_core::HistoryRow::Live(summary) = row else { panic!("only actual Proxy POSTs expected") };
            let (record, original) = detail_with_original(&mut client, project, &summary).await;
            assert_eq!(record.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("cookie")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), REQUEST_COOKIES);
            assert!(original.response.is_none() && original.response_body_complete.is_none());
            if summary.url.ends_with("/bad-gzip") {
                let FlowState::Failed(message) = &summary.state else { panic!("bad request gzip cannot be a successful negative match") };
                assert!(message.contains("解包"), "{message}");
                assert_eq!(record.request.body, invalid_body);
                assert!(record.response.is_none());
                assert_eq!(original, OriginalMessages::default());
            } else if summary.url.ends_with("/1") {
                assert_eq!(summary.state, FlowState::Complete);
                assert_eq!(record.request.body, request_body);
                assert_eq!(original, OriginalMessages::default());
            } else {
                assert_eq!(record.request.body, REQUEST_PLAIN);
                assert_eq!(summary.request_bytes, REQUEST_PLAIN.len() as u64);
                let raw = original.request.as_ref().unwrap();
                assert_eq!(raw.body, request_body);
                let (_, held_revision) = held_revisions.iter().find(|(id, _)| *id == summary.id).unwrap();
                assert!(summary.revision > *held_revision);
                let mut expected_raw = record.request.clone();
                expected_raw.body = request_body.clone();
                expected_raw.headers = raw.headers.clone();
                assert_eq!(&expected_raw, raw);
                if summary.id == pending.summary.id {
                    assert_eq!(summary.state, FlowState::Cancelled);
                    assert!(record.response.is_none());
                } else {
                    assert_eq!(summary.state, FlowState::Complete);
                }
            }
            if let Some(response) = &record.response {
                assert_cookie_response(response, &response_body, true);
            }
            observed.push((record, original));
        }
        let raw_request = &observed.iter().find(|(record, _)| record.summary.url.ends_with("/1")).unwrap().0.request;
        for (_, original) in &observed {
            if let Some(request) = &original.request {
                let mut expected = raw_request.clone();
                expected.url = request.url.clone();
                assert_eq!(request, &expected, "the whole original request must equal the unpack-disabled request apart from its path");
            }
        }
        save(&mut client, project, None).await;
        save(&mut client, project, Some(copy.clone())).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        opened(&mut client).await;
        for path in [source, copy] {
            client.commands.send(Command::OpenProject { path }).await.unwrap();
            let Event::ProjectOpened { project: reopened, workspace: restored, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
            assert_eq!(restored, workspace);
            assert!(restored.proxy_messages.unpack_compressed_requests);
            assert!(!restored.proxy_messages.unpack_compressed_responses);
            assert_eq!(reopened.id, project.id);
            assert!(reopened.generation > project.generation);
            let Event::ProxyState { address, cancel, .. } = next(&mut client).await else { panic!("reopen must report stopped Proxy") };
            assert!(address.is_none() && cancel.is_none());
            for (expected, expected_original) in &observed {
                let (record, original) = detail_with_original(&mut client, reopened, &expected.summary).await;
                assert_eq!(record, *expected);
                assert_eq!(original, *expected_original);
            }
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        no_extra_connection(listener);
    }).await.expect("first-send unpack, live switches, source byte checks, bad gzip/no-send Stop and both reopens fit the eight second budget");
}
