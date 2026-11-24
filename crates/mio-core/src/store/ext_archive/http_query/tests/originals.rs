use super::*;

#[test]
fn edited_live_history_uses_original_request_and_response_for_filtering_like_archive() {
    let (mut store, _, _) = super::super::super::proxy_history::tests::fixture(0, false, false);
    let mut original = live(
        "http://127.0.0.1:19000/original-only",
        Some(201),
        1,
        TrafficSource::Proxy,
    );
    original.summary.method = "POST".into();
    original.request.method = "POST".into();
    original.response.as_mut().unwrap().headers = vec![
        crate::Header {
            name: "X-Repeat".into(),
            value: b"before-one".to_vec(),
        },
        crate::Header {
            name: "X-Repeat".into(),
            value: b"before-two".to_vec(),
        },
    ];
    original.response.as_mut().unwrap().body = vec![0, 255, b'A'];
    original.summary.response_bytes = 3;
    store.put(&original).unwrap();

    let mut edited = original.clone();
    edited.summary.revision += 1;
    edited.summary.method = "PUT".into();
    edited.summary.url = "http://127.0.0.1:19000/edited-only".into();
    edited.summary.status = Some(404);
    edited.request.method = "PUT".into();
    edited.request.url = edited.summary.url.clone();
    let response = edited.response.as_mut().unwrap();
    response.status = 404;
    response.headers[0].value = b"after-one".to_vec();
    response.body = vec![254, 0, b'B'];
    store.put(&edited).unwrap();

    let id = HistoryEntryId::Live(original.summary.id);
    assert_eq!(store.read(original.summary.id).unwrap(), edited);
    let original_messages = store.original(original.summary.id).unwrap();
    assert_eq!(original_messages.request.as_ref(), Some(&original.request));
    assert_eq!(
        original_messages.response.as_ref(),
        original.response.as_ref()
    );
    let messages: Vec<(u8, Vec<u8>, Vec<u8>)> = store
        .connection
        .prepare("SELECT side,head,body FROM messages WHERE flow=?1 ORDER BY side")
        .unwrap()
        .query_map([original.summary.id.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        messages.iter().map(|row| row.0).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );

    let mut q = query(&store);
    q.selected_ids = vec![id];
    q.filter.hide_without_response = true;
    q.filter.status_classes = [true, false, false, false];
    store.connection.authorizer(Some(no_bodies));
    let success = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(
        success.total, 3,
        "both original 201 archive rows and the original 201 live response belong to 2xx"
    );
    assert_eq!(success.selected_matches, [(id, true)]);
    assert!(success.rows.iter().any(|row| row.id() == id));

    q.filter.status_classes = [false, false, true, false];
    let errors = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(
        errors.total, 0,
        "editing the response to 404 must not change history classification"
    );
    assert_eq!(errors.selected_matches, [(id, false)]);

    q.filter = HttpHistoryFilter::default();
    q.url_filter = "original-only".into();
    let original_url = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(original_url.total, 1);
    assert_eq!(original_url.rows[0].id(), id);
    q.url_filter = "edited-only".into();
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        0
    );
    q.url_filter = "PUT".into();
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        0
    );

    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(store.read(original.summary.id).unwrap(), edited);
    assert_eq!(
        store.original(original.summary.id).unwrap(),
        original_messages
    );
    let after: Vec<(u8, Vec<u8>, Vec<u8>)> = store
        .connection
        .prepare("SELECT side,head,body FROM messages WHERE flow=?1 ORDER BY side")
        .unwrap()
        .query_map([original.summary.id.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        after, messages,
        "history filtering never edits either message version"
    );
}

#[test]
fn hide_connect_filter_excludes_live_and_pending_connect_and_script_conversion_matches() {
    let mut store = ProjectStore::create(None).unwrap();

    let mut connect_no_resp = live("example.invalid:443", None, 10, TrafficSource::Proxy);
    connect_no_resp.summary.method = "CONNECT".into();
    connect_no_resp.request.method = "CONNECT".into();
    connect_no_resp.summary.state = FlowState::Failed("TLS handshake failed".into());
    store.put(&connect_no_resp).unwrap();

    let mut connect_502 = live("example.invalid:8443", Some(502), 20, TrafficSource::Proxy);
    connect_502.summary.method = "CONNECT".into();
    connect_502.request.method = "CONNECT".into();
    store.put(&connect_502).unwrap();

    let failed_get = live(
        "https://example.invalid/timeout",
        None,
        30,
        TrafficSource::Proxy,
    );
    store.put(&failed_get).unwrap();

    let ok_get = live(
        "https://example.invalid/ok",
        Some(200),
        40,
        TrafficSource::Proxy,
    );
    store.put(&ok_get).unwrap();

    let mut pending_connect = live("example.invalid:9443", None, 50, TrafficSource::Proxy);
    pending_connect.summary.method = "connect".into();
    pending_connect.request.method = "connect".into();
    pending_connect.summary.state = FlowState::Failed("TLS handshake failed".into());
    let pending = crate::HttpHistoryPending {
        summary: pending_connect.summary.clone(),
        first_request_method: pending_connect.request.method.clone(),
        first_request_url: pending_connect.request.url.clone(),
        first_response_status: None,
        first_request: Some(pending_connect.request.clone()),
        first_response: None,
        request: Some(pending_connect.request),
        response: None,
        listener_port: Some(8080),
        capture_metrics: Default::default(),
    };

    let cancel = CancellationToken::new();
    let mut q = query(&store);
    let all =
        super::super::query_connection(&store.connection, &q, std::slice::from_ref(&pending), &cancel)
            .unwrap();
    assert_eq!(all.total, 5);

    q.filter = HttpHistoryFilter {
        hide_connect: true,
        ..Default::default()
    };
    let filtered =
        super::super::query_connection(&store.connection, &q, std::slice::from_ref(&pending), &cancel)
            .unwrap();
    let filtered_ids: Vec<_> = filtered.rows.iter().map(HistoryRow::id).collect();
    assert_eq!(filtered.total, 2);
    assert_eq!(
        filtered_ids,
        vec![
            HistoryEntryId::Live(ok_get.summary.id),
            HistoryEntryId::Live(failed_get.summary.id),
        ],
        "hide_connect must hide CONNECT rows (with or without response) while keeping non-CONNECT no-response requests visible"
    );

    let converted = crate::convert_http_filter_to_script(&q.filter);
    q.filter = HttpHistoryFilter {
        filter_script: converted,
        ..Default::default()
    };
    let scripted =
        super::super::query_connection(&store.connection, &q, std::slice::from_ref(&pending), &cancel)
            .unwrap();
    assert_eq!(scripted.total, 2);
    let scripted_ids: Vec<_> = scripted.rows.iter().map(HistoryRow::id).collect();
    assert_eq!(scripted_ids, filtered_ids);

    let legacy: HttpHistoryFilter = serde_json::from_str(
        &serde_json::to_string(&HttpHistoryFilter::default())
            .unwrap()
            .replace(",\"hide_connect\":false", "")
            .replace("\"hide_connect\":false,", ""),
    )
    .unwrap();
    assert!(!legacy.hide_connect);
}
