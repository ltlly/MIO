use url::Url;

use super::command::next_audit_revision;
use super::*;
use crate::{
    AuditPhase, CollaboratorEvent, IssueConfidence, OastInteraction, OastProtocol,
    target_issues::{IssueSeverity, issue_definitions},
};

const OAST_SSRF_ISSUE_ID: &str = "0x00100a00";
const OAST_SSRF_TITLE: &str = "服务端请求伪造 / OAST 带外交互 (SSRF via Collaborator)";
const OAST_SSRF_PARAMETER: &str = "OAST (Collaborator)";

impl AuditService {
    pub(super) fn register_oast_probe(&mut self, record: OastProbeRecord) {
        if record.token.trim().is_empty() {
            return;
        }
        while self.oast_probes.len() >= MAX_OAST_PROBES {
            self.oast_probes.remove(0);
        }
        self.oast_probes.push(record);
    }

    pub(super) fn bind_probe_seed_flow(
        &mut self,
        project: ProjectKey,
        task_id: Uuid,
        flow_id: FlowId,
    ) {
        for probe in &mut self.oast_probes {
            if probe.project == project && probe.task_id == task_id && probe.seed_flow_id.is_none()
            {
                probe.seed_flow_id = Some(flow_id);
            }
        }
    }

    pub(super) fn has_oast_probe_for_task(&self, project: ProjectKey, task_id: Uuid) -> bool {
        self.oast_probes
            .iter()
            .any(|p| p.project == project && p.task_id == task_id)
    }

    pub(super) fn matching_oast_probes(
        &self,
        project: ProjectKey,
        interaction: &OastInteraction,
    ) -> Vec<OastProbeRecord> {
        self.oast_probes
            .iter()
            .filter(|probe| {
                probe.project == project && interaction_matches_probe(interaction, &probe.token)
            })
            .cloned()
            .collect()
    }
}

fn protocol_label(protocol: OastProtocol) -> &'static str {
    match protocol {
        OastProtocol::Http => "HTTP",
        OastProtocol::Dns => "DNS",
    }
}

fn interaction_matches_probe(interaction: &OastInteraction, probe_token: &str) -> bool {
    let needle = probe_token.trim();
    if needle.is_empty() {
        return false;
    }
    if interaction
        .token
        .as_deref()
        .is_some_and(|tok| tok.trim().eq_ignore_ascii_case(needle))
    {
        return true;
    }
    let needle_lower = needle.to_ascii_lowercase();
    if interaction
        .summary
        .to_ascii_lowercase()
        .contains(&needle_lower)
    {
        return true;
    }
    String::from_utf8_lossy(&interaction.raw_request)
        .to_ascii_lowercase()
        .contains(&needle_lower)
}

fn build_oast_finding(probe: &OastProbeRecord, interaction: &OastInteraction) -> AuditFinding {
    let def = issue_definitions()
        .iter()
        .find(|d| d.id == OAST_SSRF_ISSUE_ID);
    let sev = IssueSeverity::High;
    let host = Url::parse(&probe.target_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let proto = protocol_label(interaction.protocol);
    let evidence: String = format!(
        "{proto} 回调自 {} ({})",
        interaction.peer_addr, interaction.summary
    )
    .chars()
    .take(512)
    .collect();
    let detected_unix_ms = if interaction.observed_unix_ms > 0 {
        interaction.observed_unix_ms
    } else {
        now_unix_ms()
    };
    AuditFinding {
        id: Uuid::new_v4(),
        issue_id: OAST_SSRF_ISSUE_ID.to_owned(),
        title: OAST_SSRF_TITLE.to_owned(),
        severity_label: sev.label().to_owned(),
        severity_rank: sev.rank(),
        confidence: IssueConfidence::Certain,
        phase: AuditPhase::Active,
        host,
        url: probe.target_url.clone(),
        parameter: Some(OAST_SSRF_PARAMETER.to_owned()),
        flow_id: probe.seed_flow_id,
        summary: format!(
            "主动扫描注入 Collaborator 探针 ({}) 后收到带外 {proto} 回调。",
            probe.callback_url
        ),
        evidence,
        request_highlights: Vec::new(),
        response_highlights: Vec::new(),
        remediation: def
            .map_or(
                "将远程取回能力与用户输入解耦，阻断未授权带外请求。",
                |d| d.remediation,
            )
            .to_owned(),
        detected_unix_ms,
    }
}

impl State {
    pub(in crate::service) async fn on_collaborator_event_for_audit(
        &mut self,
        event: &CollaboratorEvent,
    ) {
        let CollaboratorEvent::InteractionObserved {
            project,
            interaction,
        } = event
        else {
            return;
        };
        if *project != self.project || self.unsaved_count() != 0 || self.quiesced {
            return;
        }
        let matched = self.audit.matching_oast_probes(*project, interaction);
        if matched.is_empty() {
            return;
        }
        let Ok(mut state) = self.store.audit().read_state() else {
            return;
        };
        let mut any_added = false;
        for probe in matched {
            let finding = build_oast_finding(&probe, interaction);
            let added = merge_audit_findings(&mut state.findings, vec![finding]);
            if !added.is_empty() {
                any_added = true;
                if let Some(task) = state.tasks.iter_mut().find(|t| t.id == probe.task_id) {
                    task.issues_found = task.issues_found.saturating_add(added.len() as u64);
                }
            }
        }
        if !any_added {
            return;
        }
        state.revision = next_audit_revision(state.revision);
        if let Err(error) = self.write_audit_snapshot(&state) {
            self.error("audit", error).await;
            return;
        }
        self.emit_audit_state(self.project, state).await;
    }

    pub(super) async fn finish_task_oast_check(&mut self, project: ProjectKey, task_id: Uuid) {
        if project != self.project || !self.audit.has_oast_probe_for_task(project, task_id) {
            return;
        }
        tokio::task::yield_now().await;
        let observed = self.collaborator.drain_local_rx();
        for interaction in observed {
            let event = CollaboratorEvent::InteractionObserved {
                project,
                interaction: Box::new(interaction),
            };
            self.on_collaborator_event_for_audit(&event).await;
            self.emit(Event::Collaborator(event)).await;
        }
    }
}
