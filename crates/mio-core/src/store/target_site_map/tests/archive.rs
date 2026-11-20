use std::collections::HashSet;

use rusqlite::{
    hooks::{AuthContext, Authorization},
    params,
};

use super::*;
use crate::TargetScope;

#[test]
fn aggregates_live_pending_and_ext_archive_without_reading_bodies_and_filters_scope() {
    let mut store = crate::store::ProjectStore::create(None).unwrap();
    let source_id = crate::Uuid::new_v4();
    let mut archive = ArchiveSpec::new(source_id);
    let archive_item = archive.add_item(ArchiveItemSpec {
        collection: "root.f1.f0",
        index: 0,
        address: 1000,
        host: "target.example",
        port: 443,
        tls: true,
        method: "GET",
        target: "/api/v1",
        status: 200,
    });
    archive.add_item(ArchiveItemSpec {
        collection: "root.f1.f1",
        index: 0,
        address: 1000,
        host: "target.example",
        port: 443,
        tls: true,
        method: "GET",
        target: "/api/v1",
        status: 200,
    });
    let other_item = archive.add_item(ArchiveItemSpec {
        collection: "root.f1.f0",
        index: 1,
        address: 2000,
        host: "other.example",
        port: 443,
        tls: true,
        method: "GET",
        target: "/outside",
        status: 200,
    });
    archive.add_null_slot("root.f1.f0", 2);
    load_archive(&mut store, archive);
    let live = live_flow(
        "https://target.example/api/v2",
        "GET",
        Some(201),
        TrafficSource::Repeater,
        FlowState::Complete,
    );
    let missing = live_flow(
        "https://target.example/api/missing",
        "GET",
        Some(404),
        TrafficSource::Proxy,
        FlowState::Complete,
    );
    store.put(&live).unwrap();
    store.put(&missing).unwrap();
    let orphan_summary = crate::FlowSummary {
        id: crate::Uuid::new_v4(),
        revision: 1,
        source: TrafficSource::Proxy,
        method: "GET".into(),
        url: "https://target.example/orphan".into(),
        status: Some(200),
        state: FlowState::Complete,
        request_bytes: 0,
        response_bytes: 0,
        started_unix_ms: 900,
        elapsed_ms: 1,
    };
    store
        .connection
        .execute(
            "INSERT INTO flows(id,revision,started,url,method,state,summary) VALUES(?1,1,900,?2,'GET','complete',?3)",
            params![
                orphan_summary.id.to_string(),
                orphan_summary.url,
                serde_json::to_vec(&orphan_summary).unwrap(),
            ],
        )
        .unwrap();
    let pending_summary = crate::FlowSummary {
        id: crate::Uuid::new_v4(),
        revision: 1,
        source: TrafficSource::Proxy,
        method: "GET".into(),
        url: "https://target.example/api/pending".into(),
        status: None,
        state: FlowState::Pending,
        request_bytes: 0,
        response_bytes: 0,
        started_unix_ms: 500,
        elapsed_ms: 0,
    };
    let pending = crate::HttpHistoryPending {
        summary: pending_summary.clone(),
        first_request_method: pending_summary.method.clone(),
        first_request_url: pending_summary.url.clone(),
        first_response_status: None,
        first_request: None,
        first_response: None,
        request: None,
        response: None,
        listener_port: None,
        capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
    };
    store.connection.authorizer(Some(deny_bodies));
    let all = store
        .target_site_map_snapshot(
            &TargetScope::default(),
            &SiteMapQuery::default(),
            std::slice::from_ref(&pending),
        )
        .unwrap();
    assert_eq!(all.total_items, 5);
    assert_eq!(all.items.len(), 5);
    let ids: HashSet<_> = all.items.iter().map(crate::HistoryRow::id).collect();
    assert!(ids.contains(&crate::HistoryEntryId::External {
        source_id,
        item_id: archive_item,
    }));
    assert!(ids.contains(&crate::HistoryEntryId::External {
        source_id,
        item_id: other_item,
    }));
    assert!(ids.contains(&crate::HistoryEntryId::Live(live.summary.id)));
    assert!(ids.contains(&crate::HistoryEntryId::Live(missing.summary.id)));
    assert!(ids.contains(&crate::HistoryEntryId::Live(pending_summary.id)));
    assert!(!ids.contains(&crate::HistoryEntryId::Live(orphan_summary.id)));
    let target_origin = all
        .nodes
        .iter()
        .find(|node| node.key == "https://target.example:443")
        .unwrap();
    assert_eq!(target_origin.subtree_count, 4);
    let api_folder = all
        .nodes
        .iter()
        .find(|node| node.key == "https://target.example:443/api")
        .unwrap();
    assert_eq!(api_folder.subtree_count, 4);

    let scope = TargetScope {
        include: vec![scope_rule("https://target.example")],
        exclude: vec![scope_rule("https://target.example/api/missing")],
        ..Default::default()
    };
    let mut scoped_query = SiteMapQuery::default();
    scoped_query.filter.in_scope_only = true;
    let scoped = store
        .target_site_map_snapshot(&scope, &scoped_query, std::slice::from_ref(&pending))
        .unwrap();
    assert_eq!(scoped.total_items, 3);
    assert!(
        !scoped
            .nodes
            .iter()
            .any(|node| node.key == "https://other.example:443")
    );
    assert!(scoped.nodes.iter().all(|node| node.in_scope));
    assert!(
        !scoped
            .items
            .iter()
            .any(|row| row.id() == crate::HistoryEntryId::Live(missing.summary.id))
    );

    let mut status_query = SiteMapQuery::default();
    status_query.filter.status_classes = [true, false, false, false];
    let successes = store
        .target_site_map_snapshot(
            &TargetScope::default(),
            &status_query,
            std::slice::from_ref(&pending),
        )
        .unwrap();
    assert_eq!(successes.total_items, 4);
    let mut hidden_query = SiteMapQuery::default();
    hidden_query.filter.hide_not_found = true;
    let hidden = store
        .target_site_map_snapshot(
            &TargetScope::default(),
            &hidden_query,
            std::slice::from_ref(&pending),
        )
        .unwrap();
    assert_eq!(hidden.total_items, 4);
    let mut search_query = SiteMapQuery::default();
    search_query.filter.search_term = "https://target.example/api".into();
    let searched = store
        .target_site_map_snapshot(
            &TargetScope::default(),
            &search_query,
            std::slice::from_ref(&pending),
        )
        .unwrap();
    assert_eq!(searched.total_items, 4);
    assert!(
        !searched
            .nodes
            .iter()
            .any(|node| node.key == "https://other.example:443")
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
}

#[test]
fn explicit_site_map_entries_persist_across_reopen_deduplicate_owner_and_keep_wire_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("site-map-explicit.mio");
    let mut store = crate::store::ProjectStore::create(Some(&path)).unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let owner_id = crate::Uuid::new_v4();
    let draft = crate::Draft {
        revision: 1,
        target: "https://target.example:443".into(),
        raw: b"POST /api/draft?x=1 HTTP/1.1\r\nHost: target.example\r\nX-Custom:  keep \r\nContent-Length: 4\r\n\r\nbody".to_vec(),
        version: HttpVersion::Http11,
    };
    let response_raw = b"HTTP/1.1 202 Accepted\r\nX-Resp: 1\r\nContent-Length: 2\r\n\r\nok".to_vec();
    let detail = SiteMapEntryDetail::from_draft(
        owner_id,
        TrafficSource::Repeater,
        &draft,
        Some(response_raw.clone()),
    )
    .unwrap();
    assert_eq!(detail.record.summary.state, FlowState::Pending);
    let first = store
        .add_site_map_entry(
            0,
            &format!("draft:Repeater:{owner_id}"),
            crate::HistoryRow::Live(detail.record.summary.clone()),
            detail.clone(),
            true,
            &cancel,
        )
        .unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.node_key, "https://target.example:443/api/draft");
    assert_eq!(
        first.row.id(),
        crate::HistoryEntryId::Live(first.entry_id)
    );
    let unchanged = store
        .add_site_map_entry(
            0,
            &format!("draft:Repeater:{owner_id}"),
            crate::HistoryRow::Live(detail.record.summary.clone()),
            detail,
            true,
            &cancel,
        )
        .unwrap();
    assert_eq!(unchanged.entry_id, first.entry_id);
    assert_eq!(unchanged.revision, 1);

    let mut updated_draft = draft.clone();
    updated_draft.revision = 2;
    updated_draft.raw = b"POST /api/draft?x=2 HTTP/1.1\r\nHost: target.example\r\nContent-Length: 5\r\n\r\nbody2".to_vec();
    let updated_detail = SiteMapEntryDetail::from_draft(
        owner_id,
        TrafficSource::Repeater,
        &updated_draft,
        Some(response_raw.clone()),
    )
    .unwrap();
    let updated = store
        .add_site_map_entry(
            0,
            &format!("draft:Repeater:{owner_id}"),
            crate::HistoryRow::Live(updated_detail.record.summary.clone()),
            updated_detail,
            true,
            &cancel,
        )
        .unwrap();
    assert_eq!(updated.entry_id, first.entry_id);
    assert_eq!(updated.revision, 2);
    store.save(None).unwrap();
    store.close().unwrap();

    let reopened = crate::store::ProjectStore::open(&path).unwrap();
    let loaded = reopened
        .read_site_map_entry(updated.entry_id, updated.revision)
        .unwrap();
    assert_eq!(loaded.request_raw, updated_draft.raw);
    assert_eq!(loaded.response_raw.as_deref(), Some(response_raw.as_slice()));
    assert!(reopened.read_site_map_entry(updated.entry_id, 1).is_err());
    let snapshot = reopened
        .target_site_map_snapshot(&TargetScope::default(), &SiteMapQuery::default(), &[])
        .unwrap();
    assert_eq!(snapshot.total_items, 1);
    assert_eq!(
        snapshot.explicit_entries,
        vec![(
            crate::HistoryEntryId::Live(updated.entry_id),
            updated.entry_id,
            2
        )]
    );
    reopened.close().unwrap();
}
