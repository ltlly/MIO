use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tokio_util::sync::CancellationToken;

use super::super as intruder_mod;
use super::{OWNERSHIP_BYTES, PAGE_BYTES, ROW_ACCOUNTING_BYTES, store, strict_json};
use crate::intruder::{MAX_PAYLOAD_SETS, payload_positions, payload_request_count, validate_positions};
use crate::{
    Draft, FlowId, FlowRecord, FlowState, FlowSummary, IntruderBaseline, IntruderPayloadPreview,
    IntruderResultOutcome, IntruderResultSummary, IntruderResultViewPage, IntruderResultViewQuery,
    IntruderResultViewRow, IntruderRetention, IntruderRunState, IntruderRunSummary, IntruderSpec,
    OriginalMessages, TabId, Uuid, http::MAX_BODY,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store::intruder) struct SessionFlow {
    pub summary: FlowSummary,
    pub summary_json: Vec<u8>,
    pub messages: BTreeMap<u8, (Vec<u8>, Vec<u8>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store::intruder) struct SessionResultRow {
    pub ordinal: u64,
    pub flow: Option<FlowId>,
    pub error: Option<String>,
    pub notes: Option<Vec<u8>>,
    pub notes_revision: u64,
    pub highlight: Option<u8>,
    pub highlight_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store::intruder) struct SessionData {
    pub run: IntruderRunSummary,
    pub spec_meta: Vec<u8>,
    pub template: Vec<u8>,
    pub payloads: Vec<Vec<Vec<u8>>>,
    pub results: BTreeMap<u64, SessionResultRow>,
    pub flows: BTreeMap<FlowId, SessionFlow>,
}

#[cfg(test)]
pub(in crate::store) type SessionWriteHook =
    std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

fn run_error_len(state: &IntruderRunState) -> usize {
    match state {
        IntruderRunState::Failed(message) => message.len(),
        _ => 0,
    }
}

fn checked_add_bytes(acc: u64, delta: usize) -> Result<u64> {
    acc.checked_add(u64::try_from(delta)?)
        .context("Intruder Session 逻辑数据计量溢出")
}

impl SessionData {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_spec(
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started: i64,
        total: u64,
        resource_pool: Option<Uuid>,
        attack_settings: crate::IntruderAttackSettings,
        payload_configs: Vec<crate::IntruderPayloadSetConfig>,
    ) -> Result<Self> {
        let spec_meta = serde_json::to_vec(&intruder_mod::SpecMeta {
            draft_revision: spec.draft.revision,
            target: spec.draft.target.clone(),
            version: spec.draft.version,
            attack_type: spec.attack_type,
            positions: spec.positions.clone(),
            max_concurrent_requests: spec.max_concurrent_requests,
            request_timeout_ms: spec.request_timeout_ms,
            update_host_header: spec.update_host_header,
            set_count: spec.payload_sets.len(),
            save_to_project: spec.save_to_project,
            baseline_request: spec.baseline_request,
            resource_pool: spec.resource_pool.clone(),
            attack_settings,
            payload_configs,
        })?;
        ensure!(
            spec_meta.len() <= intruder_mod::MAX_SPEC_META,
            "Intruder 配置元数据超过编码限额"
        );
        let run = IntruderRunSummary {
            id,
            tab_id,
            revision: 1,
            spec_revision: spec.revision,
            total,
            dispatched: 0,
            completed: 0,
            state: IntruderRunState::Running,
            started_unix_ms: started,
            retention: IntruderRetention::Session,
            title: String::new(),
            title_revision: 0,
            baseline: spec.baseline_request.then_some(IntruderBaseline {
                ordinal: 0,
                completed: false,
            }),
            resource_pool,
            resource_pool_revision: u64::from(resource_pool.is_some()),
        };
        Ok(Self {
            run,
            spec_meta,
            template: spec.draft.raw.clone(),
            payloads: spec.payload_sets.clone(),
            results: BTreeMap::new(),
            flows: BTreeMap::new(),
        })
    }

    pub(super) fn load_from_project(
        tx: &Transaction<'_>,
        id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let mut run = intruder_mod::read_run(tx, id)?;
        run.retention = IntruderRetention::Session;
        let (spec_meta, template): (Vec<u8>, Vec<u8>) = tx.query_row(
            "SELECT CASE WHEN typeof(spec_meta)='blob' AND length(spec_meta)<=?2 THEN spec_meta END,
                    CASE WHEN typeof(template)='blob' AND length(template)<=?3 THEN template END
             FROM main.intruder_runs WHERE id=?1",
            params![
                id.to_string(),
                intruder_mod::MAX_SPEC_META as i64,
                intruder_mod::MAX_REQUEST_BYTES as i64,
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let meta: intruder_mod::SpecMeta = strict_json(&spec_meta)?;
        ensure!(
            (1..=MAX_PAYLOAD_SETS).contains(&meta.set_count),
            "Intruder payload set 数量无效"
        );
        let mut payloads = vec![Vec::new(); meta.set_count];
        {
            let mut stmt = tx.prepare(
                "SELECT set_index, ordinal, typeof(value), length(value),
                        CASE WHEN typeof(value)='blob' AND length(value)<=?3 THEN value END
                 FROM main.intruder_payloads WHERE run=?1 ORDER BY set_index, ordinal LIMIT ?2",
            )?;
            let mut rows = stmt.query(params![
                id.to_string(),
                i64::try_from(crate::intruder::MAX_PAYLOADS + 1)?,
                intruder_mod::MAX_PAYLOAD_BYTES as i64,
            ])?;
            while let Some(row) = rows.next()? {
                ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
                let set: usize = row.get(0)?;
                let ordinal: usize = row.get(1)?;
                ensure!(
                    set < payloads.len() && ordinal == payloads[set].len(),
                    "Intruder payload 位置缺失、重复或越界"
                );
                ensure!(
                    row.get::<_, String>(2)? == "blob",
                    "Intruder payload 不是二进制"
                );
                let len: usize = row.get(3)?;
                let value = row
                    .get_ref(4)?
                    .as_blob()
                    .context("Intruder payload 类型或长度无效")?;
                ensure!(value.len() == len, "Intruder payload 长度不一致");
                payloads[set].push(value.to_vec());
            }
        }
        let mut flows = BTreeMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id, revision, started, url, method, state, summary
                 FROM main.flows
                 WHERE id IN (SELECT flow FROM main.intruder_results WHERE run=?1)
                 ORDER BY id",
            )?;
            let mut rows = stmt.query([id.to_string()])?;
            while let Some(row) = rows.next()? {
                ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
                let raw_id: String = row.get(0)?;
                let flow_id = Uuid::parse_str(&raw_id)?;
                ensure!(raw_id == flow_id.to_string(), "Intruder flow ID 编码无效");
                let summary_json: Vec<u8> = row.get(6)?;
                let summary: FlowSummary = strict_json(&summary_json)?;
                ensure!(
                    summary.id == flow_id
                        && summary.revision == row.get::<_, u64>(1)?
                        && summary.started_unix_ms == row.get::<_, i64>(2)?
                        && summary.url == row.get::<_, String>(3)?
                        && summary.method == row.get::<_, String>(4)?
                        && store::state_code(&summary.state) == row.get::<_, String>(5)?,
                    "Intruder 流量索引与完整摘要不一致"
                );
                flows.insert(
                    flow_id,
                    SessionFlow {
                        summary,
                        summary_json,
                        messages: BTreeMap::new(),
                    },
                );
            }
        }
        {
            let mut stmt = tx.prepare(
                "SELECT flow, side, head, body FROM main.messages
                 WHERE flow IN (SELECT flow FROM main.intruder_results WHERE run=?1)
                 ORDER BY flow, side",
            )?;
            let mut rows = stmt.query([id.to_string()])?;
            while let Some(row) = rows.next()? {
                ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
                let raw_id: String = row.get(0)?;
                let flow_id = Uuid::parse_str(&raw_id)?;
                let side: u8 = row.get(1)?;
                let head: Vec<u8> = row.get(2)?;
                let body: Vec<u8> = row.get(3)?;
                let entry = flows
                    .get_mut(&flow_id)
                    .context("Intruder 消息缺少对应 flow")?;
                ensure!(
                    entry.messages.insert(side, (head, body)).is_none(),
                    "Intruder 消息版本号重复"
                );
            }
        }
        let mut results = BTreeMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT ordinal, flow, error, notes, notes_revision, highlight, highlight_revision
                 FROM main.intruder_results WHERE run=?1 ORDER BY ordinal",
            )?;
            let mut rows = stmt.query([id.to_string()])?;
            while let Some(row) = rows.next()? {
                ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
                let ordinal: u64 = row.get(0)?;
                let flow: Option<String> = row.get(1)?;
                let flow = flow
                    .map(|raw| {
                        let parsed = Uuid::parse_str(&raw)?;
                        ensure!(raw == parsed.to_string(), "Intruder flow ID 编码无效");
                        Ok::<_, anyhow::Error>(parsed)
                    })
                    .transpose()?;
                let error: Option<String> = row.get(2)?;
                let notes: Option<Vec<u8>> = row.get(3)?;
                let notes_revision: u64 = row.get(4)?;
                let highlight: Option<u8> = row.get(5)?;
                let highlight_revision: u64 = row.get(6)?;
                ensure!(
                    results
                        .insert(
                            ordinal,
                            SessionResultRow {
                                ordinal,
                                flow,
                                error,
                                notes,
                                notes_revision,
                                highlight,
                                highlight_revision,
                            },
                        )
                        .is_none(),
                    "Intruder 结果序号重复"
                );
            }
        }
        Ok(Self {
            run,
            spec_meta,
            template,
            payloads,
            results,
            flows,
        })
    }

    pub(super) fn write_to_project(
        &self,
        tx: &Transaction<'_>,
        disk_flows: &BTreeSet<FlowId>,
        next_revision: u64,
        cancel: &CancellationToken,
        check_disk_flow: impl FnMut(FlowId) -> Result<()>,
    ) -> Result<()> {
        let mut check_disk_flow = check_disk_flow;
        let (state_code_num, run_error) = intruder_mod::mutations::state_parts(&self.run.state);
        tx.execute(
            "INSERT INTO main.intruder_runs(
                id, tab, revision, spec_revision, total, dispatched, completed, state, paused,
                error, started, title, title_revision, baseline_ordinal, pool_id, pool_revision,
                spec_meta, template
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![
                self.run.id.to_string(),
                self.run.tab_id.to_string(),
                next_revision,
                self.run.spec_revision,
                self.run.total,
                self.run.dispatched,
                self.run.completed,
                state_code_num,
                self.run.state == IntruderRunState::Paused,
                run_error,
                self.run.started_unix_ms,
                self.run.title,
                self.run.title_revision,
                self.run.baseline.map(|b| b.ordinal),
                self.run.resource_pool.map(|id| id.to_string()),
                self.run.resource_pool_revision,
                self.spec_meta.as_slice(),
                self.template.as_slice(),
            ],
        )?;
        {
            let mut insert_payload = tx.prepare(
                "INSERT INTO main.intruder_payloads(run, set_index, ordinal, value) VALUES(?1,?2,?3,?4)",
            )?;
            let run_str = self.run.id.to_string();
            for (set_index, set) in self.payloads.iter().enumerate() {
                for (ordinal, value) in set.iter().enumerate() {
                    ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
                    insert_payload.execute(params![
                        &run_str,
                        i64::try_from(set_index)?,
                        i64::try_from(ordinal)?,
                        value.as_slice(),
                    ])?;
                }
            }
        }
        {
            let mut insert_flow = tx.prepare(
                "INSERT INTO main.flows(
                    id, revision, started, url, method, state, summary,
                    source, status, response_bytes, elapsed_ms
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            )?;
            let mut insert_msg = tx.prepare(
                "INSERT INTO main.messages(flow, side, head, body) VALUES(?1,?2,?3,?4)",
            )?;
            for flow_id in self.flow_ids_in_order() {
                ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
                if super::has_flow(tx, flow_id)? {
                    ensure!(
                        disk_flows.contains(&flow_id),
                        "Intruder flow ID 被其它数据占用；RAM 保留"
                    );
                    check_disk_flow(flow_id)?;
                } else {
                    let orphan: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM main.messages WHERE flow=?1)
                         OR EXISTS(SELECT 1 FROM main.intruder_results WHERE flow=?1)
                         OR EXISTS(SELECT 1 FROM main.websocket_messages WHERE flow=?1)
                         OR EXISTS(SELECT 1 FROM main.websocket_connections WHERE flow=?1)
                         OR EXISTS(SELECT 1 FROM main.repeater_history WHERE flow=?1)",
                        [flow_id.to_string()],
                        |row| row.get(0),
                    )?;
                    ensure!(!orphan, "Intruder flow 存在无主引用；RAM 保留");
                    let flow = self
                        .flows
                        .get(&flow_id)
                        .context("Intruder flow 数据缺失；RAM 保留")?;
                    let flow_str = flow_id.to_string();
                    insert_flow.execute(params![
                        &flow_str,
                        flow.summary.revision,
                        flow.summary.started_unix_ms,
                        &flow.summary.url,
                        &flow.summary.method,
                        store::state_code(&flow.summary.state),
                        flow.summary_json.as_slice(),
                        store::messages::source_code(&flow.summary.source),
                        flow.summary.status.map(i64::from),
                        i64::try_from(flow.summary.response_bytes)?,
                        i64::try_from(flow.summary.elapsed_ms)?,
                    ])?;
                    for (&side, (head, body)) in &flow.messages {
                        ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
                        insert_msg.execute(params![
                            &flow_str,
                            side,
                            head.as_slice(),
                            body.as_slice(),
                        ])?;
                    }
                }
            }
        }
        {
            let mut insert_result = tx.prepare(
                "INSERT INTO main.intruder_results(
                    run, ordinal, flow, error, notes, notes_revision, highlight, highlight_revision
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            )?;
            let run_str = self.run.id.to_string();
            for row in self.results.values() {
                ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
                insert_result.execute(params![
                    &run_str,
                    row.ordinal,
                    row.flow.map(|id| id.to_string()),
                    row.error.as_deref(),
                    row.notes.as_deref(),
                    row.notes_revision,
                    row.highlight,
                    row.highlight_revision,
                ])?;
            }
        }
        Ok(())
    }

    pub(super) fn logical_bytes(&self) -> Result<u64> {
        let mut total = OWNERSHIP_BYTES;
        // Run row (18 columns: 2 TEXT + 10 non-null INTEGERs + 6 nullable/BLOBs)
        let mut run_row = ROW_ACCOUNTING_BYTES + 36 + 36 + 8 * 10;
        run_row = checked_add_bytes(run_row, run_error_len(&self.run.state))?;
        run_row = checked_add_bytes(run_row, self.run.title.len())?;
        if self.run.baseline.is_some() {
            run_row += 8;
        }
        if self.run.resource_pool.is_some() {
            run_row += 36;
        }
        run_row = checked_add_bytes(run_row, self.spec_meta.len())?;
        run_row = checked_add_bytes(run_row, self.template.len())?;
        total = total
            .checked_add(run_row)
            .context("Intruder Session 逻辑数据计量溢出")?;

        // Payload rows
        for set in &self.payloads {
            for value in set {
                let row = checked_add_bytes(ROW_ACCOUNTING_BYTES + 36 + 8 + 8, value.len())?;
                total = total
                    .checked_add(row)
                    .context("Intruder Session 逻辑数据计量溢出")?;
            }
        }

        // Flow and message rows
        for flow in self.flows.values() {
            let mut f_row = ROW_ACCOUNTING_BYTES + 36 + 8 + 8;
            f_row = checked_add_bytes(f_row, flow.summary.url.len())?;
            f_row = checked_add_bytes(f_row, flow.summary.method.len())?;
            f_row = checked_add_bytes(f_row, store::state_code(&flow.summary.state).len())?;
            f_row = checked_add_bytes(f_row, flow.summary_json.len())?;
            total = total
                .checked_add(f_row)
                .context("Intruder Session 逻辑数据计量溢出")?;

            for (head, body) in flow.messages.values() {
                let m_row = checked_add_bytes(
                    checked_add_bytes(ROW_ACCOUNTING_BYTES + 36 + 8, head.len())?,
                    body.len(),
                )?;
                total = total
                    .checked_add(m_row)
                    .context("Intruder Session 逻辑数据计量溢出")?;
            }
        }

        // Result rows
        for res in self.results.values() {
            let mut r_row = ROW_ACCOUNTING_BYTES + 36 + 8 + 8 + 8;
            if res.flow.is_some() {
                r_row += 36;
            }
            if let Some(err) = &res.error {
                r_row = checked_add_bytes(r_row, err.len())?;
            }
            if let Some(notes) = &res.notes {
                r_row = checked_add_bytes(r_row, notes.len())?;
            }
            if res.highlight.is_some() {
                r_row += 8;
            }
            total = total
                .checked_add(r_row)
                .context("Intruder Session 逻辑数据计量溢出")?;
        }
        Ok(total)
    }

    pub(super) fn mutable_bytes(&self, ordinal: Option<u64>) -> Result<u64> {
        let mut bytes = 0u64;
        bytes = checked_add_bytes(bytes, self.run.title.len())?;
        bytes = checked_add_bytes(bytes, run_error_len(&self.run.state))?;
        if self.run.resource_pool.is_some() {
            bytes += 36;
        }
        if self.run.baseline.is_some() {
            bytes += 8;
        }
        for res in self.results.values() {
            bytes = checked_add_bytes(bytes, res.notes.as_ref().map_or(0, Vec::len))?;
            bytes += 8 + if res.highlight.is_some() { 8 } else { 0 } + 8;
        }
        let Some(ordinal) = ordinal else {
            return Ok(bytes);
        };
        if let Some(res) = self.results.get(&ordinal) {
            let mut r_bytes = ROW_ACCOUNTING_BYTES + 36 + 8;
            if res.flow.is_some() {
                r_bytes += 36;
            }
            if let Some(err) = &res.error {
                r_bytes = checked_add_bytes(r_bytes, err.len())?;
            }
            bytes = bytes
                .checked_add(r_bytes)
                .context("Intruder RAM 字节计量溢出")?;
            if let Some(flow_id) = res.flow
                && let Some(flow) = self.flows.get(&flow_id)
            {
                let mut f_bytes = ROW_ACCOUNTING_BYTES + 36 + 8 + 8;
                f_bytes = checked_add_bytes(f_bytes, flow.summary.url.len())?;
                f_bytes = checked_add_bytes(f_bytes, flow.summary.method.len())?;
                f_bytes =
                    checked_add_bytes(f_bytes, store::state_code(&flow.summary.state).len())?;
                f_bytes = checked_add_bytes(f_bytes, flow.summary_json.len())?;
                bytes = bytes
                    .checked_add(f_bytes)
                    .context("Intruder RAM 字节计量溢出")?;
                for (head, body) in flow.messages.values() {
                    let m_bytes = checked_add_bytes(
                        checked_add_bytes(ROW_ACCOUNTING_BYTES + 36 + 8, head.len())?,
                        body.len(),
                    )?;
                    bytes = bytes
                        .checked_add(m_bytes)
                        .context("Intruder RAM 字节计量溢出")?;
                }
            }
        }
        Ok(bytes)
    }

    pub(super) fn has_flow(&self, id: FlowId) -> bool {
        self.flows.contains_key(&id)
    }

    pub(super) fn flow_ids_in_order(&self) -> Vec<FlowId> {
        self.results.values().filter_map(|row| row.flow).collect()
    }

    pub(super) fn matches_disk_flow(
        &self,
        connection: &Connection,
        id: FlowId,
        check_messages: bool,
    ) -> Result<bool> {
        let Some(flow) = self.flows.get(&id) else {
            return Ok(false);
        };
        type FlowMetaRow = (String, u64, i64, String, String, String, Vec<u8>);
        let row: Option<FlowMetaRow> = connection
            .query_row(
                "SELECT id, revision, started, url, method, state, summary
                 FROM main.flows WHERE id=?1",
                [id.to_string()],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((disk_id, rev, started, url, method, state, summary_json)) = row else {
            return Ok(false);
        };
        if disk_id != id.to_string()
            || rev != flow.summary.revision
            || started != flow.summary.started_unix_ms
            || url != flow.summary.url
            || method != flow.summary.method
            || state != store::state_code(&flow.summary.state)
            || summary_json != flow.summary_json
        {
            return Ok(false);
        }
        if !check_messages {
            return Ok(true);
        }
        let mut stmt = connection.prepare(
            "SELECT side, head, body FROM main.messages WHERE flow=?1 ORDER BY side",
        )?;
        let mut rows = stmt.query([id.to_string()])?;
        let mut disk_msgs = Vec::new();
        while let Some(row) = rows.next()? {
            disk_msgs.push((
                row.get::<_, u8>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ));
        }
        if disk_msgs.len() != flow.messages.len() {
            return Ok(false);
        }
        for ((&side, (head, body)), (d_side, d_head, d_body)) in
            flow.messages.iter().zip(disk_msgs.iter())
        {
            if side != *d_side || head != d_head || body != d_body {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn matches_other_flow(&self, other: &Self, id: FlowId) -> bool {
        match (self.flows.get(&id), other.flows.get(&id)) {
            (Some(a), Some(b)) => {
                a.summary == b.summary
                    && a.summary_json == b.summary_json
                    && a.messages == b.messages
            }
            _ => false,
        }
    }

    pub(in crate::store::intruder) fn read_settings_only(
        &self,
    ) -> Result<(
        crate::IntruderAttackSettings,
        Vec<crate::IntruderPayloadSetConfig>,
    )> {
        ensure!(
            self.spec_meta.len() <= intruder_mod::MAX_SPEC_META,
            "Intruder 配置元数据类型或长度无效"
        );
        let meta: intruder_mod::SpecMeta = serde_json::from_slice(&self.spec_meta)?;
        Ok((meta.attack_settings, meta.payload_configs))
    }

    pub(in crate::store::intruder) fn read_spec_with_settings(
        &self,
    ) -> Result<(
        IntruderSpec,
        crate::IntruderAttackSettings,
        Vec<crate::IntruderPayloadSetConfig>,
    )> {
        ensure!(
            self.spec_meta.len() <= intruder_mod::MAX_SPEC_META
                && self.template.len() <= crate::intruder::MAX_REQUEST_BYTES,
            "Intruder 配置元数据或模板超过限额"
        );
        let meta: intruder_mod::SpecMeta = serde_json::from_slice(&self.spec_meta)?;
        ensure!(
            (1..=MAX_PAYLOAD_SETS).contains(&meta.set_count)
                && self.payloads.len() == meta.set_count,
            "Intruder payload set 数量无效"
        );
        let mut count = 0usize;
        let mut bytes = 0usize;
        for set in &self.payloads {
            for val in set {
                count += 1;
                ensure!(
                    count <= crate::intruder::MAX_PAYLOADS,
                    "Intruder payload 总数量超过限额"
                );
                bytes = bytes
                    .checked_add(val.len())
                    .context("Intruder payload 字节数溢出")?;
                ensure!(
                    bytes <= intruder_mod::MAX_PAYLOAD_BYTES,
                    "Intruder payload 总字节数超过限额"
                );
            }
        }
        let spec = IntruderSpec {
            resource_pool: meta.resource_pool,
            baseline_request: meta.baseline_request,
            revision: self.run.spec_revision,
            draft: Draft {
                revision: meta.draft_revision,
                target: meta.target,
                raw: self.template.clone(),
                version: meta.version,
            },
            attack_type: meta.attack_type,
            positions: meta.positions,
            payload_sets: self.payloads.clone(),
            max_concurrent_requests: meta.max_concurrent_requests,
            request_timeout_ms: meta.request_timeout_ms,
            update_host_header: meta.update_host_header,
            save_to_project: meta.save_to_project,
        };
        ensure!(
            spec.validate()?
                .checked_sub(u64::from(spec.baseline_request))
                .and_then(|payloads| payloads.checked_add(u64::from(self.run.baseline.is_some())))
                == Some(self.run.total)
                && (!spec.baseline_request
                    || self.run.baseline.is_some_and(|baseline| baseline.ordinal == 0)),
            "Intruder 冻结配置与请求总数不一致"
        );
        ensure!(
            spec.resource_pool.is_none() || self.run.resource_pool.is_some(),
            "Intruder 冻结资源池配置丢失运行绑定"
        );
        Ok((spec, meta.attack_settings, meta.payload_configs))
    }

    pub(super) fn read_flow_record(&self, id: FlowId) -> Result<Option<FlowRecord>> {
        let Some(flow) = self.flows.get(&id) else {
            return Ok(None);
        };
        let (req_head, req_body) = flow.messages.get(&0).context("请求正文缺失")?.clone();
        let request = store::request_from_parts((req_head, req_body))?;
        let response = flow
            .messages
            .get(&1)
            .map(|(h, b)| store::response_from_parts((h.clone(), b.clone())))
            .transpose()?;
        Ok(Some(FlowRecord {
            summary: serde_json::from_slice(&flow.summary_json)?,
            request,
            response,
        }))
    }

    pub(super) fn read_original(&self, id: FlowId) -> Result<OriginalMessages> {
        let Some(flow) = self.flows.get(&id) else {
            return Ok(OriginalMessages::default());
        };
        let response = flow
            .messages
            .get(&3)
            .map(|(h, b)| store::response_with_stream((h.clone(), b.clone())))
            .transpose()?;
        let request = flow
            .messages
            .get(&2)
            .map(|(h, b)| store::request_from_parts((h.clone(), b.clone())))
            .transpose()?;
        let mut original = OriginalMessages {
            request,
            response_body_complete: response
                .as_ref()
                .and_then(|(_, stream)| stream.as_ref().map(|stream| stream.body_complete)),
            response: response.map(|(response, _)| response),
            provenance: crate::FlowProvenance::default(),
        };
        if original.request.is_some()
            || original.response.is_some()
            || !original.provenance.is_empty()
        {
            let final_req = flow
                .messages
                .get(&0)
                .map(|(h, b)| store::request_from_parts((h.clone(), b.clone())))
                .transpose()?;
            let final_resp = flow
                .messages
                .get(&1)
                .map(|(h, b)| store::response_from_parts((h.clone(), b.clone())))
                .transpose()?;
            original.synthesize_provenance(final_req.as_ref(), final_resp.as_ref());
        }
        Ok(original)
    }

    pub(super) fn result_summary(&self, ordinal: u64) -> Result<Option<IntruderResultSummary>> {
        let Some(row) = self.results.get(&ordinal) else {
            return Ok(None);
        };
        let outcome = match (row.flow, &row.error) {
            (None, Some(error)) => {
                ensure!(
                    error.len() <= intruder_mod::MAX_ERROR_BYTES,
                    "Intruder 拒绝原因超过读取限额"
                );
                IntruderResultOutcome::Rejected(error.clone())
            }
            (Some(flow_id), None) => {
                let flow = self
                    .flows
                    .get(&flow_id)
                    .context("Intruder 结果类型或关联流量无效")?;
                ensure!(
                    flow.summary_json.len() <= intruder_mod::MAX_SUMMARY_BYTES,
                    "Intruder 结果摘要超过读取限额"
                );
                let summary: FlowSummary = serde_json::from_slice(&flow.summary_json)?;
                intruder_mod::validate_summary(&summary)?;
                ensure!(
                    summary.id == flow_id && summary == flow.summary,
                    "Intruder flow 摘要与索引不一致"
                );
                IntruderResultOutcome::Flow(summary)
            }
            _ => bail!("Intruder 结果类型或关联流量无效"),
        };
        Ok(Some(IntruderResultSummary { ordinal, outcome }))
    }

    pub(in crate::store::intruder) fn results_page(
        &self,
        after: Option<u64>,
        limit: usize,
    ) -> Result<(Vec<IntruderResultSummary>, Option<u64>)> {
        ensure!(
            after.is_none_or(|value| value < self.run.total),
            "Intruder 分页游标超出计划"
        );
        let mut output = Vec::with_capacity(limit);
        let mut bytes = 0usize;
        let mut expected = after.map_or(0, |value| value + 1);
        let mut more = false;
        let start = after.map_or(0, |value| value + 1);
        for (&ordinal, row) in self.results.range(start..) {
            if output.len() == limit {
                more = true;
                break;
            }
            let size = match (row.flow, &row.error) {
                (None, Some(err)) => {
                    ensure!(
                        err.len() <= intruder_mod::MAX_ERROR_BYTES,
                        "Intruder 拒绝原因超过读取限额"
                    );
                    err.len()
                }
                (Some(flow_id), None) => {
                    let flow = self
                        .flows
                        .get(&flow_id)
                        .context("Intruder 结果类型或关联流量无效")?;
                    ensure!(
                        flow.summary_json.len() <= intruder_mod::MAX_SUMMARY_BYTES,
                        "Intruder 结果摘要超过读取限额"
                    );
                    flow.summary_json.len()
                }
                _ => bail!("Intruder 结果类型或关联流量无效"),
            };
            if !output.is_empty() && size > PAGE_BYTES - bytes {
                more = true;
                break;
            }
            let result = self.result_summary(ordinal)?.expect("row exists");
            ensure!(
                !self.run.state.is_terminal() || intruder_mod::outcome_terminal(&result.outcome),
                "Intruder 终态 run 仍引用在途结果"
            );
            ensure!(
                result.ordinal == expected && expected < self.run.dispatched,
                "Intruder 结果序号缺失或与计数不符"
            );
            expected += 1;
            bytes += size;
            output.push(result);
        }
        ensure!(
            more || expected >= self.run.dispatched,
            "Intruder 结果记录缺失"
        );
        let next_after = more.then(|| output.last().expect("nonempty bounded page").ordinal);
        Ok((output, next_after))
    }

    pub(super) fn ensure_no_pool_references(
        &self,
        removed: &BTreeSet<Uuid>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        ensure!(!cancel.is_cancelled(), "Intruder 资源池删除已取消");
        if let Some(pool) = self.run.resource_pool {
            ensure!(!pool.is_nil(), "Intruder 资源池引用编码无效；未删除");
            ensure!(
                !removed.contains(&pool),
                "Intruder 资源池仍被运行引用；未删除"
            );
        }
        let meta: intruder_mod::SpecMeta = strict_json(&self.spec_meta)?;
        ensure!(
            meta.resource_pool
                .as_ref()
                .is_none_or(|selection| !removed.contains(&selection.id())),
            "Intruder 资源池仍被冻结配置引用；未删除"
        );
        Ok(())
    }

    pub(super) fn validate_snapshot(
        &self,
        cancel: &CancellationToken,
        allow_running: bool,
    ) -> Result<IntruderRunSummary> {
        ensure!(
            self.run.state.is_terminal()
                || (allow_running
                    && matches!(
                        self.run.state,
                        IntruderRunState::Running | IntruderRunState::Paused
                    )),
            "Intruder 状态不允许此保留转换；数据保留"
        );
        let _: intruder_mod::SpecMeta = strict_json(&self.spec_meta)?;
        self.read_spec_with_settings()?;
        let mut expected = 0u64;
        let mut completed = 0u64;
        for &ordinal in self.results.keys() {
            ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
            let result = self.result_summary(ordinal)?.expect("row exists");
            let terminal = intruder_mod::outcome_terminal(&result.outcome);
            ensure!(
                result.ordinal == expected
                    && expected < self.run.dispatched
                    && (terminal || (allow_running && !self.run.state.is_terminal())),
                "Intruder 结果序号、计数或终态无效；数据保留"
            );
            completed += u64::from(terminal);
            if let IntruderResultOutcome::Flow(summary) = &result.outcome {
                ensure!(
                    terminal || summary.state == FlowState::Pending,
                    "Intruder 在途结果必须是已登记的 Pending 请求；数据保留"
                );
                let flow = self
                    .flows
                    .get(&summary.id)
                    .context("Intruder 流量缺失")?;
                let strict: FlowSummary = strict_json(&flow.summary_json)?;
                ensure!(strict == *summary, "Intruder 流量摘要不一致");
                for (&side, (head, body)) in &flow.messages {
                    ensure!(
                        side <= 3
                            && head.len() <= intruder_mod::MAX_MESSAGE_HEAD
                            && body.len() <= MAX_BODY,
                        "Intruder 消息版本、类型或长度不受支持；数据保留"
                    );
                    if side == 0 || side == 2 {
                        let _: store::OwnedRequestHead = strict_json(head)?;
                    } else {
                        ensure!(
                            intruder_mod::is_flow_terminal(&summary.state),
                            "Intruder 在途结果不能包含响应版本；数据保留"
                        );
                        let parsed: store::OwnedResponseHead = strict_json(head)?;
                        ensure!(
                            parsed.stream.is_none(),
                            "Intruder 流式消息布局不支持保留切换；数据保留"
                        );
                    }
                }
                let record = self
                    .read_flow_record(summary.id)?
                    .context("Intruder 流量缺失")?;
                ensure!(record.summary == *summary, "Intruder 流量内容与摘要不一致");
                ensure!(
                    intruder_mod::is_flow_terminal(&summary.state) || record.response.is_none(),
                    "Intruder 在途结果不能包含响应；数据保留"
                );
                intruder_mod::validate_record(&record)?;
            }
            expected += 1;
        }
        ensure!(
            expected == self.run.dispatched,
            "Intruder 结果缺失；数据保留"
        );
        ensure!(
            completed == self.run.completed,
            "Intruder 已完成结果数量与运行计数不一致；数据保留"
        );
        Ok(self.run.clone())
    }

    pub(super) fn rename(
        &mut self,
        expected_title_revision: u64,
        title: &str,
        cancel: &CancellationToken,
        #[cfg(test)] hook: Option<&SessionWriteHook>,
    ) -> Result<IntruderRunSummary> {
        ensure!(
            title.len() <= intruder_mod::MAX_TITLE_BYTES,
            "Intruder 任务名称超过 4096 字节；名称未改变"
        );
        ensure!(
            self.run.title_revision == expected_title_revision,
            "Intruder 任务名称 revision 已改变；名称未覆盖"
        );
        let revision = intruder_mod::next_revision(expected_title_revision);
        ensure!(!cancel.is_cancelled(), "Intruder 重命名已取消；名称未改变");
        #[cfg(test)]
        if let Some(hook) = hook {
            ensure!(hook("rename"), "Intruder Session 写入测试拒绝");
        }
        self.run.title = title.to_owned();
        self.run.title_revision = revision;
        Ok(self.run.clone())
    }

    pub(super) fn bind_resource_pool(
        &mut self,
        expected: u64,
        pool_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        ensure!(
            !pool_id.is_nil() && !cancel.is_cancelled(),
            "Intruder 资源池绑定无效或已取消"
        );
        ensure!(
            self.run.resource_pool_revision == expected,
            "Intruder 资源池绑定 revision 已改变；未覆盖"
        );
        ensure!(
            self.run.resource_pool != Some(pool_id),
            "Intruder 已绑定此资源池；未重复修改"
        );
        let revision = intruder_mod::next_revision(expected);
        ensure!(!cancel.is_cancelled(), "Intruder 资源池绑定已取消");
        self.run.resource_pool = Some(pool_id);
        self.run.resource_pool_revision = revision;
        Ok(self.run.clone())
    }

    pub(super) fn request_baseline<T>(
        &mut self,
        expected_revision: u64,
        cancel: &CancellationToken,
        prepare: impl FnOnce(IntruderSpec) -> Result<T>,
    ) -> Result<(IntruderRunSummary, T)> {
        ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
        ensure!(
            self.run.revision == expected_revision,
            "Intruder run revision 已改变；未追加基准请求"
        );
        ensure!(
            self.run.baseline.is_none(),
            "Intruder 已有基准请求；不会再次发送"
        );
        ensure!(
            matches!(
                self.run.state,
                IntruderRunState::Running | IntruderRunState::Paused | IntruderRunState::Completed
            ),
            "Intruder 状态不允许追加基准请求；不会恢复原攻击"
        );
        let total = self
            .run
            .total
            .checked_add(1)
            .filter(|total| *total <= crate::intruder::MAX_REQUESTS)
            .context("Intruder 基准请求超过总请求限额；未追加")?;
        let revision = intruder_mod::next_revision(self.run.revision);
        let _: intruder_mod::SpecMeta = strict_json(&self.spec_meta)?;
        let (spec, _, _) = self.read_spec_with_settings()?;
        let mut expected = 0u64;
        let mut completed = 0u64;
        for &ordinal in self.results.keys() {
            ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
            let result = self.result_summary(ordinal)?.expect("row exists");
            ensure!(
                result.ordinal == expected && expected < self.run.dispatched,
                "Intruder 结果序号或计数损坏；未追加基准请求"
            );
            completed += u64::from(intruder_mod::outcome_terminal(&result.outcome));
            expected += 1;
        }
        ensure!(
            expected == self.run.dispatched && completed == self.run.completed,
            "Intruder 结果数量与计数不符；未追加基准请求"
        );
        let prepared = prepare(spec)?;
        ensure!(!cancel.is_cancelled(), "Intruder 基准请求已取消；未追加");
        self.run.baseline = Some(IntruderBaseline {
            ordinal: self.run.dispatched,
            completed: false,
        });
        self.run.total = total;
        self.run.revision = revision;
        if self.run.state != IntruderRunState::Paused {
            self.run.state = IntruderRunState::Running;
        }
        Ok((self.run.clone(), prepared))
    }

    pub(super) fn write_result_with_reason_phrase(
        &mut self,
        ordinal: u64,
        result: std::result::Result<&FlowRecord, &str>,
        reason_phrase: Option<&[u8]>,
        #[cfg(test)] hook: Option<&SessionWriteHook>,
    ) -> Result<IntruderRunSummary> {
        ensure!(
            ordinal < self.run.total,
            "Intruder 结果序号超过冻结计划的数量"
        );
        let previous = self.result_summary(ordinal)?;
        ensure!(
            previous.is_none() || ordinal < self.run.dispatched,
            "Intruder 已有结果超过已派发计数"
        );
        let terminal = match result {
            Ok(record) => {
                intruder_mod::validate_record_with_reason_phrase(record, reason_phrase)?;
                intruder_mod::is_flow_terminal(&record.summary.state)
            }
            Err(message) => {
                ensure!(
                    reason_phrase.is_none(),
                    "Intruder 拒绝结果不能附带响应 reason phrase"
                );
                ensure!(
                    message.len() <= intruder_mod::MAX_ERROR_BYTES,
                    "Intruder 拒绝原因超过 64 KiB"
                );
                true
            }
        };
        let mut next_run = self.run.clone();
        if let Some(previous) = &previous {
            ensure!(
                !next_run.state.is_terminal() || intruder_mod::outcome_terminal(&previous.outcome),
                "Intruder 终态 run 仍引用在途结果"
            );
            match (&previous.outcome, result) {
                (IntruderResultOutcome::Rejected(before), Err(after)) if before == after => {
                    return Ok(next_run);
                }
                (IntruderResultOutcome::Flow(before), Ok(after)) => {
                    ensure!(
                        before.id == after.summary.id,
                        "Intruder 序号不能改为另一 flow ID"
                    );
                    if before == &after.summary
                        && self.record_matches_with_reason_phrase(after, reason_phrase)?
                    {
                        return Ok(next_run);
                    }
                    ensure!(
                        !next_run.state.is_terminal()
                            && !intruder_mod::is_flow_terminal(&before.state),
                        "Intruder 终态结果不能被覆盖"
                    );
                    ensure!(
                        terminal
                            && after.summary.revision
                                == intruder_mod::next_revision(before.revision),
                        "Intruder 仅允许在途结果进入下一 revision 的终态"
                    );
                    ensure!(
                        before.started_unix_ms == after.summary.started_unix_ms
                            && before.method == after.summary.method
                            && before.url == after.summary.url
                            && before.request_bytes == after.summary.request_bytes
                            && self.request_matches(after)?
                            && self.response_absent(before.id),
                        "Intruder 在途请求不能被替换"
                    );
                }
                _ => bail!("Intruder 结果类型或拒绝原因冲突；未覆盖已有结果"),
            }
        } else {
            ensure!(
                next_run.state == IntruderRunState::Running,
                "Intruder 已停止派发新请求"
            );
            ensure!(
                ordinal == next_run.dispatched,
                "Intruder 新结果必须按连续序号登记"
            );
            if let Ok(record) = result {
                ensure!(
                    !self.flows.contains_key(&record.summary.id),
                    "Intruder 新序号不能使用已经存在的 flow ID"
                );
            }
            next_run.dispatched += 1;
        }

        if terminal {
            next_run.completed += 1;
            if let Some(baseline) = &mut next_run.baseline
                && baseline.ordinal == ordinal
            {
                baseline.completed = true;
            }
        }
        next_run.revision = intruder_mod::next_revision(next_run.revision);
        ensure!(
            next_run.completed <= next_run.dispatched && next_run.dispatched <= next_run.total,
            "Intruder 计数不一致"
        );

        match result {
            Ok(record) => {
                let summary_json = serde_json::to_vec(&record.summary)?;
                let req_head =
                    serde_json::to_vec(&store::RequestHead::from(&record.request))?;
                ensure!(
                    req_head.len() <= crate::store::MAX_MESSAGE_HEAD_BYTES,
                    "消息头编码超过 4 MiB 保存限额"
                );
                let resp_pair = if let Some(response) = &record.response {
                    let effective_reason = reason_phrase.or(response.reason_phrase.as_deref());
                    let mut head = store::ResponseHead::from(response);
                    head.reason_phrase = effective_reason;
                    let resp_head = serde_json::to_vec(&head)?;
                    ensure!(
                        resp_head.len() <= crate::store::MAX_MESSAGE_HEAD_BYTES,
                        "消息头编码超过 4 MiB 保存限额"
                    );
                    Some((resp_head, response.body.clone()))
                } else {
                    None
                };

                #[cfg(test)]
                if let Some(hook) = hook {
                    ensure!(hook("insert_flow"), "Intruder Session 写入测试拒绝");
                    if previous.is_none() {
                        ensure!(hook("dispatched"), "Intruder Session 写入测试拒绝");
                    }
                }

                let flow = self
                    .flows
                    .entry(record.summary.id)
                    .or_insert_with(|| SessionFlow {
                        summary: record.summary.clone(),
                        summary_json: summary_json.clone(),
                        messages: BTreeMap::new(),
                    });
                flow.summary = record.summary.clone();
                flow.summary_json = summary_json;
                Self::put_message(&mut flow.messages, 0, req_head, record.request.body.clone());
                if let Some((resp_head, resp_body)) = resp_pair {
                    Self::put_message(&mut flow.messages, 1, resp_head, resp_body);
                } else {
                    if let Some(existing) = flow.messages.get(&1).cloned() {
                        flow.messages.entry(3).or_insert(existing);
                    }
                    flow.messages.remove(&1);
                }
                if previous.is_none() {
                    self.results.insert(
                        ordinal,
                        SessionResultRow {
                            ordinal,
                            flow: Some(record.summary.id),
                            error: None,
                            notes: None,
                            notes_revision: 0,
                            highlight: None,
                            highlight_revision: 0,
                        },
                    );
                }
            }
            Err(message) => {
                #[cfg(test)]
                if let Some(hook) = hook
                    && previous.is_none()
                {
                    ensure!(hook("dispatched"), "Intruder Session 写入测试拒绝");
                }
                self.results.insert(
                    ordinal,
                    SessionResultRow {
                        ordinal,
                        flow: None,
                        error: Some(message.to_owned()),
                        notes: None,
                        notes_revision: 0,
                        highlight: None,
                        highlight_revision: 0,
                    },
                );
            }
        }
        self.run = next_run;
        Ok(self.run.clone())
    }

    fn put_message(
        messages: &mut BTreeMap<u8, (Vec<u8>, Vec<u8>)>,
        side: u8,
        head: Vec<u8>,
        body: Vec<u8>,
    ) {
        if let Some(existing) = messages
            .get(&side)
            .filter(|(existing_head, existing_body)| existing_head != &head || existing_body != &body)
            .cloned()
        {
            messages.entry(side + 2).or_insert(existing);
        }
        messages.insert(side, (head, body));
    }

    fn request_matches(&self, record: &FlowRecord) -> Result<bool> {
        let head = serde_json::to_vec(&store::RequestHead::from(&record.request))?;
        Ok(self.message_matches(record.summary.id, 0, &head, &record.request.body))
    }

    fn response_absent(&self, flow_id: FlowId) -> bool {
        self.flows
            .get(&flow_id)
            .is_none_or(|f| !f.messages.contains_key(&1))
    }

    fn record_matches(&self, record: &FlowRecord) -> Result<bool> {
        self.record_matches_with_reason_phrase(record, None)
    }

    fn record_matches_with_reason_phrase(
        &self,
        record: &FlowRecord,
        reason_phrase: Option<&[u8]>,
    ) -> Result<bool> {
        if !self.request_matches(record)? {
            return Ok(false);
        }
        match &record.response {
            Some(response) => {
                let mut head = store::ResponseHead::from(response);
                let effective_reason = reason_phrase.or(response.reason_phrase.as_deref());
                head.reason_phrase = effective_reason;
                if self.message_matches(
                    record.summary.id,
                    1,
                    &serde_json::to_vec(&head)?,
                    &response.body,
                ) {
                    return Ok(true);
                }
                if effective_reason.is_none()
                    && let Some(flow) = self.flows.get(&record.summary.id)
                    && let Some((stored_bytes, _)) = flow.messages.get(&1)
                    && stored_bytes.len() <= crate::store::MAX_MESSAGE_HEAD_BYTES
                {
                    let stored_head: store::OwnedResponseHead =
                        serde_json::from_slice(stored_bytes)?;
                    if stored_head.reason_phrase.is_some() {
                        head.reason_phrase = stored_head.reason_phrase.as_deref();
                        return Ok(self.message_matches(
                            record.summary.id,
                            1,
                            &serde_json::to_vec(&head)?,
                            &response.body,
                        ));
                    }
                }
                Ok(false)
            }
            None => Ok(self.response_absent(record.summary.id)),
        }
    }

    fn message_matches(&self, flow_id: FlowId, side: u8, head: &[u8], body: &[u8]) -> bool {
        self.flows
            .get(&flow_id)
            .and_then(|f| f.messages.get(&side))
            .is_some_and(|(h, b)| {
                h.len() <= intruder_mod::MAX_MESSAGE_HEAD
                    && b.len() <= MAX_BODY
                    && h.as_slice() == head
                    && b.as_slice() == body
            })
    }

    pub(super) fn write_state(&mut self, state: IntruderRunState) -> Result<IntruderRunSummary> {
        if state.is_terminal() {
            ensure!(
                self.run.completed == self.run.dispatched,
                "Intruder 尚有在途结果，不能确认停止"
            );
            let mut expected = 0u64;
            for &ordinal in self.results.keys() {
                let result = self.result_summary(ordinal)?.expect("row exists");
                ensure!(
                    result.ordinal == expected && expected < self.run.dispatched,
                    "Intruder 终态结果序号缺失或与计数不符"
                );
                ensure!(
                    intruder_mod::outcome_terminal(&result.outcome),
                    "Intruder 尚有在途结果，不能确认停止"
                );
                expected += 1;
            }
            ensure!(
                expected == self.run.dispatched,
                "Intruder 终态结果记录缺失"
            );
        }
        if self.run.state == state {
            return Ok(self.run.clone());
        }
        ensure!(
            !self.run.state.is_terminal(),
            "Intruder 终态不能重新开始或被覆盖"
        );
        match &state {
            IntruderRunState::Running => ensure!(
                self.run.state == IntruderRunState::Paused && self.run.completed < self.run.total,
                "Intruder 只能恢复尚未结束的暂停攻击，不能重发已派发请求"
            ),
            IntruderRunState::Paused => ensure!(
                self.run.state == IntruderRunState::Running && self.run.completed < self.run.total,
                "Intruder 只能暂停正在运行的攻击"
            ),
            IntruderRunState::Interrupted => {
                bail!("Intruder 不能跳过中断恢复")
            }
            IntruderRunState::Stopping => (),
            IntruderRunState::Completed => ensure!(
                self.run.completed == self.run.dispatched && self.run.dispatched == self.run.total,
                "Intruder 尚未收齐所有请求终态"
            ),
            IntruderRunState::Stopped | IntruderRunState::Failed(_) => ensure!(
                self.run.completed == self.run.dispatched,
                "Intruder 尚有在途结果，不能确认停止"
            ),
        }
        if let IntruderRunState::Failed(message) = &state {
            ensure!(
                message.len() <= intruder_mod::MAX_ERROR_BYTES,
                "Intruder 错误超过 64 KiB"
            );
        }
        let next_rev = intruder_mod::next_revision(self.run.revision);
        self.run.state = state;
        self.run.revision = next_rev;
        Ok(self.run.clone())
    }

    pub(super) fn write_result_notes(
        &mut self,
        ordinal: u64,
        expected_notes_revision: u64,
        notes: &[u8],
        cancel: &CancellationToken,
    ) -> Result<(u64, Vec<u8>)> {
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果备注修改已取消；数据保留"
        );
        ensure!(
            notes.len() <= intruder_mod::MAX_ERROR_BYTES,
            "Intruder 结果备注超过 64 KiB 字节限额"
        );
        ensure!(
            ordinal < self.run.total,
            "Intruder 结果序号超过冻结计划的数量"
        );
        let row = self
            .results
            .get(&ordinal)
            .context("Intruder 结果不存在；不能修改注解")?;
        ensure!(
            ordinal < self.run.dispatched,
            "Intruder 基准请求尚未派发；不能修改注解"
        );
        let current = self
            .result_summary(ordinal)?
            .context("Intruder 结果不存在；不能修改注解")?;
        ensure!(
            matches!(current.outcome, IntruderResultOutcome::Flow(_)),
            "未发送的 Intruder 请求不能设置注解"
        );
        ensure!(
            row.notes_revision == expected_notes_revision,
            "Intruder 结果备注 revision 已改变；未覆盖（当前 {}）",
            row.notes_revision
        );
        let revision = intruder_mod::next_revision(expected_notes_revision);
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果备注修改已取消；数据保留"
        );
        let row = self.results.get_mut(&ordinal).expect("checked");
        row.notes = Some(notes.to_vec());
        row.notes_revision = revision;
        Ok((revision, notes.to_vec()))
    }

    pub(super) fn write_result_highlights(
        &mut self,
        entries: &[(u64, u64)],
        highlight: Option<u8>,
        cancel: &CancellationToken,
    ) -> Result<Vec<(u64, u64)>> {
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果高亮修改已取消；数据保留"
        );
        let mut updates = Vec::with_capacity(entries.len());
        for &(ordinal, expected_highlight_revision) in entries {
            ensure!(
                !cancel.is_cancelled(),
                "Intruder 结果高亮修改已取消；数据保留"
            );
            ensure!(
                ordinal < self.run.total,
                "Intruder 结果序号超过冻结计划的数量"
            );
            let row = self
                .results
                .get(&ordinal)
                .context("Intruder 结果不存在；不能修改注解")?;
            ensure!(
                ordinal < self.run.dispatched,
                "Intruder 基准请求尚未派发；不能修改注解"
            );
            let current = self
                .result_summary(ordinal)?
                .context("Intruder 结果不存在；不能修改注解")?;
            ensure!(
                matches!(current.outcome, IntruderResultOutcome::Flow(_)),
                "未发送的 Intruder 请求不能设置注解"
            );
            ensure!(
                row.highlight_revision == expected_highlight_revision,
                "Intruder 结果高亮修改冲突"
            );
            let revision = intruder_mod::next_revision(expected_highlight_revision);
            updates.push((ordinal, revision));
        }
        ensure!(
            !cancel.is_cancelled(),
            "Intruder 结果高亮修改已取消；数据保留"
        );
        for &(ordinal, revision) in &updates {
            let row = self.results.get_mut(&ordinal).expect("checked");
            row.highlight = highlight;
            row.highlight_revision = revision;
        }
        Ok(updates)
    }

    pub(super) fn write_recovery(
        &mut self,
        mutations: &[store::IntruderMutation],
        #[cfg(test)] hook: Option<&SessionWriteHook>,
    ) -> Result<()> {
        let mut final_run = None;
        let mut final_results = BTreeMap::new();
        for mutation in mutations {
            let run = match mutation {
                store::IntruderMutation::Result {
                    run,
                    ordinal,
                    result,
                } => {
                    final_results.insert(*ordinal, result);
                    run
                }
                store::IntruderMutation::State { run } => run,
            };
            ensure!(
                run.id == self.run.id && run.retention == IntruderRetention::Session,
                "Intruder 恢复批次不能改变存储归属"
            );
            final_run = Some(run);
        }
        if let Some(expected) = final_run
            && self.run == *expected
        {
            for (ordinal, result) in final_results {
                ensure!(
                    ordinal < expected.dispatched,
                    "Intruder 恢复批次最终结果超过已登记计数"
                );
                let stored = self
                    .result_summary(ordinal)?
                    .context("Intruder 恢复批次最终结果缺失")?;
                match (&stored.outcome, result.as_deref().map_err(String::as_str)) {
                    (IntruderResultOutcome::Flow(before), Ok(after)) => {
                        intruder_mod::validate_record(after)?;
                        ensure!(
                            before == &after.summary && self.record_matches(after)?,
                            "Intruder 恢复批次最终流量与磁盘记录冲突"
                        );
                    }
                    (IntruderResultOutcome::Rejected(before), Err(after)) => ensure!(
                        before == after,
                        "Intruder 恢复批次最终拒绝原因与磁盘记录冲突"
                    ),
                    _ => bail!("Intruder 恢复批次最终结果类型与磁盘记录冲突"),
                }
            }
            if expected.state.is_terminal() {
                let mut cloned = self.clone();
                let actual = cloned.write_state(expected.state.clone())?;
                ensure!(actual == *expected, "Intruder 恢复批次最终状态冲突");
                *self = cloned;
            }
            return Ok(());
        }
        let mut staged = self.clone();
        for mutation in mutations {
            let (expected, actual) = match mutation {
                store::IntruderMutation::Result {
                    run,
                    ordinal,
                    result,
                } => (
                    run,
                    staged.write_result_with_reason_phrase(
                        *ordinal,
                        result.as_deref().map_err(String::as_str),
                        None,
                        #[cfg(test)]
                        hook,
                    )?,
                ),
                store::IntruderMutation::State { run } => {
                    (run, staged.write_state(run.state.clone())?)
                }
            };
            ensure!(
                actual == *expected,
                "Intruder 恢复步骤的完整摘要或 revision 与预期不符"
            );
        }
        *self = staged;
        Ok(())
    }

    fn payload_shape(
        &self,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<(intruder_mod::SpecMeta, Vec<usize>)> {
        check()?;
        ensure!(
            self.spec_meta.len() <= intruder_mod::MAX_SPEC_META
                && self.template.len() <= crate::intruder::MAX_REQUEST_BYTES,
            "Intruder 配置元数据或模板超过限额"
        );
        let meta: intruder_mod::SpecMeta = serde_json::from_slice(&self.spec_meta)?;
        ensure!(
            (1..=MAX_PAYLOAD_SETS).contains(&meta.set_count)
                && self.payloads.len() == meta.set_count,
            "Intruder payload set 数量无效"
        );
        validate_positions(self.template.len(), &meta.positions)?;
        let mut lengths = Vec::with_capacity(self.payloads.len());
        let mut count = 0usize;
        let mut bytes = 0usize;
        for set in &self.payloads {
            check()?;
            lengths.push(set.len());
            count += set.len();
            for val in set {
                bytes = bytes
                    .checked_add(val.len())
                    .context("Intruder payload 字节数溢出")?;
            }
            ensure!(
                count <= crate::intruder::MAX_PAYLOADS
                    && bytes <= intruder_mod::MAX_PAYLOAD_BYTES,
                "Intruder payload 输入超过限额"
            );
        }
        Ok((meta, lengths))
    }

    pub(in crate::store::intruder) fn payload_summaries(
        &self,
        run: &IntruderRunSummary,
        ordinals: &[u64],
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Vec<intruder_mod::result_view::ResultPayloads>> {
        check()?;
        let id = run.id;
        ensure!(
            !id.is_nil() && !ordinals.is_empty() && ordinals.len() <= 501,
            "Intruder payload 查询需要有效运行及至多 500 行和一个页外选择"
        );
        let unique: BTreeSet<_> = ordinals.iter().copied().collect();
        ensure!(
            unique.len() == ordinals.len(),
            "Intruder payload 查询结果序号重复"
        );
        ensure!(
            ordinals.iter().all(|ordinal| *ordinal < run.total),
            "Intruder payload 结果序号超出计划"
        );
        let (meta, lengths) = self.payload_shape(check)?;
        let count = payload_request_count(meta.attack_type, meta.positions.len(), &lengths)?;
        ensure!(
            count.checked_add(u64::from(run.baseline.is_some())) == Some(run.total)
                && (!meta.baseline_request
                    || run.baseline.is_some_and(|baseline| baseline.ordinal == 0)),
            "Intruder payload 组合数量与运行不一致"
        );
        let mut page_bytes = 0usize;
        let mut cache = BTreeMap::new();
        let mut output = Vec::with_capacity(ordinals.len());
        for &ordinal in ordinals {
            check()?;
            let selected = payload_positions(
                meta.attack_type,
                meta.positions.len(),
                &lengths,
                ordinal,
                run.baseline.map(|baseline| baseline.ordinal),
            )?;
            let mut payloads = Vec::with_capacity(selected.len());
            let mut inputs = BTreeSet::new();
            let mut row_bytes = 32usize;
            for selected in selected {
                check()?;
                let key = (selected.set_index, selected.payload_index);
                if !inputs.insert(key) {
                    continue;
                }
                if let std::collections::btree_map::Entry::Vacant(entry) = cache.entry(key) {
                    let raw = self
                        .payloads
                        .get(selected.set_index)
                        .and_then(|set| set.get(selected.payload_index))
                        .context("Intruder payload 输入不存在")?;
                    ensure!(
                        raw.len() <= intruder_mod::MAX_PAYLOAD_BYTES,
                        "Intruder payload 超出读取限额"
                    );
                    let raw_bytes = raw.len() as u64;
                    let prefix_len = raw.len().min(intruder_mod::result_view::PREVIEW_BYTES);
                    let prefix = intruder_mod::result_view::preview(
                        raw[..prefix_len].to_vec(),
                        raw_bytes <= intruder_mod::result_view::PREVIEW_BYTES as u64,
                    );
                    entry.insert((raw_bytes, prefix));
                }
                let (raw_bytes, preview_bytes) = &cache[&key];
                row_bytes = row_bytes
                    .checked_add(preview_bytes.len() + 64)
                    .context("Intruder payload 结果行长度溢出")?;
                ensure!(
                    row_bytes <= PAGE_BYTES,
                    "Intruder payload 单行超过 1 MiB；未返回部分输入"
                );
                payloads.push(intruder_mod::result_view::PayloadSummary {
                    selected,
                    raw_bytes: *raw_bytes,
                    preview_bytes: preview_bytes.clone(),
                });
            }
            if row_bytes > PAGE_BYTES - page_bytes {
                break;
            }
            page_bytes += row_bytes;
            output.push(intruder_mod::result_view::ResultPayloads { ordinal, payloads });
        }
        check()?;
        Ok(output)
    }

    pub(in crate::store::intruder) fn payload_order(
        &self,
        run: &IntruderRunSummary,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<intruder_mod::result_view::PayloadOrder> {
        check()?;
        let (meta, lengths) = self.payload_shape(check)?;
        let count = payload_request_count(meta.attack_type, meta.positions.len(), &lengths)?;
        ensure!(
            count.checked_add(u64::from(run.baseline.is_some())) == Some(run.total)
                && (!meta.baseline_request
                    || run.baseline.is_some_and(|baseline| baseline.ordinal == 0)),
            "Intruder payload 组合数量与运行不一致"
        );
        let mut ranks = Vec::with_capacity(self.payloads.len());
        for set in &self.payloads {
            check()?;
            let unique: BTreeSet<&[u8]> = set.iter().map(Vec::as_slice).collect();
            let rank_map: BTreeMap<&[u8], u32> = unique
                .into_iter()
                .enumerate()
                .map(|(idx, val)| (val, u32::try_from(idx + 1).expect("bounded payload count")))
                .collect();
            ranks.push(set.iter().map(|val| rank_map[val.as_slice()]).collect());
        }
        Ok(intruder_mod::result_view::PayloadOrder {
            attack_type: meta.attack_type,
            position_count: meta.positions.len(),
            lengths,
            ranks,
            baseline: run.baseline.map(|baseline| baseline.ordinal),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::store::intruder) fn query_result_view(
        &self,
        query: &IntruderResultViewQuery,
        latest: Option<&IntruderRunSummary>,
        recovery: &[IntruderResultSummary],
        recovery_responses: &BTreeMap<u64, &crate::HttpResponse>,
        validate_session: &dyn Fn(&IntruderResultSummary) -> Result<()>,
        cancel: &CancellationToken,
        shutdown: &CancellationToken,
        #[cfg(test)] body_reads: &std::sync::atomic::AtomicUsize,
    ) -> Result<IntruderResultViewPage> {
        let deadline = Instant::now() + intruder_mod::query::QUERY_TIME;
        let check = || intruder_mod::query::check_query(deadline, cancel, shutdown);
        check()?;
        let stored = self.run.clone();
        let run = latest.unwrap_or(&stored);
        intruder_mod::query::validate_run_overlay(&stored, run)?;
        ensure!(
            query
                .selected_ordinal
                .is_none_or(|ordinal| ordinal < run.total),
            "Intruder 选中结果序号超出计划"
        );

        // Validate recovery overlay
        ensure!(
            recovery.len() <= crate::intruder::MAX_REQUESTS as usize,
            "Intruder 恢复结果数量超过限额"
        );
        let mut recovery_map = BTreeMap::new();
        let mut recovery_flows = BTreeSet::new();
        for result in recovery {
            check()?;
            ensure!(
                result.ordinal < run.dispatched,
                "Intruder 恢复结果序号超出已派发范围"
            );
            ensure!(
                recovery_map.insert(result.ordinal, result).is_none(),
                "Intruder 恢复结果序号重复"
            );
            match &result.outcome {
                IntruderResultOutcome::Rejected(error) => ensure!(
                    error.len() <= intruder_mod::MAX_ERROR_BYTES,
                    "Intruder 恢复拒绝原因超过限额"
                ),
                IntruderResultOutcome::Flow(summary) => {
                    intruder_mod::validate_summary(summary)?;
                    ensure!(
                        !summary.id.is_nil() && recovery_flows.insert(summary.id),
                        "Intruder 恢复 flow 身份重复或无效"
                    );
                    ensure!(
                        serde_json::to_vec(summary)?.len() <= intruder_mod::MAX_SUMMARY_BYTES,
                        "Intruder 恢复摘要超过读取限额"
                    );
                    if self.flows.contains_key(&summary.id) {
                        let owner = self
                            .results
                            .values()
                            .find(|r| r.flow == Some(summary.id))
                            .map(|r| r.ordinal);
                        ensure!(
                            owner == Some(result.ordinal),
                            "Intruder 恢复 flow 归属冲突"
                        );
                    }
                }
            }
        }
        intruder_mod::query::validate_recovery_responses(&recovery_map, recovery_responses, &check)?;
        for result in recovery_map.values() {
            check()?;
            validate_session(result)?;
        }

        // Validate stored results + overlay
        let mut expected = 0u64;
        let mut stored_completed = 0u64;
        let mut completed = 0u64;
        let mut baseline_completed = false;
        let mut stored_outcomes = BTreeMap::new();
        for (&ordinal, row) in &self.results {
            check()?;
            let result = self.result_summary(ordinal)?.expect("row exists");
            if let IntruderResultOutcome::Flow(summary) = &result.outcome {
                let flow = &self.flows[&summary.id];
                let _: FlowSummary = strict_json(&flow.summary_json)?;
            }
            ensure!(
                result.ordinal == expected && expected < stored.dispatched,
                "Intruder 原始结果序号缺失或与计数不符"
            );
            ensure!(
                !stored.state.is_terminal() || intruder_mod::outcome_terminal(&result.outcome),
                "Intruder 原始终态运行仍引用在途结果"
            );
            stored_completed += u64::from(intruder_mod::outcome_terminal(&result.outcome));
            validate_session(&result)?;
            let latest_item = recovery_map.get(&expected).copied().unwrap_or(&result);
            if latest_item != &result {
                ensure!(
                    !intruder_mod::outcome_terminal(&result.outcome),
                    "Intruder 恢复覆盖已完成结果"
                );
                match (&result.outcome, &latest_item.outcome) {
                    (
                        IntruderResultOutcome::Flow(previous),
                        IntruderResultOutcome::Flow(next),
                    ) => {
                        ensure!(
                            previous.id == next.id && next.revision > previous.revision,
                            "Intruder 恢复 flow 身份或 revision 不一致"
                        );
                    }
                    _ => bail!("Intruder 恢复结果类型不一致"),
                }
            }
            let terminal = intruder_mod::outcome_terminal(&latest_item.outcome);
            ensure!(
                !run.state.is_terminal() || terminal,
                "Intruder 终态运行仍引用在途结果"
            );
            completed += u64::from(terminal);
            if run
                .baseline
                .is_some_and(|baseline| baseline.ordinal == latest_item.ordinal)
            {
                baseline_completed = terminal;
            }
            stored_outcomes.insert(ordinal, (result, row));
            expected += 1;
        }
        ensure!(
            expected == stored.dispatched && stored_completed == stored.completed,
            "Intruder 原始结果缺失或完成计数不符"
        );
        while expected < run.dispatched {
            check()?;
            let result = recovery_map
                .get(&expected)
                .copied()
                .context("Intruder 恢复结果序号缺失")?;
            let terminal = intruder_mod::outcome_terminal(&result.outcome);
            ensure!(
                !run.state.is_terminal() || terminal,
                "Intruder 终态运行仍引用在途结果"
            );
            completed += u64::from(terminal);
            if run
                .baseline
                .is_some_and(|baseline| baseline.ordinal == result.ordinal)
            {
                baseline_completed = terminal;
            }
            expected += 1;
        }
        ensure!(
            completed == run.completed
                && run
                    .baseline
                    .is_none_or(|baseline| baseline.completed == baseline_completed),
            "Intruder 恢复结果完成计数或基准状态不符"
        );

        // Filter & sort matching ordinals
        let matcher = intruder_mod::query::search_matcher(&query.filter, &check)?;
        let need_grep_filter = query.filter.only_grep_matched && !query.grep_config.is_empty();
        let is_grep_sort = matches!(
            query.sort.column,
            crate::IntruderResultSortColumn::GrepMatch(_)
                | crate::IntruderResultSortColumn::GrepExtract(_)
                | crate::IntruderResultSortColumn::GrepPayload(_)
        );
        let need_grep = need_grep_filter || (is_grep_sort && !query.grep_config.is_empty());
        let baseline = run.baseline.map(|b| b.ordinal);
        let admitted = baseline.filter(|&ordinal| ordinal == run.dispatched);
        let enabled = query.filter.status_classes;

        let mut candidates = Vec::new();
        let number_for = |ordinal: u64| -> u64 {
            if Some(ordinal) == baseline {
                0
            } else {
                ordinal + 1 - u64::from(baseline.is_some_and(|b| ordinal > b))
            }
        };
        let status_allowed = |status: Option<u16>| -> bool {
            match status {
                None => true,
                Some(_) if enabled == [true, true, true, true] => true,
                Some(200..=299) => enabled[0],
                Some(300..=399) => enabled[1],
                Some(400..=499) => enabled[2],
                Some(500..=599) => enabled[3],
                Some(_) => false,
            }
        };

        for ordinal in 0..run.dispatched {
            check()?;
            let (status, flow_id, response_bytes, response_received, has_notes, has_highlight) =
                if let Some(rec) = recovery_map.get(&ordinal) {
                    match &rec.outcome {
                        IntruderResultOutcome::Flow(s) => (
                            s.status,
                            Some(s.id.to_string()),
                            s.response_bytes,
                            s.status.map(|_| s.elapsed_ms),
                            false,
                            false,
                        ),
                        IntruderResultOutcome::Rejected(_) => (None, None, 0, None, false, false),
                    }
                } else if let Some((stored_res, row)) = stored_outcomes.get(&ordinal) {
                    let has_notes = row.notes.as_ref().is_some_and(|n| !n.is_empty());
                    let has_highlight = row.highlight.is_some_and(|h| h > 0);
                    match &stored_res.outcome {
                        IntruderResultOutcome::Flow(s) => (
                            s.status,
                            Some(s.id.to_string()),
                            s.response_bytes,
                            s.status.map(|_| s.elapsed_ms),
                            has_notes,
                            has_highlight,
                        ),
                        IntruderResultOutcome::Rejected(_) => {
                            (None, None, 0, None, has_notes, has_highlight)
                        }
                    }
                } else {
                    continue;
                };
            if status_allowed(status)
                && (!query.filter.only_with_notes || has_notes)
                && (!query.filter.only_highlighted || has_highlight)
            {
                candidates.push(intruder_mod::query::MatchedResult {
                    ordinal,
                    status,
                    number: number_for(ordinal),
                    payload_index: None,
                    flow: flow_id,
                    response_bytes,
                    response_received,
                    grep_matches: Vec::new(),
                    grep_extracts: Vec::new(),
                    grep_payload_hits: Vec::new(),
                });
            }
        }
        if let Some(ordinal) = admitted
            && !query.filter.only_with_notes
            && !query.filter.only_highlighted
        {
            candidates.push(intruder_mod::query::MatchedResult {
                ordinal,
                status: None,
                number: number_for(ordinal),
                payload_index: None,
                flow: None,
                response_bytes: 0,
                response_received: None,
                grep_matches: Vec::new(),
                grep_extracts: Vec::new(),
                grep_payload_hits: Vec::new(),
            });
        }
        candidates.sort_by_key(|c| c.number);

        let (total, selected_rank, ordinals) = if matcher.is_some() || need_grep {
            let payload_order = matches!(
                query.sort.column,
                crate::IntruderResultSortColumn::Payload
            )
            .then(|| self.payload_order(run, &check))
            .transpose()?;
            let mut payload_map = BTreeMap::new();
            if need_grep && query.grep_config.grep_payloads && !candidates.is_empty() {
                let all_ordinals: Vec<u64> = candidates.iter().map(|c| c.ordinal).collect();
                let mut idx = 0usize;
                while idx < all_ordinals.len() {
                    let chunk = &all_ordinals[idx..(idx + 500).min(all_ordinals.len())];
                    let batch = self.payload_summaries(run, chunk, &check)?;
                    ensure!(!batch.is_empty(), "Intruder payload 单行超过分页限额");
                    idx += batch.len();
                    for row in batch {
                        payload_map.insert(row.ordinal, row.payloads);
                    }
                }
            }
            let mut filtered = Vec::new();
            for mut candidate in candidates {
                check()?;
                if let Some(expected_status) = candidate.status {
                    let raw = if let Some(&response) = recovery_responses.get(&candidate.ordinal) {
                        ensure!(
                            response.status == expected_status
                                && response.body.len() as u64 == candidate.response_bytes,
                            "Intruder 恢复响应状态或长度不一致"
                        );
                        crate::response_to_raw(response)
                    } else {
                        let flow_str = candidate
                            .flow
                            .as_deref()
                            .context("Intruder 已完成响应缺少关联 flow")?;
                        let flow_id = Uuid::parse_str(flow_str)?;
                        self.read_search_response(
                            flow_id,
                            expected_status,
                            candidate.response_bytes,
                            &check,
                            #[cfg(test)]
                            body_reads,
                        )?
                    };
                    check()?;
                    if let Some(matcher) = &matcher {
                        let matched = matcher.is_match(&raw);
                        check()?;
                        if matched == query.filter.negative_search {
                            continue;
                        }
                    }
                    if need_grep {
                        let payload_slices: Vec<&[u8]> = payload_map
                            .get(&candidate.ordinal)
                            .map(|list| {
                                list.iter()
                                    .map(|item| item.preview_bytes.as_slice())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let (grep_matches, grep_extracts, grep_payload_hits) =
                            crate::intruder::evaluate_grep_row(
                                &raw,
                                &query.grep_config,
                                &payload_slices,
                            );
                        if need_grep_filter
                            && grep_matches.is_empty()
                            && grep_extracts.is_empty()
                            && !grep_payload_hits.iter().any(|&hit| hit)
                        {
                            continue;
                        }
                        candidate.grep_matches = grep_matches;
                        candidate.grep_extracts = grep_extracts;
                        candidate.grep_payload_hits = grep_payload_hits;
                    }
                    check()?;
                } else {
                    if matcher.is_some() && !query.filter.negative_search {
                        continue;
                    }
                    if need_grep_filter {
                        continue;
                    }
                }
                if let Some(order) = &payload_order {
                    candidate.payload_index = order.request_index(candidate.ordinal)?;
                }
                filtered.push(candidate);
            }
            if query.sort.column == crate::IntruderResultSortColumn::Length {
                let mut lengths = Vec::with_capacity(filtered.len());
                for row in &filtered {
                    check()?;
                    let length = self.response_length(
                        recovery_responses,
                        row.ordinal,
                        row.status,
                        row.response_bytes,
                        row.flow.as_deref(),
                        &check,
                    )?;
                    lengths.push((row.ordinal, row.number, length));
                }
                intruder_mod::query::sort_by_length(&mut lengths, query.sort.ascending, &check)?;
                let ordinals = lengths.into_iter().map(|(ordinal, _, _)| ordinal).collect();
                intruder_mod::query::page_from_ordinals(query, ordinals)
            } else {
                intruder_mod::query::sort_filtered_rows(
                    &mut filtered,
                    query.sort,
                    &query.grep_config,
                    payload_order.as_ref(),
                    &check,
                )?;
                let ordinals = filtered.into_iter().map(|row| row.ordinal).collect();
                intruder_mod::query::page_from_ordinals(query, ordinals)
            }
        } else if matches!(query.sort.column, crate::IntruderResultSortColumn::Payload) {
            let order = self.payload_order(run, &check)?;
            let mut matching = Vec::with_capacity(candidates.len());
            for c in candidates {
                check()?;
                matching.push((c.ordinal, c.number, order.request_index(c.ordinal)?));
            }
            let mut comparisons = 0usize;
            let mut aborted = None;
            matching.sort_by(|left, right| {
                if aborted.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                comparisons = comparisons.wrapping_add(1);
                if comparisons.is_multiple_of(1024)
                    && let Err(error) = check()
                {
                    aborted = Some(error);
                    return std::cmp::Ordering::Equal;
                }
                order
                    .compare_indices(left.2, right.2, query.sort.ascending)
                    .then_with(|| left.1.cmp(&right.1))
            });
            if let Some(error) = aborted {
                return Err(error);
            }
            check()?;
            let ordinals = matching.into_iter().map(|(ordinal, _, _)| ordinal).collect();
            intruder_mod::query::page_from_ordinals(query, ordinals)
        } else if matches!(query.sort.column, crate::IntruderResultSortColumn::Length) {
            let mut lengths = Vec::with_capacity(candidates.len());
            for c in candidates {
                check()?;
                let length = self.response_length(
                    recovery_responses,
                    c.ordinal,
                    c.status,
                    c.response_bytes,
                    c.flow.as_deref(),
                    &check,
                )?;
                lengths.push((c.ordinal, c.number, length));
            }
            intruder_mod::query::sort_by_length(&mut lengths, query.sort.ascending, &check)?;
            let ordinals = lengths.into_iter().map(|(ordinal, _, _)| ordinal).collect();
            intruder_mod::query::page_from_ordinals(query, ordinals)
        } else {
            let asc = query.sort.ascending;
            match query.sort.column {
                crate::IntruderResultSortColumn::Request => {
                    if !asc {
                        candidates.reverse();
                    }
                }
                crate::IntruderResultSortColumn::Status => {
                    let mut sort_error: Option<anyhow::Error> = None;
                    let mut comparisons = 0usize;
                    candidates.sort_by(|a, b| {
                        if sort_error.is_some() {
                            return std::cmp::Ordering::Equal;
                        }
                        comparisons += 1;
                        if comparisons.is_multiple_of(1024)
                            && let Err(err) = check()
                        {
                            sort_error = Some(err);
                            return std::cmp::Ordering::Equal;
                        }
                        let ord = match (a.status, b.status) {
                            (None, None) => std::cmp::Ordering::Equal,
                            (None, Some(_)) => {
                                if asc {
                                    std::cmp::Ordering::Less
                                } else {
                                    std::cmp::Ordering::Greater
                                }
                            }
                            (Some(_), None) => {
                                if asc {
                                    std::cmp::Ordering::Greater
                                } else {
                                    std::cmp::Ordering::Less
                                }
                            }
                            (Some(l), Some(r)) => {
                                let o = l.cmp(&r);
                                if asc { o } else { o.reverse() }
                            }
                        };
                        ord.then_with(|| a.number.cmp(&b.number))
                    });
                    if let Some(err) = sort_error {
                        return Err(err);
                    }
                }
                crate::IntruderResultSortColumn::ResponseReceived => {
                    let mut sort_error: Option<anyhow::Error> = None;
                    let mut comparisons = 0usize;
                    candidates.sort_by(|a, b| {
                        if sort_error.is_some() {
                            return std::cmp::Ordering::Equal;
                        }
                        comparisons += 1;
                        if comparisons.is_multiple_of(1024)
                            && let Err(err) = check()
                        {
                            sort_error = Some(err);
                            return std::cmp::Ordering::Equal;
                        }
                        let o = a.response_received.cmp(&b.response_received);
                        let ord = if asc { o } else { o.reverse() };
                        ord.then_with(|| a.number.cmp(&b.number))
                    });
                    if let Some(err) = sort_error {
                        return Err(err);
                    }
                }
                crate::IntruderResultSortColumn::GrepMatch(_)
                | crate::IntruderResultSortColumn::GrepExtract(_)
                | crate::IntruderResultSortColumn::GrepPayload(_) => {}
                crate::IntruderResultSortColumn::Payload
                | crate::IntruderResultSortColumn::Length => unreachable!(),
            }
            let ordinals = candidates.into_iter().map(|c| c.ordinal).collect();
            intruder_mod::query::page_from_ordinals(query, ordinals)
        };

        check()?;
        let mut requested = Vec::with_capacity(ordinals.len() + 1);
        if selected_rank.is_some() {
            requested.push(query.selected_ordinal.expect("matching selection"));
        }
        requested.extend(ordinals.iter().copied().filter(|ordinal| {
            selected_rank.is_none() || Some(*ordinal) != query.selected_ordinal
        }));
        let mut payloads = if requested.is_empty() {
            BTreeMap::new()
        } else {
            self.payload_summaries(run, &requested, &check)?
                .into_iter()
                .map(|row| (row.ordinal, row.payloads))
                .collect()
        };
        let mut bytes = serde_json::to_vec(run)?.len() + 128;
        ensure!(bytes < PAGE_BYTES, "Intruder 运行摘要超过分页限额");
        let mut selected_row = None;
        if selected_rank.is_some() {
            let ordinal = query.selected_ordinal.expect("matching selection");
            let row = self
                .build_view_row(
                    run,
                    &recovery_map,
                    recovery_responses,
                    &stored_outcomes,
                    ordinal,
                    &mut payloads,
                    &query.grep_config,
                    &check,
                    #[cfg(test)]
                    body_reads,
                )?
                .context("Intruder 选中结果 payload 超过分页限额")?;
            bytes = bytes
                .checked_add(intruder_mod::query::view_row_bytes(&row)?)
                .context("Intruder 分页长度溢出")?;
            ensure!(bytes <= PAGE_BYTES, "Intruder 选中结果超过 1 MiB 分页限额");
            selected_row = Some(row);
        }
        let mut rows = Vec::with_capacity(ordinals.len());
        for ordinal in ordinals {
            check()?;
            if selected_row
                .as_ref()
                .is_some_and(|row| row.ordinal == ordinal)
            {
                rows.push(selected_row.take().expect("selected row"));
                continue;
            }
            let Some(row) = self.build_view_row(
                run,
                &recovery_map,
                recovery_responses,
                &stored_outcomes,
                ordinal,
                &mut payloads,
                &query.grep_config,
                &check,
                #[cfg(test)]
                body_reads,
            )?
            else {
                break;
            };
            let row_bytes = intruder_mod::query::view_row_bytes(&row)?;
            ensure!(
                row_bytes <= PAGE_BYTES,
                "Intruder 单条结果超过 1 MiB 分页限额"
            );
            if row_bytes > PAGE_BYTES - bytes {
                break;
            }
            bytes += row_bytes;
            rows.push(row);
        }
        ensure!(
            !rows.is_empty() || query.offset >= total,
            "Intruder 当前页与选中结果合计超过 1 MiB；未返回不完整结果"
        );
        let next = query
            .offset
            .checked_add(rows.len())
            .context("Intruder 分页位置溢出")?;
        check()?;
        Ok(IntruderResultViewPage {
            run: run.clone(),
            offset: query.offset,
            total,
            rows,
            next_offset: (next < total).then_some(next),
            selected_rank,
            selected_row,
            persisted: false,
        })
    }

    fn read_response_head(
        &self,
        flow_id: FlowId,
        expected_status: u16,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Option<store::OwnedResponseHead>> {
        check()?;
        let Some((head_bytes, _)) = self.flows.get(&flow_id).and_then(|f| f.messages.get(&1))
        else {
            return Ok(None);
        };
        ensure!(
            head_bytes.len() <= intruder_mod::query::MAX_SEARCH_HEAD_BYTES,
            "Intruder 结果响应头超过 128 KiB 限额"
        );
        let head: store::OwnedResponseHead = strict_json(head_bytes)?;
        ensure!(
            head.stream.is_none() && head.status == expected_status,
            "Intruder 结果响应状态或流式元数据无效"
        );
        check()?;
        Ok(Some(head))
    }

    fn read_search_response(
        &self,
        flow_id: FlowId,
        expected_status: u16,
        expected_bytes: u64,
        check: &dyn Fn() -> Result<()>,
        #[cfg(test)] body_reads: &std::sync::atomic::AtomicUsize,
    ) -> Result<Vec<u8>> {
        check()?;
        let (head_bytes, body) = self
            .flows
            .get(&flow_id)
            .and_then(|f| f.messages.get(&1))
            .context("Intruder 结果响应消息缺失")?;
        #[cfg(test)]
        body_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ensure!(
            head_bytes.len() <= intruder_mod::query::MAX_SEARCH_HEAD_BYTES
                && body.len() <= MAX_BODY
                && body.len() as u64 == expected_bytes,
            "Intruder 结果响应消息类型或长度无效"
        );
        let head: store::OwnedResponseHead = strict_json(head_bytes)?;
        ensure!(
            head.stream.is_none() && head.status == expected_status,
            "Intruder 结果响应状态或流式元数据无效"
        );
        check()?;
        Ok(crate::response_to_raw(&crate::HttpResponse {
            status: head.status,
            version: head.version,
            reason_phrase: head.reason_phrase,
            headers: head.headers,
            trailers: head.trailers,
            body: body.clone(),
        }))
    }

    fn response_length(
        &self,
        recovery_responses: &BTreeMap<u64, &crate::HttpResponse>,
        ordinal: u64,
        status: Option<u16>,
        response_bytes: u64,
        flow: Option<&str>,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Option<u64>> {
        let Some(status) = status else {
            return Ok(None);
        };
        if let Some(&response) = recovery_responses.get(&ordinal) {
            ensure!(
                response.status == status && response.body.len() as u64 == response_bytes,
                "Intruder 恢复响应状态或长度不一致"
            );
            return Ok(Some(crate::http::intruder_response_len(
                response.version,
                response.status,
                response.reason_phrase.as_deref(),
                &response.headers,
                response.body.len() as u64,
            )));
        }
        let flow_id = Uuid::parse_str(flow.context("Intruder 已完成响应缺少关联 flow")?)?;
        let Some(head) = self.read_response_head(flow_id, status, check)? else {
            bail!("Intruder 结果响应消息缺失");
        };
        Ok(Some(crate::http::intruder_response_len(
            head.version,
            head.status,
            head.reason_phrase.as_deref(),
            &head.headers,
            response_bytes,
        )))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_view_row(
        &self,
        run: &IntruderRunSummary,
        recovery: &BTreeMap<u64, &IntruderResultSummary>,
        recovery_responses: &BTreeMap<u64, &crate::HttpResponse>,
        stored_outcomes: &BTreeMap<u64, (IntruderResultSummary, &SessionResultRow)>,
        ordinal: u64,
        payloads: &mut BTreeMap<u64, Vec<intruder_mod::result_view::PayloadSummary>>,
        grep_config: &crate::IntruderGrepConfig,
        check: &dyn Fn() -> Result<()>,
        #[cfg(test)] body_reads: &std::sync::atomic::AtomicUsize,
    ) -> Result<Option<IntruderResultViewRow>> {
        let Some(payloads) = payloads.remove(&ordinal) else {
            return Ok(None);
        };
        let result = if let Some(&result) = recovery.get(&ordinal) {
            Some(result.clone())
        } else if ordinal == run.dispatched
            && run
                .baseline
                .is_some_and(|baseline| baseline.ordinal == ordinal)
        {
            None
        } else {
            Some(
                stored_outcomes
                    .get(&ordinal)
                    .map(|(r, _)| r.clone())
                    .context("Intruder 页面结果缺失")?,
            )
        };
        let (response_length, grep_matches, grep_extracts, grep_payload_hits) = match result.as_ref()
        {
            Some(IntruderResultSummary {
                outcome: IntruderResultOutcome::Flow(summary),
                ..
            }) if summary.status.is_some() => {
                let flow_str = summary.id.to_string();
                let length = self.response_length(
                    recovery_responses,
                    ordinal,
                    summary.status,
                    summary.response_bytes,
                    Some(&flow_str),
                    check,
                )?;
                if grep_config.is_empty() {
                    (length, Vec::new(), Vec::new(), Vec::new())
                } else {
                    let expected_status = summary.status.expect("status checked");
                    let raw = if let Some(&response) = recovery_responses.get(&ordinal) {
                        ensure!(
                            response.status == expected_status
                                && response.body.len() as u64 == summary.response_bytes,
                            "Intruder 恢复响应状态或长度不一致"
                        );
                        crate::response_to_raw(response)
                    } else {
                        self.read_search_response(
                            summary.id,
                            expected_status,
                            summary.response_bytes,
                            check,
                            #[cfg(test)]
                            body_reads,
                        )?
                    };
                    let payload_slices: Vec<&[u8]> = payloads
                        .iter()
                        .map(|payload| payload.preview_bytes.as_slice())
                        .collect();
                    let (matches, extracts, hits) =
                        crate::intruder::evaluate_grep_row(&raw, grep_config, &payload_slices);
                    (length, matches, extracts, hits)
                }
            }
            _ => (None, Vec::new(), Vec::new(), Vec::new()),
        };
        let (notes, notes_revision, highlight, highlight_revision) =
            if result.is_none() || recovery.contains_key(&ordinal) {
                (None, 0, None, 0)
            } else {
                let row = stored_outcomes
                    .get(&ordinal)
                    .map(|(_, row)| *row)
                    .context("Intruder 结果注解行缺失")?;
                let preview = row
                    .notes
                    .as_deref()
                    .filter(|n| n.len() <= intruder_mod::MAX_ERROR_BYTES);
                (
                    preview,
                    row.notes_revision,
                    row.highlight,
                    row.highlight_revision,
                )
            };
        let notes_preview = notes
            .map(|bytes| {
                let text = String::from_utf8_lossy(bytes);
                let cut = text.floor_char_boundary(256.min(text.len()));
                text[..cut].to_owned()
            })
            .unwrap_or_default();
        Ok(Some(IntruderResultViewRow {
            ordinal,
            result,
            payloads: payloads
                .into_iter()
                .map(|payload| IntruderPayloadPreview {
                    set_index: payload.selected.set_index,
                    payload_index: payload.selected.payload_index,
                    raw_bytes: payload.raw_bytes,
                    prefix: payload.preview_bytes,
                })
                .collect(),
            response_length,
            notes_revision,
            notes_preview,
            highlight,
            highlight_revision,
            grep_matches,
            grep_extracts,
            grep_payload_hits,
        }))
    }
}
