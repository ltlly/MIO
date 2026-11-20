use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use mio_proxy::SendConfig;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{Header, HttpResponse, HttpVersion, ScopeRule, TargetScope, Uuid};

fn resp(status: u16, ct: &str, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: ct.as_bytes().to_vec(),
        }],
        body: body.as_bytes().to_vec(),
        trailers: Vec::new(),
    }
}

#[test]
fn discover_html_and_json_filters_schemes_cross_origin_fragments_and_scope() {
    let html = r##"
        <html>
        <head>
            <link rel="stylesheet" href="/static/site.css#v1">
            <script src="/js/app.js"></script>
            <script>const ignored = "<a href='/fake'>x</a>";</script>
            <!-- <a href="/commented-out">nope</a> -->
        </head>
        <body>
            <a href="/docs?page=1&amp;tab=overview#top">Docs</a>
            <a href="javascript:void(0)">JS</a>
            <a href="mailto:ops@example.com">Mail</a>
            <a href="data:text/plain;base64,YWJj">Data</a>
            <a href="#section-only">InPage</a>
            <a href="https://other.example.com/leak">CrossOrigin</a>
            <a href="/admin/excluded">Excluded</a>
            <form action="/api/search#frag" method="get">
                <input type="text" name="q" value="status">
                <input type="hidden" name="lang" value="zh">
            </form>
            <form action="/api/preferences" method="POST">
                <input name="theme" value="light">
                <textarea name="note">benign &amp; clean</textarea>
                <input type="file" name="upload">
            </form>
        </body>
        </html>
    "##;

    let scope = TargetScope {
        include: vec![ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://portal.example.com".into(),
            include_subdomains: false,
        }],
        exclude: vec![ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://portal.example.com/admin".into(),
            include_subdomains: false,
        }],
        ..TargetScope::default()
    };

    let endpoints = discover_same_origin_links_and_forms(
        "https://portal.example.com/home/index.html#hero",
        &resp(200, "text/html; charset=utf-8", html),
        &scope,
    );
    let summary: Vec<(&str, &str, DiscoverySource)> = endpoints
        .iter()
        .map(|e| (e.method.as_str(), e.url.as_str(), e.source))
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "GET",
                "https://portal.example.com/static/site.css",
                DiscoverySource::LinkHref
            ),
            (
                "GET",
                "https://portal.example.com/js/app.js",
                DiscoverySource::ScriptSrc
            ),
            (
                "GET",
                "https://portal.example.com/docs?page=1&tab=overview",
                DiscoverySource::AnchorHref
            ),
            (
                "GET",
                "https://portal.example.com/api/search?q=status&lang=zh",
                DiscoverySource::FormSubmission
            ),
            (
                "POST",
                "https://portal.example.com/api/preferences",
                DiscoverySource::FormSubmission
            ),
        ]
    );
    let post_ep = endpoints.iter().find(|e| e.method == "POST").unwrap();
    assert_eq!(
        std::str::from_utf8(&post_ep.request.body).unwrap(),
        "theme=light&note=benign+%26+clean"
    );

    let json_body = r##"{
        "next": "/v1/items?cursor=2#ignore",
        "self": "https://portal.example.com/v1/items",
        "external": "https://cdn.other.test/a.json",
        "excluded": "/admin/audit",
        "label": "plain words / not url"
    }"##;
    let json_eps = discover_same_origin_links_and_forms(
        "https://portal.example.com/v1/root",
        &resp(200, "application/json", json_body),
        &scope,
    );
    let json_urls: Vec<&str> = json_eps.iter().map(|e| e.url.as_str()).collect();
    assert_eq!(
        json_urls,
        vec![
            "https://portal.example.com/v1/items?cursor=2",
            "https://portal.example.com/v1/items",
        ]
    );

    let mut many_links = String::from("<html><body>");
    for idx in 0..300 {
        many_links.push_str(&format!("<a href=\"/p/{idx}\">{idx}</a>"));
    }
    many_links.push_str("</body></html>");
    let capped = discover_same_origin_links_and_forms(
        "https://portal.example.com/",
        &resp(200, "text/html", &many_links),
        &TargetScope::default(),
    );
    assert_eq!(capped.len(), MAX_DISCOVERED_PER_PAGE);
}

#[tokio::test]
async fn crawl_in_scope_enforces_loopback_or_scope_bounds_and_passive_checks() {
    let cancel = CancellationToken::new();
    assert!(
        crawl_in_scope_endpoints(
            "https://unscoped.example.com/",
            &TargetScope::default(),
            &CrawlConfig::default(),
            &SendConfig::default(),
            &cancel,
        )
        .await
        .is_err()
    );
    assert!(
        crawl_in_scope_endpoints(
            "http://127.0.0.1:1/",
            &TargetScope::default(),
            &CrawlConfig {
                max_depth: 5,
                max_requests: 10
            },
            &SendConfig::default(),
            &cancel,
        )
        .await
        .is_err()
    );
    assert!(
        crawl_in_scope_endpoints(
            "http://127.0.0.1:1/",
            &TargetScope::default(),
            &CrawlConfig {
                max_depth: 2,
                max_requests: 65
            },
            &SendConfig::default(),
            &cancel,
        )
        .await
        .is_err()
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let post_hits = Arc::new(AtomicUsize::new(0));
    let post_hits_srv = post_hits.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let hits = post_hits_srv.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_str.lines().next().unwrap_or_default();
                if first_line.starts_with("POST ") {
                    hits.fetch_add(1, Ordering::SeqCst);
                }
                let body = if first_line.starts_with("GET / ") {
                    "<html><body><a href=\"/dir\">Dir</a><form action=\"/save\" method=\"post\"><input name=\"k\" value=\"v\"></form></body></html>"
                } else if first_line.starts_with("GET /dir ") {
                    "<html><body><a href=\"/leaf\">Leaf</a></body></html>"
                } else {
                    "<html><body>Leaf</body></html>"
                };
                let raw_resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: sid=1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(raw_resp.as_bytes()).await;
            });
        }
    });

    let seed = format!("http://127.0.0.1:{}/", addr.port());
    let report = crawl_in_scope_endpoints(
        &seed,
        &TargetScope::default(),
        &CrawlConfig {
            max_depth: 2,
            max_requests: 8,
        },
        &SendConfig::default(),
        &cancel,
    )
    .await
    .unwrap();
    server.abort();

    assert_eq!(post_hits.load(Ordering::SeqCst), 0);
    assert_eq!(report.requests_sent, 3);
    assert_eq!(report.flows.len(), 3);
    assert!(!report.findings.is_empty());
    assert!(
        report
            .endpoints
            .iter()
            .any(|e| e.method == "POST" && e.url.ends_with("/save"))
    );
    assert!(
        report
            .endpoints
            .iter()
            .any(|e| e.method == "GET" && e.url.ends_with("/leaf"))
    );
}
