use super::*;

#[tokio::test]
async fn param_name_and_value_rules_match_query_form_and_multipart_across_both_stages() {
    tokio::time::timeout(FIXTURE_BUDGET, async {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let source = directory.path().join("param-name-value.mio");
        let copy = directory.path().join("param-name-value-copy.mio");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, mut workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        workspace.revision += 1;
        workspace.proxy_messages.unpack_compressed_requests = false;
        workspace.proxy_messages.unpack_compressed_responses = false;
        workspace.proxy_interception.request.enabled = true;
        workspace.proxy_interception.response.enabled = false;
        workspace.proxy_interception.request.rules = vec![
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::Matches, r"^q%20name$"),
            cookie_body_rule(ProxyInterceptType::ParamValue, ProxyInterceptRelation::Matches, r"^q\+val%20raw$"),
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::Matches, r"^form_key$"),
            cookie_body_rule(ProxyInterceptType::ParamValue, ProxyInterceptRelation::Matches, r"^form_val\xff$"),
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::DoesNotMatch, r"^cookie_key$"),
            cookie_body_rule(ProxyInterceptType::ParamValue, ProxyInterceptRelation::DoesNotMatch, r"^cookie_val$"),
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::DoesNotMatch, r"^q name$"),
            cookie_body_rule(ProxyInterceptType::ParamValue, ProxyInterceptRelation::DoesNotMatch, r"^q val raw$"),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        save(&mut client, project, Some(source.clone())).await;
        client.commands.send(Command::SetIntercept { project, requests: true, responses: true }).await.unwrap();
        client.commands.send(Command::StartProxy { project, listen: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), cancel: Some(cancel), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };

        let send_param_req = |path_and_query: &str, content_type: Option<&str>, body: &[u8]| {
            let path_and_query = path_and_query.to_owned();
            let content_type = content_type.map(str::to_owned);
            let body = body.to_vec();
            async move {
                let mut stream = TcpStream::connect(proxy).await.unwrap();
                let ct_header = content_type
                    .map(|ct| format!("Content-Type: {ct}\r\n"))
                    .unwrap_or_default();
                let head = format!(
                    "POST http://{target}{path_and_query} HTTP/1.1\r\nHost: {target}\r\n{ct_header}Cookie: cookie_key=cookie_val\r\nX-Duplicate: first\r\nX-Duplicate: second\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
                stream
            }
        };

        let form_body = b"form_key=form_val\xff&extra=1";
        let held_form = send_param_req(
            "/param/query-and-form?q%20name=q+val%20raw",
            Some("application/x-www-form-urlencoded"),
            form_body,
        )
        .await;
        let Event::InterceptPending { project: owner, stage, response_head_only, record: pending } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(owner, project);
        assert_eq!(stage, InterceptStage::Request);
        assert!(!response_head_only);
        assert_eq!(pending.request.body, form_body);
        assert!(listener_pending(&listener));
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardRequest(pending.request.clone()) }).await.unwrap();
        respond(&listener, "/param/query-and-form?q%20name=q+val%20raw", form_body).await;
        response(held_form).await;

        let json_body = br#"{"q%20name":"q+val%20raw","form_key":"form_val"}"#;
        let unheld_json = send_param_req(
            "/param/cookie-and-json-bypass",
            Some("application/json"),
            json_body,
        )
        .await;
        respond(&listener, "/param/cookie-and-json-bypass", json_body).await;
        response(unheld_json).await;

        workspace.revision += 1;
        workspace.proxy_interception.request.rules = vec![
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::Matches, r"^\{"),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        for (path, content_type, body, held) in [
            ("/param/sniff-json-no-content-type", None, json_body.as_slice(), false),
            ("/param/sniff-malformed-text", Some("text/plain"), b"{broken".as_slice(), true),
        ] {
            let downstream = send_param_req(path, content_type, body).await;
            if held {
                let Event::InterceptPending { stage, response_head_only, record: pending, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
                assert_eq!(stage, InterceptStage::Request);
                assert!(!response_head_only);
                assert!(pending.summary.url.ends_with(path));
                assert_eq!(pending.request.body, body);
                assert!(listener_pending(&listener));
                client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardRequest(pending.request.clone()) }).await.unwrap();
            }
            respond(&listener, path, body).await;
            response(downstream).await;
        }

        let multipart_body = b"--bound123\r\nContent-Disposition: form-data; name=\"text_part\"\r\n\r\ntext_val\r\n--bound123\r\nContent-Disposition: form-data; name=\"file_part\"; filename=\"a.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n\0\xfffile_body\r\n--bound123--\r\n";
        workspace.revision += 1;
        workspace.proxy_interception.request.rules = vec![
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::Matches, r"^file_part$"),
            cookie_body_rule(ProxyInterceptType::ParamValue, ProxyInterceptRelation::Matches, r"^\x00\xfffile_body$"),
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::Matches, r"^text_part$"),
            cookie_body_rule(ProxyInterceptType::ParamValue, ProxyInterceptRelation::Matches, r"^text_val$"),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let held_mp = send_param_req(
            "/param/multipart-held",
            Some("multipart/form-data; boundary=bound123"),
            multipart_body,
        )
        .await;
        let Event::InterceptPending { stage, record: pending, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(stage, InterceptStage::Request);
        assert_eq!(pending.request.body, multipart_body);
        assert!(listener_pending(&listener));
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardRequest(pending.request.clone()) }).await.unwrap();
        respond(&listener, "/param/multipart-held", multipart_body).await;
        response(held_mp).await;

        workspace.revision += 1;
        workspace.proxy_interception.request.enabled = false;
        workspace.proxy_interception.response.enabled = true;
        workspace.proxy_interception.response.rules = vec![
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::Matches, r"^file_part$"),
            cookie_body_rule(ProxyInterceptType::ParamValue, ProxyInterceptRelation::Matches, r"^\x00\xfffile_body$"),
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::DoesNotMatch, r"^cookie_key$"),
            cookie_body_rule(ProxyInterceptType::ParamValue, ProxyInterceptRelation::DoesNotMatch, r"^cookie_val$"),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let held_resp = send_param_req(
            "/param/response-held",
            Some("multipart/form-data; boundary=bound123"),
            multipart_body,
        )
        .await;
        respond(&listener, "/param/response-held", multipart_body).await;
        let Event::InterceptPending { stage, response_head_only, record: pending, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(stage, InterceptStage::Response);
        assert!(!response_head_only);
        assert_eq!(pending.request.body, multipart_body);
        let expected_response = pending.response.clone().unwrap();
        assert_eq!(expected_response.body, [0, 255, b'R']);
        client.commands.send(Command::Decide { project, flow_id: pending.summary.id, revision: pending.summary.revision, decision: Decision::ForwardResponse(expected_response) }).await.unwrap();
        response(held_resp).await;

        workspace.revision += 1;
        workspace.proxy_interception.response.rules = vec![
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::DoesNotMatch, r"^not-present$"),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let broken_mp = b"--bound123\r\nContent-Disposition: form-data; name=\"broken\"\r\n\r\nunterminated";
        let mut broken_stream = send_param_req(
            "/param/response-broken-multipart",
            Some("multipart/form-data; boundary=bound123"),
            broken_mp,
        )
        .await;
        respond(&listener, "/param/response-broken-multipart", broken_mp).await;
        let mut broken_wire = Vec::new();
        broken_stream.read_to_end(&mut broken_wire).await.unwrap();
        assert!(
            broken_wire.starts_with(b"HTTP/1.1 400"),
            "corrupted multipart in DoesNotMatch must fail visibly with 400 instead of negating to a bypass"
        );

        workspace.revision += 1;
        workspace.proxy_interception.request.enabled = true;
        workspace.proxy_interception.response.enabled = false;
        workspace.proxy_interception.request.rules = vec![
            cookie_body_rule(ProxyInterceptType::ParamName, ProxyInterceptRelation::Matches, r"^hold_cancel$"),
        ];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        acknowledged(&mut client, project, &workspace).await;
        let mut cancelled_stream = send_param_req(
            "/param/cancelled?hold_cancel=1",
            Some("application/x-www-form-urlencoded"),
            form_body,
        )
        .await;
        let Event::InterceptPending { stage, record: cancelled_record, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
        assert_eq!(stage, InterceptStage::Request);
        assert_eq!(cancelled_record.request.body, form_body);
        assert!(listener_pending(&listener));
        cancel.cancel();
        client.commands.send(Command::StopProxy { project }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProxyState { address: None, cancel: None, .. })).await;
        let mut cancelled_wire = Vec::new();
        cancelled_stream.read_to_end(&mut cancelled_wire).await.unwrap();
        assert!(listener_pending(&listener), "Stop must never forward a held request");
        assert!(TcpStream::connect(proxy).await.is_err());
        assert!(cancel.is_cancelled());

        client.commands.send(Command::QueryHistory { project, query_revision: 805, offset: 0, limit: 10, filter: String::new() }).await.unwrap();
        let Event::HistoryPage { total, rows, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 805, .. })).await else { unreachable!() };
        assert_eq!(total, 8);
        assert_eq!(rows.len(), 8);
        let mut observed = Vec::new();
        for row in rows {
            let mio_core::HistoryRow::Live(summary) = row else { panic!("only loopback Proxy traffic expected") };
            let record = detail(&mut client, project, &summary).await;
            if summary.id == cancelled_record.summary.id {
                assert_eq!(summary.state, FlowState::Cancelled);
                assert!(record.response.is_none());
            } else if summary.url.ends_with("/param/response-broken-multipart") {
                let FlowState::Failed(message) = &summary.state else {
                    panic!("corrupted multipart cannot be Complete: {:?}", summary.state)
                };
                assert!(message.contains("multipart"), "{message}");
                assert_eq!(record.request.body, broken_mp);
                assert_eq!(record.response.as_ref().unwrap().body, [0, 255, b'R']);
            } else {
                assert_eq!(summary.state, FlowState::Complete);
                assert_eq!(record.response.as_ref().unwrap().body, [0, 255, b'R']);
                if summary.url.ends_with("/param/sniff-json-no-content-type") {
                    assert_eq!(record.request.body, json_body);
                    assert!(record.request.headers.iter().all(|header| !header.name.eq_ignore_ascii_case("content-type")));
                } else if summary.url.ends_with("/param/sniff-malformed-text") {
                    assert_eq!(record.request.body, b"{broken");
                    assert_eq!(record.request.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("content-type")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), [b"text/plain"]);
                }
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
    }).await.expect("ParamName/ParamValue rules across both stages, broken multipart failure, Stop and both reopens fit the eight second budget");
}
