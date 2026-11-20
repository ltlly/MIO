use std::collections::BTreeMap;

use super::*;
use crate::{
    ExtImportReport, FlowProvenance, FlowState, Header, HttpVersion, IntruderRunState,
    IntruderSpec, ProvenanceActor, SlimSaveOptions, TrafficSource, WorkTab,
    intruder::{AttackType, Position},
};

fn make_flow(source: TrafficSource, url: &str, content_type: &str, body: Vec<u8>) -> FlowRecord {
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 2,
            source,
            method: "GET".into(),
            url: url.into(),
            status: Some(200),
            state: FlowState::Complete,
            request_bytes: 0,
            response_bytes: body.len() as u64,
            started_unix_ms: 1_700_000_000_000,
            elapsed_ms: 12,
        },
        request: HttpRequest {
            method: "GET".into(),
            url: url.into(),
            version: HttpVersion::Http11,
            headers: vec![Header {
                name: "Host".into(),
                value: b"example.com".to_vec(),
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        },
        response: Some(HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![Header {
                name: "Content-Type".into(),
                value: content_type.as_bytes().to_vec(),
            }],
            body,
            trailers: Vec::new(),
        }),
    }
}

#[test]
fn slim_save_keeps_pinned_and_repeater_flows_compacts_uncompressed_ext_chunks_and_passes_schema_guard()
 {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("source.mio");
    let dst_path = dir.path().join("slimmed.mio");

    let mut store = ProjectStore::create(Some(&src_path)).unwrap();

    let pinned_tab_flow = make_flow(
        TrafficSource::Proxy,
        "https://example.com/logo.png",
        "image/png",
        vec![0x89; 64 * 1024],
    );
    store.put(&pinned_tab_flow).unwrap();
    let mut ws = store.workspace().unwrap();
    let mut tab = WorkTab::from_request(Some(&pinned_tab_flow.request), "1".into());
    tab.last_flow = Some(pinned_tab_flow.summary.id);
    ws.tabs.push(tab);
    ws.active = Some(ws.tabs[0].id);
    ws.revision += 1;
    store.save_workspace(&ws).unwrap();

    let mut annotated_flow = make_flow(
        TrafficSource::Proxy,
        "https://example.com/app.css",
        "text/css",
        vec![b'a'; 64 * 1024],
    );
    let orig_annotated_req = annotated_flow.request.clone();
    annotated_flow.request.headers.push(Header {
        name: "X-Trace".into(),
        value: b"1".to_vec(),
    });
    store.put(&annotated_flow).unwrap();
    {
        let tx = store.connection.transaction().unwrap();
        let mut map = BTreeMap::new();
        map.insert(
            annotated_flow.summary.id,
            plugin_storage::LiveHttpAnnotation {
                notes_revision: 1,
                notes: "important finding".into(),
                highlight_revision: 0,
                highlight: 0,
            },
        );
        plugin_storage::write_live_http_annotations(&tx, &mut map, &CancellationToken::new())
            .unwrap();
        tx.commit().unwrap();
    }

    let repeater_flow = make_flow(
        TrafficSource::Repeater,
        "https://example.com/api/debug",
        "application/json",
        vec![b'R'; 128 * 1024],
    );
    store.put(&repeater_flow).unwrap();

    let scanner_flow = make_flow(
        TrafficSource::Scanner,
        "https://example.com/probe",
        "text/html",
        vec![b'S'; 128 * 1024],
    );
    store.put(&scanner_flow).unwrap();

    let unpinned_image = make_flow(
        TrafficSource::Proxy,
        "https://example.com/banner.png",
        "image/png",
        vec![b'I'; 256 * 1024],
    );
    store.put(&unpinned_image).unwrap();

    let oversized_proxy = make_flow(
        TrafficSource::Proxy,
        "https://example.com/bundle.json",
        "application/json",
        vec![b'J'; 256 * 1024],
    );
    store.put(&oversized_proxy).unwrap();

    let intr_draft = WorkTab::from_request(Some(&repeater_flow.request), "intr".into()).draft;
    let spec = IntruderSpec {
        revision: 1,
        draft: intr_draft,
        attack_type: AttackType::Sniper,
        positions: vec![Position { start: 4, end: 5 }],
        payload_sets: vec![vec![b"x".to_vec()]],
        max_concurrent_requests: 4,
        request_timeout_ms: 5000,
        update_host_header: true,
        save_to_project: true,
        baseline_request: false,
        resource_pool: None,
    };
    let mut ws = store.workspace().unwrap();
    let intr_tab_id = Uuid::new_v4();
    ws.intruder.tabs.push(crate::IntruderTab {
        id: intr_tab_id,
        title: "1".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    ws.revision += 1;
    store.save_workspace(&ws).unwrap();
    let run_id = Uuid::new_v4();
    store
        .create_intruder_run(run_id, intr_tab_id, &spec, 1_700_000_000_000)
        .unwrap();
    let mut ws2 = store.workspace().unwrap();
    ws2.intruder.tabs[0].last_run = Some(run_id);
    ws2.revision += 1;
    store.save_workspace(&ws2).unwrap();
    let intr_flow = make_flow(
        TrafficSource::Intruder,
        "https://example.com/fuzz",
        "text/plain",
        vec![b'F'; 64 * 1024],
    );
    store
        .put_intruder_result(run_id, 0, Ok(&intr_flow))
        .unwrap();
    store
        .set_intruder_state(run_id, IntruderRunState::Completed)
        .unwrap();

    let ext_source = Uuid::new_v4();
    let archive_bytes = vec![b'B'; 128 * 1024];
    let mut ext_report = ExtImportReport {
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: archive_bytes.len() as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: Vec::new(),
        repeater_active_tab: None,
        gaps: vec!["test archive".into()],
        complete_archive: false,
    };
    store.begin_source(ext_source, &ext_report).unwrap();
    store
        .append_source_chunk(ext_source, 0, &archive_bytes)
        .unwrap();
    ext_report.complete_archive = true;
    store.finish_source(ext_source, &ext_report).unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_source_chunks SET codec=0, data=?1 WHERE source=?2 AND ordinal=0",
            params![&archive_bytes, ext_source.to_string()],
        )
        .unwrap();

    let mut pinned_prov = FlowProvenance::default();
    pinned_prov.push_request_step(
        &orig_annotated_req,
        ProvenanceActor::SessionRule {
            rule_name: "AddTrace".into(),
        },
        &annotated_flow.request,
    );
    store
        .put_flow_provenance(annotated_flow.summary.id, &pinned_prov)
        .unwrap();
    store
        .put_flow_provenance(Uuid::new_v4(), &pinned_prov)
        .unwrap();

    store.save(None).unwrap();

    let opts = SlimSaveOptions {
        drop_scanner_and_unreferenced_tool_flows: true,
        drop_static_asset_flows: true,
        drop_finished_intruder_results: true,
        compact_ext_source_chunks: true,
        max_unpinned_body_bytes: Some(32 * 1024),
    };
    let report = store
        .slim_save_as(
            &dst_path,
            &opts,
            &[],
            &[],
            &[],
            &[],
            &CancellationToken::new(),
        )
        .unwrap();

    assert!(
        report.new_bytes < report.old_bytes,
        "slimmed DB ({}) must be smaller than original ({})",
        report.new_bytes,
        report.old_bytes
    );
    assert_eq!(report.removed_flows, 4);
    assert_eq!(report.compacted_ext_chunks, 1);

    let reopened = ProjectStore::open(&dst_path).unwrap();
    assert!(
        reopened
            .read_optional(pinned_tab_flow.summary.id)
            .unwrap()
            .is_some()
    );
    assert!(
        reopened
            .read_optional(annotated_flow.summary.id)
            .unwrap()
            .is_some()
    );
    assert!(
        reopened
            .read_optional(repeater_flow.summary.id)
            .unwrap()
            .is_some()
    );
    assert!(
        reopened
            .read_optional(scanner_flow.summary.id)
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .read_optional(unpinned_image.summary.id)
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .read_optional(oversized_proxy.summary.id)
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .read_optional(intr_flow.summary.id)
            .unwrap()
            .is_none()
    );

    let recovered_chunk = ext_archive::read_chunk(
        &reopened.connection,
        ext_source,
        archive_bytes.len() as u64,
        0,
    )
    .unwrap();
    assert_eq!(recovered_chunk, archive_bytes);

    let restored_orig = reopened.original(annotated_flow.summary.id).unwrap();
    assert_eq!(restored_orig.provenance.request_steps.len(), 2);
    let orphan_count: i64 = reopened
        .connection
        .query_row(
            "SELECT count(*) FROM metadata WHERE key GLOB 'flow_provenance:*'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphan_count, 1);
    reopened.close().unwrap();
    store.close().unwrap();
}

#[test]
fn multi_step_flow_provenance_chain_persists_and_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prov.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let mut flow = make_flow(
        TrafficSource::Proxy,
        "https://example.com/login",
        "application/json",
        b"{\"ok\":true}".to_vec(),
    );
    let orig_req = flow.request.clone();
    let mut step1_req = orig_req.clone();
    step1_req.headers.push(Header {
        name: "X-Mnr".into(),
        value: b"applied".to_vec(),
    });
    let mut step2_req = step1_req.clone();
    step2_req.headers.push(Header {
        name: "Cookie".into(),
        value: b"sid=abc".to_vec(),
    });
    let mut step3_req = step2_req.clone();
    step3_req.body = b"user=admin".to_vec();
    flow.request = step3_req.clone();

    let orig_resp = flow.response.clone().unwrap();
    let mut unpacked_resp = orig_resp.clone();
    unpacked_resp.body = b"{\"ok\":true,\"unpacked\":1}".to_vec();
    flow.response = Some(unpacked_resp.clone());

    store.put(&flow).unwrap();
    let mut prov = FlowProvenance::default();
    prov.push_request_step(
        &orig_req,
        ProvenanceActor::MatchAndReplace {
            rule_id: Uuid::from_u128(10),
            comment: "Inject X-Mnr".into(),
        },
        &step1_req,
    );
    prov.push_request_step(
        &orig_req,
        ProvenanceActor::SessionRule {
            rule_name: "Attach Session Cookie".into(),
        },
        &step2_req,
    );
    prov.push_request_step(&orig_req, ProvenanceActor::ManualInterceptEdit, &step3_req);
    prov.push_response_step(
        &orig_resp,
        ProvenanceActor::UnpackCompressed,
        &unpacked_resp,
    );

    store.put_flow_provenance(flow.summary.id, &prov).unwrap();
    store.save(None).unwrap();
    store.close().unwrap();

    let reopened = ProjectStore::open(&path).unwrap();
    let orig = reopened.original(flow.summary.id).unwrap();
    assert_eq!(orig.provenance.request_steps.len(), 4);
    assert_eq!(
        orig.provenance.request_steps[0].actor,
        ProvenanceActor::Original
    );
    assert!(matches!(
        orig.provenance.request_steps[1].actor,
        ProvenanceActor::MatchAndReplace { .. }
    ));
    assert!(matches!(
        orig.provenance.request_steps[2].actor,
        ProvenanceActor::SessionRule { .. }
    ));
    assert_eq!(
        orig.provenance.request_steps[3].actor,
        ProvenanceActor::ManualInterceptEdit
    );
    assert_eq!(orig.provenance.response_steps.len(), 2);
    assert_eq!(
        orig.provenance.response_steps[1].actor,
        ProvenanceActor::UnpackCompressed
    );
    reopened.close().unwrap();
}
