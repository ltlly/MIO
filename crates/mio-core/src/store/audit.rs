use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, TransactionBehavior, params};
use tokio_util::sync::CancellationToken;

use super::{ProjectStore, metadata};
use crate::{
    AuditFinding, AuditPhase, AuditStateSnapshot, AuditTaskState, MAX_AUDIT_FINDINGS,
    MAX_AUDIT_STATE_BYTES, MAX_AUDIT_TASKS,
};

pub(super) const KEY: &str = "audit_state";

type FindingKey = (String, String, String, Option<String>, AuditPhase);

fn finding_key(finding: &AuditFinding) -> FindingKey {
    (
        finding.issue_id.clone(),
        finding.host.clone(),
        finding.url.clone(),
        finding.parameter.clone(),
        finding.phase,
    )
}

pub(crate) fn merge_audit_findings(
    existing: &mut Vec<AuditFinding>,
    incoming: Vec<AuditFinding>,
) -> Vec<AuditFinding> {
    let mut seen: HashSet<FindingKey> = existing.iter().map(finding_key).collect();
    let mut added = Vec::new();
    for finding in incoming {
        if existing.len() >= MAX_AUDIT_FINDINGS {
            break;
        }
        if seen.insert(finding_key(&finding)) {
            existing.push(finding.clone());
            added.push(finding);
        }
    }
    added
}

fn read(connection: &Connection) -> Result<AuditStateSnapshot> {
    let mut query = connection.prepare(
        "SELECT key, typeof(value),
                CASE WHEN typeof(value)='blob' AND length(value)<=?2 THEN value END
         FROM main.metadata WHERE key=?1",
    )?;
    let mut rows = query.query(params![KEY, i64::try_from(MAX_AUDIT_STATE_BYTES)?])?;
    let Some(row) = rows.next()? else {
        return Ok(AuditStateSnapshot::default());
    };
    ensure!(row.get::<_, String>(0)? == KEY, "审计状态目录键编码无效");
    ensure!(
        row.get::<_, String>(1)? == "blob",
        "审计状态必须为二进制 JSON"
    );
    let bytes = row
        .get_ref(2)?
        .as_blob()
        .context("审计状态类型无效或超过编码读取限额")?;
    let state: AuditStateSnapshot =
        serde_json::from_slice(bytes).context("审计状态 JSON 损坏或包含无效字段")?;
    ensure!(rows.next()?.is_none(), "审计状态记录重复");
    ensure!(
        state.tasks.len() <= MAX_AUDIT_TASKS,
        "审计任务数量超过 {MAX_AUDIT_TASKS} 个上限"
    );
    ensure!(
        state.findings.len() <= MAX_AUDIT_FINDINGS,
        "审计告警数量超过 {MAX_AUDIT_FINDINGS} 条上限"
    );
    Ok(state)
}

impl<S: std::ops::Deref<Target = ProjectStore>> super::AuditRepo<S> {
    pub(crate) fn read_state(&self) -> Result<AuditStateSnapshot> {
        let tx = self.0.connection.unchecked_transaction()?;
        ensure!(
            metadata::project_id(&tx)? == self.0.id,
            "审计状态项目归属已改变"
        );
        read(&tx)
    }
}

impl super::AuditRepo<&mut ProjectStore> {
    pub(crate) fn write_state(
        &mut self,
        next: &AuditStateSnapshot,
        cancel: &CancellationToken,
    ) -> Result<()> {
        ensure!(!cancel.is_cancelled(), "审计状态更新已取消");
        ensure!(
            next.tasks.len() <= MAX_AUDIT_TASKS,
            "审计任务数量超过 {MAX_AUDIT_TASKS} 个上限"
        );
        ensure!(
            next.findings.len() <= MAX_AUDIT_FINDINGS,
            "审计告警数量超过 {MAX_AUDIT_FINDINGS} 条上限"
        );
        let encoded = serde_json::to_vec(next)?;
        ensure!(
            encoded.len() <= MAX_AUDIT_STATE_BYTES,
            "审计状态超过编码限额"
        );
        let tx = self
            .0
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let schema: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(schema == super::SCHEMA, "项目版本已改变；未写入审计状态");
        super::ensure_schema_validated_in_tx(&self.0.cache, &tx)?;
        ensure!(
            metadata::project_id(&tx)? == self.0.id,
            "审计状态项目归属已改变"
        );
        let before_tc = super::query_total_changes(&tx)?;
        ensure!(!cancel.is_cancelled(), "审计状态更新已取消");
        tx.execute(
            "INSERT OR REPLACE INTO main.metadata(key, value) VALUES(?1, ?2)",
            params![KEY, encoded],
        )?;
        ensure!(!cancel.is_cancelled(), "审计状态更新已取消");
        tx.commit()?;
        self.0.advance_workspace_cache_changes(before_tc)?;
        Ok(())
    }

    pub(crate) fn recover_tasks_on_open(
        &mut self,
        pause_automated_tasks: bool,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let mut state = self.0.audit().read_state()?;
        let mut changed = false;
        for task in &mut state.tasks {
            if matches!(task.state, AuditTaskState::Running | AuditTaskState::Paused) {
                task.state = AuditTaskState::Cancelled;
                changed = true;
            }
        }
        if pause_automated_tasks
            && (state.live_passive_enabled || state.live_passive_crawl_enabled)
        {
            state.live_passive_enabled = false;
            state.live_passive_crawl_enabled = false;
            changed = true;
        }
        if changed {
            state.revision += 1;
            self.write_state(&state, cancel)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuditTaskMode, AuditTaskSummary, IssueConfidence, Uuid};

    fn sample_finding(issue_id: &str, param: Option<&str>, phase: AuditPhase) -> AuditFinding {
        AuditFinding {
            id: Uuid::new_v4(),
            issue_id: issue_id.to_owned(),
            title: "测试告警".to_owned(),
            severity_label: "高".to_owned(),
            severity_rank: 3,
            confidence: IssueConfidence::Firm,
            phase,
            host: "127.0.0.1".to_owned(),
            url: "http://127.0.0.1/vuln?q=1".to_owned(),
            parameter: param.map(str::to_owned),
            flow_id: None,
            summary: "摘要".to_owned(),
            evidence: "证据".to_owned(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "修复建议".to_owned(),
            detected_unix_ms: 1_700_000_000_000,
        }
    }

    fn sample_task(state: AuditTaskState) -> AuditTaskSummary {
        AuditTaskSummary {
            id: Uuid::new_v4(),
            title: "审计任务".to_owned(),
            target_url: "http://127.0.0.1/vuln?q=1".to_owned(),
            mode: AuditTaskMode::PassiveAndActive,
            state,
            requests_sent: 2,
            total_planned: 5,
            issues_found: 1,
            started_unix_ms: 1_700_000_000_000,
            error: None,
        }
    }

    #[test]
    fn empty_project_reads_default_audit_state_and_rejects_corrupt_type() {
        let store = ProjectStore::create(None).unwrap();
        let state = store.read_audit_state().unwrap();
        assert_eq!(state, AuditStateSnapshot::default());
        assert!(state.live_passive_enabled);

        store
            .connection
            .execute(
                "INSERT INTO main.metadata(key, value) VALUES(?1, 'text')",
                [KEY],
            )
            .unwrap();
        assert!(store.read_audit_state().is_err());
    }

    #[test]
    fn merge_deduplicates_findings_and_write_enforces_upper_bounds() {
        let mut store = ProjectStore::create(None).unwrap();
        let cancel = CancellationToken::new();
        let mut findings = Vec::new();
        let f1 = sample_finding("0x00200300", Some("q"), AuditPhase::Active);
        let dup = sample_finding("0x00200300", Some("q"), AuditPhase::Active);
        let diff_param = sample_finding("0x00200300", Some("id"), AuditPhase::Active);
        let added = merge_audit_findings(&mut findings, vec![f1, dup, diff_param]);
        assert_eq!(added.len(), 2);
        assert_eq!(findings.len(), 2);

        let valid = AuditStateSnapshot {
            revision: 1,
            live_passive_enabled: true,
            live_passive_crawl_enabled: true,
            live_passive_suite_scope_only: false,
            tasks: vec![sample_task(AuditTaskState::Completed)],
            findings,
        };
        store.write_audit_state(&valid, &cancel).unwrap();
        assert_eq!(store.read_audit_state().unwrap(), valid);

        let mut too_many_tasks = valid.clone();
        too_many_tasks.tasks = (0..=MAX_AUDIT_TASKS)
            .map(|_| sample_task(AuditTaskState::Completed))
            .collect();
        assert!(store.write_audit_state(&too_many_tasks, &cancel).is_err());
    }

    #[test]
    fn reopen_recovery_marks_running_tasks_cancelled_and_applies_pause_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit-reopen.mio");
        let cancel = CancellationToken::new();
        let running_id;
        let completed_id;
        {
            let mut store = ProjectStore::create(Some(&path)).unwrap();
            let running = sample_task(AuditTaskState::Running);
            let completed = sample_task(AuditTaskState::Completed);
            running_id = running.id;
            completed_id = completed.id;
            store
                .write_audit_state(
                    &AuditStateSnapshot {
                        revision: 3,
                        live_passive_enabled: true,
                        live_passive_crawl_enabled: true,
                        live_passive_suite_scope_only: false,
                        tasks: vec![running, completed],
                        findings: Vec::new(),
                    },
                    &cancel,
                )
                .unwrap();
            store.checkpoint().unwrap();
            store.close().unwrap();
        }
        let mut reopened = ProjectStore::open(&path).unwrap();
        reopened.recover_audit_tasks_on_open(true, &cancel).unwrap();
        let recovered = reopened.read_audit_state().unwrap();
        assert_eq!(recovered.revision, 4);
        assert!(!recovered.live_passive_enabled);
        assert!(!recovered.live_passive_crawl_enabled);
        assert_eq!(
            recovered
                .tasks
                .iter()
                .find(|t| t.id == running_id)
                .unwrap()
                .state,
            AuditTaskState::Cancelled
        );
        assert_eq!(
            recovered
                .tasks
                .iter()
                .find(|t| t.id == completed_id)
                .unwrap()
                .state,
            AuditTaskState::Completed
        );
    }
}
