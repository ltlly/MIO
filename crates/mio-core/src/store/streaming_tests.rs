use super::*;
use crate::{FlowState, Header, HttpVersion};

fn capture() -> (FlowRecord, StreamedResponse) {
    let mut record = tests::record();
    record.summary.state = FlowState::Pending;
    record.summary.status = Some(200);
    record.response = Some(HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "content-type".into(),
            value: b"application/octet-stream".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    });
    let stream = StreamedResponse {
        original_status: 200,
        original_version: HttpVersion::Http11,
        original_headers: record.response.as_ref().unwrap().headers.clone(),
        body_complete: false,
    };
    (record, stream)
}

#[test]
fn streaming_metadata_and_observed_bytes_cannot_be_dropped_or_rewritten() {
    let mut store = ProjectStore::create(None).unwrap();
    let (mut record, mut stream) = capture();
    store.put_capture(&record, Some(&stream)).unwrap();
    record.summary.revision += 1;
    // Repeated empty head snapshots must not depend on SQLite substr(empty_blob) returning a blob.
    store.put_capture(&record, Some(&stream)).unwrap();
    record.summary.revision += 1;
    record.response.as_mut().unwrap().body = vec![0, 255];
    record.summary.response_bytes = 2;
    store.put_capture(&record, Some(&stream)).unwrap();
    assert!(
        store
            .original(record.summary.id)
            .unwrap()
            .response
            .is_none()
    );

    let mut next = record.clone();
    next.summary.revision += 1;
    assert!(
        store.put(&next).is_err(),
        "capture metadata cannot disappear"
    );
    let mut changed = stream.clone();
    changed.original_headers[0].value = b"text/event-stream".to_vec();
    assert!(store.put_capture(&next, Some(&changed)).is_err());
    next.response.as_mut().unwrap().body[0] = 42;
    assert!(store.put_capture(&next, Some(&stream)).is_err());
    assert_eq!(store.read(record.summary.id).unwrap(), record);

    record.summary.revision += 1;
    record.summary.state = FlowState::Complete;
    stream.body_complete = true;
    store.put_capture(&record, Some(&stream)).unwrap();
    store.put_capture(&record, Some(&stream)).unwrap();
    let mut changed = stream.clone();
    changed.body_complete = false;
    assert!(store.put_capture(&record, Some(&changed)).is_err());
    next = record.clone();
    next.summary.revision += 1;
    assert!(store.put_capture(&next, Some(&changed)).is_err());
    next.response.as_mut().unwrap().body.push(42);
    assert!(store.put_capture(&next, Some(&stream)).is_err());
    assert_eq!(store.read(record.summary.id).unwrap(), record);
}

#[test]
fn invalid_stream_heads_do_not_commit_or_allocate_an_oversized_stored_head() {
    let mut store = ProjectStore::create(None).unwrap();
    let (record, mut stream) = capture();
    stream.original_headers[0].value = vec![b'x'; StreamedResponse::MAX_HEAD_BYTES];
    assert!(store.put_capture(&record, Some(&stream)).is_err());
    assert!(store.read_optional(record.summary.id).unwrap().is_none());
    stream.original_headers = record.response.as_ref().unwrap().headers.clone();
    store.put_capture(&record, Some(&stream)).unwrap();
    store
        .connection
        .execute(
            "UPDATE messages SET head=zeroblob(?1) WHERE flow=?2 AND side=1",
            params![MAX_MESSAGE_HEAD_BYTES + 1, record.summary.id.to_string()],
        )
        .unwrap();
    assert!(read_response_head(&store.connection, &record.summary.id.to_string(), 1).is_err());
    assert!(store.read(record.summary.id).is_err());
}

#[test]
fn streaming_cannot_silently_replace_an_empty_text_body_with_a_blob() {
    let mut store = ProjectStore::create(None).unwrap();
    let (mut record, stream) = capture();
    store.put_capture(&record, Some(&stream)).unwrap();
    store
        .connection
        .execute(
            "UPDATE messages SET body='' WHERE flow=?1 AND side=1",
            [record.summary.id.to_string()],
        )
        .unwrap();
    record.summary.revision += 1;
    assert!(
        store.put_capture(&record, Some(&stream)).is_err(),
        "an empty TEXT is not an observed binary prefix"
    );
    let body_type: String = store
        .connection
        .query_row(
            "SELECT typeof(body) FROM messages WHERE flow=?1 AND side=1",
            [record.summary.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body_type, "text");
}

#[test]
fn streaming_cannot_append_trailers_after_eos() {
    let mut store = ProjectStore::create(None).unwrap();
    let (mut record, mut stream) = capture();
    record.summary.state = FlowState::Complete;
    stream.body_complete = true;
    store.put_capture(&record, Some(&stream)).unwrap();
    let mut changed = record.clone();
    changed.summary.revision += 1;
    changed.response.as_mut().unwrap().trailers.push(Header {
        name: "x-late".into(),
        value: b"after-eos".to_vec(),
    });
    assert!(
        store.put_capture(&changed, Some(&stream)).is_err(),
        "complete captures cannot acquire trailers after EOS"
    );
    assert_eq!(store.read(record.summary.id).unwrap(), record);
}
