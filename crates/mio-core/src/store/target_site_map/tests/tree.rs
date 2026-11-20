use super::*;
use crate::TargetScope;

#[test]
fn merges_same_origin_default_ports_and_separates_non_default_ports() {
    let mut store = crate::store::ProjectStore::create(None).unwrap();
    for (url, method, status, source) in [
        (
            "https://EXAMPLE.com/login",
            "GET",
            Some(200),
            TrafficSource::Proxy,
        ),
        (
            "https://example.com:443/api/users",
            "GET",
            Some(200),
            TrafficSource::Repeater,
        ),
        (
            "http://example.com:80/plain",
            "GET",
            Some(200),
            TrafficSource::Proxy,
        ),
        (
            "http://EXAMPLE.com/plain/sub",
            "POST",
            Some(201),
            TrafficSource::Intruder,
        ),
        (
            "https://example.com:8443/admin",
            "GET",
            Some(403),
            TrafficSource::Proxy,
        ),
    ] {
        store
            .put(&live_flow(url, method, status, source, FlowState::Complete))
            .unwrap();
    }
    let snapshot = store
        .target_site_map_snapshot(&TargetScope::default(), &SiteMapQuery::default(), &[])
        .unwrap();
    let origins: Vec<_> = snapshot
        .nodes
        .iter()
        .filter(|node| node.kind == SiteMapNodeKind::Origin)
        .collect();
    assert_eq!(
        origins
            .iter()
            .map(|node| (node.key.as_str(), node.label.as_str(), node.subtree_count))
            .collect::<Vec<_>>(),
        [
            ("http://example.com:80", "http://example.com", 2),
            ("https://example.com:443", "https://example.com", 2),
            ("https://example.com:8443", "https://example.com:8443", 1),
        ]
    );
    assert!(
        origins
            .iter()
            .all(|node| node.url_prefix == node.label && node.depth == 0)
    );
    let mut query = SiteMapQuery {
        selected_node_key: Some("http://example.com:80".into()),
        ..Default::default()
    };
    let http_page = store
        .target_site_map_snapshot(&TargetScope::default(), &query, &[])
        .unwrap();
    assert_eq!(http_page.total_items, 2);
    assert_eq!(http_page.items.len(), 2);
    assert!(http_page.items.iter().all(|row| matches!(
        row,
        crate::HistoryRow::Live(summary)
            if summary.url.to_ascii_lowercase().starts_with("http://example.com")
    )));
    query.selected_node_key = Some("https://example.com:443".into());
    let https_page = store
        .target_site_map_snapshot(&TargetScope::default(), &query, &[])
        .unwrap();
    assert_eq!(https_page.total_items, 2);
}

#[test]
fn truncates_and_merges_paths_beyond_sixteen_segments() {
    let mut store = crate::store::ProjectStore::create(None).unwrap();
    let prefix: String = (1..=15).map(|number| format!("/s{number}")).collect();
    for suffix in ["/s16/s17/s18/a", "/s16/s17/b"] {
        let url = format!("https://deep.example{prefix}{suffix}");
        store
            .put(&live_flow(
                &url,
                "GET",
                Some(200),
                TrafficSource::Proxy,
                FlowState::Complete,
            ))
            .unwrap();
    }
    let snapshot = store
        .target_site_map_snapshot(&TargetScope::default(), &SiteMapQuery::default(), &[])
        .unwrap();
    assert!(
        snapshot
            .nodes
            .iter()
            .all(|node| node.depth <= MAX_SITE_MAP_DEPTH)
    );
    let deepest = snapshot.nodes.iter().max_by_key(|node| node.depth).unwrap();
    assert_eq!(deepest.depth, MAX_SITE_MAP_DEPTH);
    assert_eq!(deepest.label, "s16");
    assert_eq!(deepest.kind, SiteMapNodeKind::Folder);
    assert_eq!((deepest.direct_count, deepest.subtree_count), (2, 2));
    let query = SiteMapQuery {
        selected_node_key: Some(deepest.key.clone()),
        ..Default::default()
    };
    let page = store
        .target_site_map_snapshot(&TargetScope::default(), &query, &[])
        .unwrap();
    assert_eq!(page.total_items, 2);
    assert_eq!(page.items.len(), 2);
}

#[test]
fn isolates_sibling_prefixes_api_vs_apiv2_and_port_prefixes() {
    let mut store = crate::store::ProjectStore::create(None).unwrap();
    for (url, method) in [
        ("https://example.com/api", "GET"),
        ("https://example.com/api/users", "GET"),
        ("https://example.com/apiv2", "GET"),
        ("https://example.com/apiv2/users", "GET"),
        ("http://example.com/root", "GET"),
        ("http://example.com:8080/root", "GET"),
    ] {
        store
            .put(&live_flow(
                url,
                method,
                Some(200),
                TrafficSource::Proxy,
                FlowState::Complete,
            ))
            .unwrap();
    }
    let mut query = SiteMapQuery {
        selected_node_key: Some("https://example.com/api/".into()),
        ..Default::default()
    };
    let api_page = store
        .target_site_map_snapshot(&TargetScope::default(), &query, &[])
        .unwrap();
    assert_eq!(api_page.total_items, 2);
    assert!(api_page.items.iter().all(|row| match row {
        crate::HistoryRow::Live(summary) => {
            summary.url.contains("example.com/api") && !summary.url.contains("example.com/apiv2")
        }
        crate::HistoryRow::ExtProxy(_) => false,
    }));
    query.selected_node_key = Some("http://example.com:80".into());
    let port_page = store
        .target_site_map_snapshot(&TargetScope::default(), &query, &[])
        .unwrap();
    assert_eq!(port_page.total_items, 1);
    assert!(!port_page.items.iter().any(|row| matches!(
        row,
        crate::HistoryRow::Live(summary) if summary.url.contains(":8080")
    )));
}
