use std::collections::HashSet;

use super::active::{build_scanner_flow, maybe_unpack_response, now_unix_ms, send_audit_signal};
use super::*;
use crate::audit_engine::discover_same_origin_links_and_forms;

const MAX_CRAWLED_ACTIVE_TARGETS: usize = 8;

pub(super) struct ActiveTarget {
    pub(super) request: HttpRequest,
    pub(super) base_response: Arc<HttpResponse>,
    pub(super) points: Vec<InsertionPoint>,
}

pub(super) struct CrawlStageOutcome {
    pub(super) targets: Vec<ActiveTarget>,
    pub(super) cancelled: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn collect_active_targets(
    project: ProjectKey,
    task_id: Uuid,
    base_req: &HttpRequest,
    base_resp: &HttpResponse,
    base_points: Vec<InsertionPoint>,
    mode: AuditTaskMode,
    crawl_same_origin: bool,
    scope: &TargetScope,
    config: &SendConfig,
    unpack_compressed_responses: bool,
    requests_sent: &mut u64,
    cancel: &CancellationToken,
    signal_tx: &mpsc::Sender<AuditSignal>,
) -> CrawlStageOutcome {
    let mut seen: HashSet<(String, String, Vec<u8>)> = HashSet::new();
    seen.insert((
        base_req.method.clone(),
        base_req.url.clone(),
        base_req.body.clone(),
    ));

    let mut targets = Vec::new();
    if mode == AuditTaskMode::PassiveAndActive && !base_points.is_empty() {
        targets.push(ActiveTarget {
            request: base_req.clone(),
            base_response: Arc::new(base_resp.clone()),
            points: base_points,
        });
    }

    if !crawl_same_origin {
        return CrawlStageOutcome {
            targets,
            cancelled: cancel.is_cancelled(),
        };
    }

    let mut crawl_cfg = config.clone();
    crawl_cfg.http.raw_http1_override = None;
    crawl_cfg.http.update_content_length = true;
    let crawl_bounds = CrawlConfig {
        max_depth: 2,
        max_requests: 16,
    };

    let mut discovered_endpoints =
        discover_same_origin_links_and_forms(&base_req.url, base_resp, scope);
    for ep in &mut discovered_endpoints {
        crate::audit_engine::discovery::apply_same_origin_context_headers(
            &mut ep.request,
            &base_req.headers,
        );
    }
    let mut crawled_flows = Vec::new();

    match crate::audit_engine::crawl_in_scope_endpoints_with_headers(
        &base_req.url,
        &base_req.headers,
        scope,
        &crawl_bounds,
        &crawl_cfg,
        cancel,
    )
    .await
    {
        Ok(mut report) => {
            let mut remaining_findings = report.findings;
            for flow in &report.flows {
                *requests_sent = requests_sent.saturating_add(1);
                let fid = flow.summary.id;
                let mut flow_findings = Vec::new();
                let mut rest = Vec::new();
                for finding in remaining_findings {
                    if finding.flow_id == Some(fid) {
                        flow_findings.push(finding);
                    } else {
                        rest.push(finding);
                    }
                }
                remaining_findings = rest;
                let _ = send_audit_signal(
                    signal_tx,
                    cancel,
                    AuditSignal {
                        project,
                        task_id,
                        kind: AuditSignalKind::Step {
                            requests_sent: *requests_sent,
                            flow: Some(Box::new(flow.clone())),
                            findings: flow_findings,
                        },
                    },
                )
                .await;
            }
            if !remaining_findings.is_empty() {
                let _ = send_audit_signal(
                    signal_tx,
                    cancel,
                    AuditSignal {
                        project,
                        task_id,
                        kind: AuditSignalKind::Step {
                            requests_sent: *requests_sent,
                            flow: None,
                            findings: remaining_findings,
                        },
                    },
                )
                .await;
            }
            if report.cancelled || cancel.is_cancelled() {
                return CrawlStageOutcome {
                    targets,
                    cancelled: true,
                };
            }
            crawled_flows = report.flows;
            report.endpoints.extend(discovered_endpoints);
            discovered_endpoints = report.endpoints;
        }
        Err(_) if cancel.is_cancelled() => {
            return CrawlStageOutcome {
                targets,
                cancelled: true,
            };
        }
        Err(_) => {}
    }

    if mode != AuditTaskMode::PassiveAndActive {
        return CrawlStageOutcome {
            targets,
            cancelled: cancel.is_cancelled(),
        };
    }

    let mut crawled_active_count = 0usize;
    for ep in discovered_endpoints {
        if crawled_active_count >= MAX_CRAWLED_ACTIVE_TARGETS {
            break;
        }
        if cancel.is_cancelled() {
            return CrawlStageOutcome {
                targets,
                cancelled: true,
            };
        }
        let key = (
            ep.request.method.clone(),
            ep.request.url.clone(),
            ep.request.body.clone(),
        );
        if !seen.insert(key) {
            continue;
        }
        let pts = extract_insertion_points(&ep.request);
        if pts.is_empty() {
            continue;
        }
        let existing_resp = crawled_flows
            .iter()
            .find(|f| {
                f.request.method == ep.request.method
                    && f.request.url == ep.request.url
                    && f.request.body == ep.request.body
            })
            .and_then(|f| f.response.clone());

        let ep_base = if let Some(resp) = existing_resp {
            maybe_unpack_response(
                &ep.request,
                &crawl_cfg,
                unpack_compressed_responses,
                resp,
                cancel,
            )
            .await
        } else {
            let started_ms = now_unix_ms();
            let timer = Instant::now();
            let send_res = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return CrawlStageOutcome {
                        targets,
                        cancelled: true,
                    };
                }
                res = send(ep.request.clone(), crawl_cfg.clone(), cancel.child_token()) => res,
            };
            let raw_resp = match send_res {
                Ok(resp) => resp,
                Err(_) if cancel.is_cancelled() => {
                    return CrawlStageOutcome {
                        targets,
                        cancelled: true,
                    };
                }
                Err(_) => {
                    *requests_sent = requests_sent.saturating_add(1);
                    continue;
                }
            };
            *requests_sent = requests_sent.saturating_add(1);
            let elapsed = timer.elapsed().as_millis().min(u64::MAX as u128) as u64;
            let unpacked = maybe_unpack_response(
                &ep.request,
                &crawl_cfg,
                unpack_compressed_responses,
                raw_resp,
                cancel,
            )
            .await;
            let flow_id = Uuid::new_v4();
            let passive_hits =
                inspect_passive(Some(flow_id), &ep.request, &unpacked, now_unix_ms());
            let flow = build_scanner_flow(
                flow_id,
                ep.request.clone(),
                unpacked.clone(),
                started_ms,
                elapsed,
            );
            let _ = send_audit_signal(
                signal_tx,
                cancel,
                AuditSignal {
                    project,
                    task_id,
                    kind: AuditSignalKind::Step {
                        requests_sent: *requests_sent,
                        flow: Some(Box::new(flow)),
                        findings: passive_hits,
                    },
                },
            )
            .await;
            unpacked
        };

        crawled_active_count += 1;
        targets.push(ActiveTarget {
            request: ep.request,
            base_response: Arc::new(ep_base),
            points: pts,
        });
    }

    CrawlStageOutcome {
        targets,
        cancelled: cancel.is_cancelled(),
    }
}
