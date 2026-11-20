use super::*;

pub(super) fn repeater_records() -> (FlowRecord, FlowRecord, FlowRecord) {
    use std::io::Write;

    let mut pending = record();
    pending.summary.source = TrafficSource::Repeater;
    pending.summary.state = FlowState::Pending;
    pending.summary.elapsed_ms = 0;
    let mut received = pending.clone();
    let body = vec![0, 255, b'R', 0, 254];
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&body).unwrap();
    let encoded = encoder.finish().unwrap();
    received.summary.revision = 2;
    received.summary.state = FlowState::Complete;
    received.summary.status = Some(201);
    received.summary.response_bytes = encoded.len() as u64;
    received.summary.elapsed_ms = 1;
    received.response = Some(HttpResponse {
        status: 201,
        version: HttpVersion::Http2,
        reason_phrase: None,
        headers: vec![
            Header {
                name: "Content-Encoding".into(),
                value: b"gzip".to_vec(),
            },
            Header {
                name: "Content-Length".into(),
                value: encoded.len().to_string().into_bytes(),
            },
            Header {
                name: "X-Duplicate".into(),
                value: vec![255],
            },
            Header {
                name: "X-Duplicate".into(),
                value: b"two".to_vec(),
            },
        ],
        body: encoded,
        trailers: vec![Header {
            name: "X-End".into(),
            value: vec![0, 254],
        }],
    });
    let mut decoded = received.clone();
    decoded.summary.revision = 3;
    decoded.summary.elapsed_ms = 2;
    decoded.summary.response_bytes = body.len() as u64;
    let response = decoded.response.as_mut().unwrap();
    response.headers.remove(0);
    response.headers[0].value = body.len().to_string().into_bytes();
    response.body = body;
    (pending, received, decoded)
}

#[test]
fn repeater_completion_preserves_original_and_repeated_final_is_idempotent() {
    let mut store = ProjectStore::create(None).unwrap();
    let (pending, received, decoded) = repeater_records();
    store.put(&pending).unwrap();
    store.put_repeater_completion(&received, &decoded).unwrap();
    let original = OriginalMessages {
        response: received.response.clone(),
        ..OriginalMessages::default()
    };
    assert_eq!(store.read(decoded.summary.id).unwrap(), decoded);
    assert_eq!(store.original(decoded.summary.id).unwrap(), original);
    let changes: u64 = store
        .connection
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    store.put_repeater_completion(&received, &decoded).unwrap();
    let after: u64 = store
        .connection
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, changes);
    assert_eq!(store.read(decoded.summary.id).unwrap(), decoded);
    assert_eq!(store.original(decoded.summary.id).unwrap(), original);
    let sides: Vec<u8> = store
        .connection
        .prepare("SELECT side FROM messages WHERE flow=?1 ORDER BY side")
        .unwrap()
        .query_map([decoded.summary.id.to_string()], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(sides, [0, 1, 3]);
}

#[test]
fn repeater_completion_rejects_invalid_pairs_without_changing_pending() {
    let mut store = ProjectStore::create(None).unwrap();
    let (pending, received, decoded) = repeater_records();
    store.put(&pending).unwrap();
    for case in 0..8 {
        let mut invalid = decoded.clone();
        match case {
            0 => invalid.summary.id = Uuid::new_v4(),
            1 => invalid.request.body.push(42),
            2 => invalid.summary.revision += 1,
            3 => invalid.summary.source = TrafficSource::Proxy,
            4 => invalid.summary.state = FlowState::Pending,
            5 => invalid.response = None,
            6 => invalid.summary.response_bytes += 1,
            7 => invalid.summary.status = Some(202),
            _ => unreachable!(),
        }
        assert!(
            store.put_repeater_completion(&received, &invalid).is_err(),
            "case {case}"
        );
        assert_eq!(store.read(pending.summary.id).unwrap(), pending);
        assert_eq!(
            store.original(pending.summary.id).unwrap(),
            OriginalMessages::default()
        );
    }
    let mut other_request = received.clone();
    other_request.request.body.push(7);
    other_request.summary.request_bytes += 1;
    let mut other_final = decoded.clone();
    other_final.request = other_request.request.clone();
    other_final.summary.request_bytes = other_request.summary.request_bytes;
    assert!(
        store
            .put_repeater_completion(&other_request, &other_final)
            .is_err()
    );
    assert_eq!(store.read(pending.summary.id).unwrap(), pending);
}

#[test]
fn repeater_completion_final_write_failure_rolls_back_received_and_original() {
    let mut store = ProjectStore::create(None).unwrap();
    let (pending, received, decoded) = repeater_records();
    store.put(&pending).unwrap();
    store
        .connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_decoded BEFORE UPDATE ON flows WHEN NEW.revision=3
             BEGIN SELECT RAISE(ABORT,'synthetic decoded write failure'); END;",
        )
        .unwrap();
    let error = store
        .put_repeater_completion(&received, &decoded)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("synthetic decoded write failure")
    );
    assert_eq!(store.read(pending.summary.id).unwrap(), pending);
    assert_eq!(
        store.original(pending.summary.id).unwrap(),
        OriginalMessages::default()
    );
    store
        .connection
        .execute_batch("DROP TRIGGER reject_decoded;")
        .unwrap();
    store.put_repeater_completion(&received, &decoded).unwrap();
    assert_eq!(store.read(decoded.summary.id).unwrap(), decoded);
    assert_eq!(
        store.original(decoded.summary.id).unwrap().response,
        received.response
    );
}

#[test]
fn repeater_completion_rejects_conflicting_final_or_missing_or_changed_original() {
    for case in 0..3 {
        let mut store = ProjectStore::create(None).unwrap();
        let (pending, received, mut decoded) = repeater_records();
        store.put(&pending).unwrap();
        store.put_repeater_completion(&received, &decoded).unwrap();
        let saved = decoded.clone();
        match case {
            0 => decoded.response.as_mut().unwrap().body[0] ^= 1,
            1 => {
                store
                    .connection
                    .execute(
                        "DELETE FROM messages WHERE flow=?1 AND side=3",
                        [decoded.summary.id.to_string()],
                    )
                    .unwrap();
            }
            2 => {
                store
                    .connection
                    .execute(
                        "UPDATE messages SET body=?2 WHERE flow=?1 AND side=3",
                        params![decoded.summary.id.to_string(), [0u8, 255].as_slice()],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let original = store.original(decoded.summary.id).unwrap();
        assert!(
            store.put_repeater_completion(&received, &decoded).is_err(),
            "case {case}"
        );
        assert_eq!(store.read(decoded.summary.id).unwrap(), saved);
        assert_eq!(store.original(decoded.summary.id).unwrap(), original);
    }
}
