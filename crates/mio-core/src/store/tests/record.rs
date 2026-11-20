use super::*;

pub(in crate::store) fn record() -> FlowRecord {
    let request = HttpRequest {
        method: "POST".into(),
        url: "https://localhost/中文".into(),
        version: HttpVersion::Http2,
        headers: vec![
            Header {
                name: "x-test".into(),
                value: b"one".to_vec(),
            },
            Header {
                name: "x-test".into(),
                value: vec![255],
            },
        ],
        trailers: vec![Header {
            name: "x-end".into(),
            value: b"end".to_vec(),
        }],
        body: vec![0, 255, 1],
    };
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: request.method.clone(),
            url: request.url.clone(),
            status: None,
            state: FlowState::Dropped,
            request_bytes: 3,
            response_bytes: 0,
            started_unix_ms: 10,
            elapsed_ms: 2,
        },
        request,
        response: None,
    }
}
