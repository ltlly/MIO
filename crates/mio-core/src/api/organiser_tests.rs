use super::*;
use crate::{
    FlowState, FlowSummary, Header, HttpRequest, TrafficSource, next_organiser_number,
    validate_organiser_items,
};

fn sample_binary_flow() -> FlowRecord {
    let request = HttpRequest {
        method: "POST".into(),
        url: "https://example.com:8443/api/upload?lang=zh".into(),
        version: HttpVersion::Http2,
        headers: vec![
            Header {
                name: "Host".into(),
                value: b"example.com:8443".to_vec(),
            },
            Header {
                name: "X-Custom-Bin".into(),
                value: vec![0x61, 0xff, 0x62],
            },
        ],
        body: vec![0x00, 0xff, 0xfe, 0x80, b'M', b'I', b'O'],
        trailers: Vec::new(),
    };
    let response = HttpResponse {
        status: 201,
        version: HttpVersion::Http2,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: b"application/octet-stream".to_vec(),
        }],
        body: vec![0xde, 0xad, 0xbe, 0xef, 0x00],
        trailers: Vec::new(),
    };
    let req_len = request_to_raw(&request).len() as u64;
    let resp_len = response_to_raw(&response).len() as u64;
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: "https://example.com:8443/api/upload?lang=zh".into(),
            status: Some(201),
            state: FlowState::Complete,
            request_bytes: req_len,
            response_bytes: resp_len,
            started_unix_ms: 1_763_000_123_000,
            elapsed_ms: 15,
        },
        request,
        response: Some(response),
    }
}

#[test]
fn flow_and_draft_preserve_binary_payloads_and_serde_roundtrip() {
    let flow = sample_binary_flow();
    let mut item = OrganiserItem::from_flow_record(1, "Proxy", &flow).expect("from_flow_record");
    assert!(!item.id.is_nil());
    assert_eq!(item.number, 1);
    assert_eq!(item.added_unix_ms, 1_763_000_123_000);
    assert_eq!(item.source_tool, "Proxy");
    assert_eq!(item.target, "https://example.com:8443");
    assert_eq!(item.host(), "example.com:8443");
    assert_eq!(item.method, "POST");
    assert_eq!(item.url, "https://example.com:8443/api/upload?lang=zh");
    assert_eq!(item.status_code, Some(201));
    assert_eq!(item.http_version, HttpVersion::Http2);
    assert_eq!(item.workflow_status, OrganiserStatus::Unknown);
    assert!(
        item.request_raw
            .ends_with(&[0x00, 0xff, 0xfe, 0x80, b'M', b'I', b'O'])
    );
    assert!(
        item.response_raw
            .as_ref()
            .is_some_and(|raw| raw.ends_with(&[0xde, 0xad, 0xbe, 0xef, 0x00]))
    );

    item.workflow_status = OrganiserStatus::InProgress;
    item.highlight = Some(3);
    item.notes = "第一行中文备注\n第二行复现步骤\t保留换行".into();
    assert_eq!(
        item.notes_preview(),
        "第一行中文备注 第二行复现步骤 保留换行"
    );
    validate_organiser_items(std::slice::from_ref(&item)).expect("valid item");

    let json = serde_json::to_vec(&item).expect("serialize organiser item");
    let decoded: OrganiserItem = serde_json::from_slice(&json).expect("deserialize organiser item");
    assert_eq!(decoded, item);

    let mut raw = b"PUT /v2/blob HTTP/1.1\r\nHost: 127.0.0.1:9090\r\n\r\n".to_vec();
    raw.extend_from_slice(&[0xff, 0x00, 0xfe, 0x42]);
    let draft = Draft {
        revision: 4,
        target: "http://127.0.0.1:9090/old".into(),
        raw: raw.clone(),
        version: HttpVersion::Http11,
    };
    let resp = HttpResponse {
        status: 204,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: Vec::new(),
        body: Vec::new(),
        trailers: Vec::new(),
    };
    let from_draft = OrganiserItem::from_draft(
        next_organiser_number(std::slice::from_ref(&decoded)).unwrap(),
        "Repeater",
        &draft,
        Some(&resp),
    )
    .expect("from_draft");
    assert_eq!(from_draft.number, 2);
    assert_eq!(from_draft.target, "http://127.0.0.1:9090");
    assert_eq!(from_draft.url, "http://127.0.0.1:9090/v2/blob");
    assert_eq!(from_draft.method, "PUT");
    assert_eq!(from_draft.status_code, Some(204));
    assert_eq!(from_draft.request_raw, raw);
    validate_organiser_items(&[decoded, from_draft]).expect("valid collection");
}

#[test]
fn workflow_status_labels_and_weights_stay_ordered() {
    let expected = [
        (OrganiserStatus::Unknown, "未处理", 0, false),
        (OrganiserStatus::New, "新建", 1, false),
        (OrganiserStatus::InProgress, "处理中", 2, false),
        (OrganiserStatus::Postponed, "延后", 3, false),
        (OrganiserStatus::Done, "已完成", 4, true),
        (OrganiserStatus::Ignored, "已忽略", 5, true),
    ];
    assert_eq!(OrganiserStatus::default(), OrganiserStatus::Unknown);
    for (idx, (status, label, weight, cleanup)) in expected.into_iter().enumerate() {
        assert_eq!(OrganiserStatus::ALL[idx], status);
        assert_eq!(status.label(), label);
        assert_eq!(status.sort_weight(), weight);
        assert_eq!(status.is_terminal_cleanup_target(), cleanup);
    }
}

#[test]
fn validation_rejects_oversized_duplicate_and_malformed_items() {
    let base = OrganiserItem::from_flow_record(1, "Proxy", &sample_binary_flow()).unwrap();

    let mut nil_id = base.clone();
    nil_id.id = Uuid::nil();
    assert!(validate_organiser_items(&[nil_id]).is_err());

    let mut dup_id = base.clone();
    dup_id.number = 2;
    assert!(validate_organiser_items(&[base.clone(), dup_id]).is_err());

    let mut dup_num = base.clone();
    dup_num.id = Uuid::new_v4();
    assert!(validate_organiser_items(&[base.clone(), dup_num]).is_err());

    let mut oversized_req = base.clone();
    oversized_req.request_raw = vec![b'A'; MAX_ORGANISER_MESSAGE_BYTES + 1];
    assert!(oversized_req.validate().is_err());

    let mut oversized_resp = base.clone();
    oversized_resp.response_raw = Some(vec![b'B'; MAX_ORGANISER_MESSAGE_BYTES + 1]);
    assert!(oversized_resp.validate().is_err());

    let mut oversized_notes = base.clone();
    oversized_notes.notes = "中".repeat(MAX_ORGANISER_NOTES_BYTES / 3 + 2);
    assert!(oversized_notes.validate().is_err());

    let mut nul_notes = base.clone();
    nul_notes.notes = "bad\0note".into();
    assert!(nul_notes.validate().is_err());

    let mut bad_highlight = base.clone();
    bad_highlight.highlight = Some(0);
    assert!(bad_highlight.validate().is_err());
    bad_highlight.highlight = Some(10);
    assert!(bad_highlight.validate().is_err());

    let mut bad_target = base.clone();
    bad_target.target = "https://user:pass@example.com".into();
    assert!(bad_target.validate().is_err());

    let mut too_many = Vec::with_capacity(MAX_ORGANISER_ITEMS + 1);
    for index in 1..=(MAX_ORGANISER_ITEMS as u64 + 1) {
        let mut item = base.clone();
        item.id = Uuid::new_v4();
        item.number = index;
        too_many.push(item);
    }
    assert!(validate_organiser_items(&too_many).is_err());

    let mut heavy = Vec::new();
    for index in 1..=9_u64 {
        let mut item = base.clone();
        item.id = Uuid::new_v4();
        item.number = index;
        item.request_raw = vec![b'X'; MAX_ORGANISER_MESSAGE_BYTES];
        item.response_raw = None;
        heavy.push(item);
    }
    assert!(validate_organiser_items(&heavy).is_err());
}
