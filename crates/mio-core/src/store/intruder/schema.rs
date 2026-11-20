use super::*;

pub(in crate::store) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "CREATE TABLE intruder_runs (
            id TEXT PRIMARY KEY,
            tab TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK(revision>0),
            spec_revision INTEGER NOT NULL CHECK(spec_revision>0),
            total INTEGER NOT NULL CHECK(total BETWEEN 0 AND {MAX_REQUESTS}),
            dispatched INTEGER NOT NULL CHECK(dispatched BETWEEN 0 AND total),
            completed INTEGER NOT NULL CHECK(completed BETWEEN 0 AND dispatched),
            state INTEGER NOT NULL CHECK(state BETWEEN 0 AND 5),
            {PAUSED_COLUMN},
            error TEXT CHECK(error IS NULL OR (typeof(error)='text' AND length(CAST(error AS BLOB))<={MAX_ERROR_BYTES})),
            started INTEGER NOT NULL,
            {TITLE_COLUMN},
            {TITLE_REVISION_COLUMN},
            {BASELINE_COLUMN},
            {POOL_COLUMN},
            {POOL_REVISION_COLUMN},
            spec_meta BLOB NOT NULL CHECK(typeof(spec_meta)='blob' AND length(spec_meta)<={MAX_SPEC_META}),
            template BLOB NOT NULL CHECK(typeof(template)='blob' AND length(template)<={MAX_REQUEST_BYTES}),
            CHECK((state=5 AND error IS NOT NULL) OR (state!=5 AND error IS NULL))
        );
        CREATE INDEX intruder_runs_state ON intruder_runs(state,id);
        CREATE INDEX intruder_runs_started ON intruder_runs(started,id);
        CREATE TABLE intruder_payloads (
            run TEXT NOT NULL REFERENCES intruder_runs(id) ON DELETE CASCADE,
            set_index INTEGER NOT NULL CHECK(set_index BETWEEN 0 AND 19),
            ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 99999),
            value BLOB NOT NULL CHECK(typeof(value)='blob' AND length(value)<={MAX_PAYLOAD_BYTES}),
            PRIMARY KEY(run,set_index,ordinal)
        ) WITHOUT ROWID;
        CREATE TABLE intruder_results (
            run TEXT NOT NULL REFERENCES intruder_runs(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 999999),
            flow TEXT UNIQUE REFERENCES flows(id),
            error TEXT CHECK(error IS NULL OR (typeof(error)='text' AND length(CAST(error AS BLOB))<={MAX_ERROR_BYTES})), {RESULT_NOTES_COLUMN}, {RESULT_NOTES_REVISION_COLUMN}, {RESULT_HIGHLIGHT_COLUMN}, {RESULT_HIGHLIGHT_REVISION_COLUMN},
            PRIMARY KEY(run,ordinal),
            CHECK((flow IS NOT NULL AND error IS NULL) OR (flow IS NULL AND error IS NOT NULL))
        ) WITHOUT ROWID;"
    ))?;
    Ok(())
}

pub(in crate::store) fn add_result_annotation_columns(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "ALTER TABLE intruder_results ADD COLUMN {RESULT_NOTES_COLUMN};
         ALTER TABLE intruder_results ADD COLUMN {RESULT_NOTES_REVISION_COLUMN};
         ALTER TABLE intruder_results ADD COLUMN {RESULT_HIGHLIGHT_COLUMN};
         ALTER TABLE intruder_results ADD COLUMN {RESULT_HIGHLIGHT_REVISION_COLUMN};"
    ))?;
    Ok(())
}

pub(in crate::store) fn add_pause_column(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "ALTER TABLE intruder_runs ADD COLUMN {PAUSED_COLUMN};"
    ))?;
    Ok(())
}

pub(in crate::store) fn add_title_columns(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "ALTER TABLE intruder_runs ADD COLUMN {TITLE_COLUMN};
         ALTER TABLE intruder_runs ADD COLUMN {TITLE_REVISION_COLUMN};"
    ))?;
    Ok(())
}

pub(in crate::store) fn add_baseline_column(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "ALTER TABLE intruder_runs ADD COLUMN {BASELINE_COLUMN};"
    ))?;
    Ok(())
}

pub(in crate::store) fn add_resource_pool_columns(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!("ALTER TABLE intruder_runs ADD COLUMN {POOL_COLUMN}; ALTER TABLE intruder_runs ADD COLUMN {POOL_REVISION_COLUMN};"))?;
    Ok(())
}

#[allow(dead_code)]
pub(in crate::store::intruder) fn insert_intruder_run(
    transaction: &Transaction<'_>,
    id: Uuid,
    tab_id: TabId,
    spec: &IntruderSpec,
    started_unix_ms: i64,
    total: u64,
    resource_pool: Option<Uuid>,
) -> Result<IntruderRunSummary> {
    insert_intruder_run_with_settings(
        transaction,
        id,
        tab_id,
        spec,
        started_unix_ms,
        total,
        resource_pool,
        crate::IntruderAttackSettings::default(),
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::store::intruder) fn insert_intruder_run_with_settings(
    transaction: &Transaction<'_>,
    id: Uuid,
    tab_id: TabId,
    spec: &IntruderSpec,
    started_unix_ms: i64,
    total: u64,
    resource_pool: Option<Uuid>,
    attack_settings: crate::IntruderAttackSettings,
    payload_configs: Vec<crate::IntruderPayloadSetConfig>,
) -> Result<IntruderRunSummary> {
    let meta = serde_json::to_vec(&SpecMeta {
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
        meta.len() <= MAX_SPEC_META,
        "Intruder 配置元数据超过编码限额"
    );
    transaction.execute(
            "INSERT INTO intruder_runs(id,tab,revision,spec_revision,total,dispatched,completed,state,error,started,spec_meta,template,baseline_ordinal,pool_id,pool_revision)
             VALUES(?1,?2,1,?3,?4,0,0,0,NULL,?5,?6,?7,?8,?9,?10)",
            params![id.to_string(), tab_id.to_string(), spec.revision, total, started_unix_ms, meta, spec.draft.raw, spec.baseline_request.then_some(0), resource_pool.map(|id| id.to_string()), u64::from(resource_pool.is_some())],
        )?;
    {
        let mut insert = transaction.prepare(
            "INSERT INTO intruder_payloads(run,set_index,ordinal,value) VALUES(?1,?2,?3,?4)",
        )?;
        for (set_index, set) in spec.payload_sets.iter().enumerate() {
            for (ordinal, value) in set.iter().enumerate() {
                insert.execute(params![
                    id.to_string(),
                    i64::try_from(set_index)?,
                    i64::try_from(ordinal)?,
                    value
                ])?;
            }
        }
    }
    read_run(transaction, id)
}
