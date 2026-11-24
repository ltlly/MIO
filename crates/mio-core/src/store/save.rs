use super::*;

pub(in crate::store) fn workspace_arc_in_tx(
    cache: &std::sync::Mutex<StoreCache>,
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Arc<Workspace>> {
    let data_version: i64 =
        transaction.pragma_query_value(None, "data_version", |row| row.get(0))?;
    let schema_version: i64 =
        transaction.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    let total_changes = query_total_changes(transaction)?;
    if let Ok(guard) = cache.lock()
        && let Some(cached) = guard.workspace.as_ref()
        && cached.data_version == data_version
        && cached.schema_version == schema_version
        && cached.total_changes == total_changes
    {
        return Ok(Arc::clone(&cached.workspace));
    }
    let workspace = Arc::new(metadata::workspace(transaction)?);
    if let Ok(mut guard) = cache.lock() {
        guard.workspace = Some(WorkspaceCacheEntry {
            data_version,
            schema_version,
            total_changes,
            workspace: Arc::clone(&workspace),
        });
    }
    Ok(workspace)
}

impl<S: std::ops::Deref<Target = ProjectStore>> MetadataRepo<S> {
    pub fn workspace(&self) -> Result<Workspace> {
        self.workspace_arc().map(|ws| (*ws).clone())
    }

    pub(crate) fn workspace_arc(&self) -> Result<Arc<Workspace>> {
        let data_version: i64 =
            self.0
                .connection
                .pragma_query_value(None, "data_version", |row| row.get(0))?;
        let schema_version: i64 =
            self.0
                .connection
                .pragma_query_value(None, "schema_version", |row| row.get(0))?;
        let total_changes = query_total_changes(&self.0.connection)?;
        if let Ok(guard) = self.0.cache.lock()
            && let Some(cached) = guard.workspace.as_ref()
            && cached.data_version == data_version
            && cached.schema_version == schema_version
            && cached.total_changes == total_changes
        {
            return Ok(Arc::clone(&cached.workspace));
        }
        let transaction = self.0.connection.unchecked_transaction()?;
        let tx_dv: i64 =
            transaction.pragma_query_value(None, "data_version", |row| row.get(0))?;
        let tx_sv: i64 =
            transaction.pragma_query_value(None, "schema_version", |row| row.get(0))?;
        let tx_tc = query_total_changes(&transaction)?;
        let workspace = Arc::new(metadata::workspace(&transaction)?);
        transaction.commit()?;
        if let Ok(mut guard) = self.0.cache.lock() {
            guard.workspace = Some(WorkspaceCacheEntry {
                data_version: tx_dv,
                schema_version: tx_sv,
                total_changes: tx_tc,
                workspace: Arc::clone(&workspace),
            });
        }
        Ok(workspace)
    }
}

impl MetadataRepo<&mut ProjectStore> {
    pub fn save_workspace(&mut self, workspace: &Workspace) -> Result<()> {
        self.0.intruder().validate_session_workspace(workspace)?;
        let transaction = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        write_workspace(&transaction, workspace)?;
        transaction.commit()?;
        self.0.update_workspace_cache(workspace);
        Ok(())
    }

    pub(crate) fn reconcile_ipc_workspace(&mut self, workspace: &Workspace) -> Result<()> {
        self.0.intruder().validate_session_workspace(workspace)?;
        let transaction = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        write_workspace_with_mode(&transaction, workspace, true)?;
        transaction.commit()?;
        self.0.update_workspace_cache(workspace);
        Ok(())
    }
}

impl ProjectStore {
    pub(in crate::store) fn invalidate_workspace_cache(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.workspace = None;
        }
    }

    pub(in crate::store) fn update_workspace_cache(&self, workspace: &Workspace) {
        let versions = (|| -> Result<(i64, i64, i64)> {
            let dv: i64 =
                self.connection
                    .pragma_query_value(None, "data_version", |row| row.get(0))?;
            let sv: i64 =
                self.connection
                    .pragma_query_value(None, "schema_version", |row| row.get(0))?;
            let tc = query_total_changes(&self.connection)?;
            Ok((dv, sv, tc))
        })();
        if let Ok(mut guard) = self.cache.lock() {
            guard.workspace = match versions {
                Ok((data_version, schema_version, total_changes)) => Some(WorkspaceCacheEntry {
                    data_version,
                    schema_version,
                    total_changes,
                    workspace: Arc::new(workspace.clone()),
                }),
                Err(_) => None,
            };
        }
    }

    pub fn save(&mut self, destination: Option<&Path>) -> Result<(PathBuf, u64)> {
        self.save_with(destination, None, &[], &[], &[], &CancellationToken::new())
    }

    pub fn check_path_outside_temporary(&self, path: &Path) -> Result<()> {
        if let Some(temporary) = &self.temporary {
            check_outside_owned_directory(path, temporary.path())?;
        }
        Ok(())
    }

    pub fn save_with(
        &mut self,
        destination: Option<&Path>,
        workspace: Option<&Workspace>,
        records: &[CapturedFlow],
        websocket_records: &[CapturedWebSocket],
        intruder_mutations: &[IntruderMutation],
        cancel: &CancellationToken,
    ) -> Result<(PathBuf, u64)> {
        self.save_with_notes(
            destination,
            (workspace, &[], &[], &[]),
            records,
            websocket_records,
            intruder_mutations,
            cancel,
        )
        .map(|(path, bytes, _)| (path, bytes))
    }

    pub(crate) fn save_with_notes(
        &mut self,
        destination: Option<&Path>,
        edits: ProjectEdits<'_>,
        records: &[CapturedFlow],
        websocket_records: &[CapturedWebSocket],
        intruder_mutations: &[IntruderMutation],
        cancel: &CancellationToken,
    ) -> Result<ProjectSaveReceipt> {
        ensure!(!cancel.is_cancelled(), "项目保存已取消");
        let (workspace, notes, http_notes, highlights) = edits;
        websocket_notes::validate_changes(notes)?;
        ext_archive::http_notes::validate_changes(http_notes)?;
        ext_archive::http_highlights::validate_changes(highlights)?;
        for capture in records {
            self.intruder()
                .reject_session_flow_write(capture.record.summary.id)?;
        }
        for mutation in intruder_mutations {
            if let IntruderMutation::Result {
                result: Ok(record), ..
            } = mutation
            {
                self.intruder()
                    .reject_session_flow_write(record.summary.id)?;
            }
        }
        if let Some(workspace) = workspace {
            self.intruder().validate_session_workspace(workspace)?;
        }
        let receipts = if let Some(destination) =
            destination.filter(|p| self.path.as_deref() != Some(p))
        {
            let (reopened, receipts) = self.stage_backup_and_publish(
                destination,
                cancel,
                "项目保存已取消",
                "项目快照未完成",
                &format!("快照已保存在 {}，但无法切换到新项目", destination.display()),
                |staged| {
                    staged.apply_pending_with_notes(
                        edits,
                        records,
                        websocket_records,
                        intruder_mutations,
                        cancel,
                    )
                },
            )?;
            self.adopt_snapshot(reopened)?;
            receipts
        } else {
            ensure!(self.path.is_some(), "临时项目需要选择保存路径");
            // FULL WAL commits are durable; a reader delaying checkpoint is not a failed save.
            let _: (i32, i32, i32) =
                self.connection
                    .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?;
            self.apply_pending_with_notes(
                edits,
                records,
                websocket_records,
                intruder_mutations,
                cancel,
            )?
        };
        let path = self.path.clone().context("临时项目需要选择保存路径")?;
        Ok((path, self.total_bytes()?, receipts))
    }

    pub(super) fn stage_backup_and_publish<T>(
        &self,
        destination: &Path,
        cancel: &CancellationToken,
        cancel_msg: &str,
        backup_err_msg: &str,
        open_err_msg: &str,
        mutate: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<(Self, T)> {
        ensure!(
            !crate::is_external_archive_file(destination),
            "不能覆盖外部归档项目文件"
        );
        check_new_project_path(destination)?;
        self.check_path_outside_temporary(destination)?;
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let temporary = NamedTempFile::new_in(parent)?;
        let mut snapshot = Connection::open(temporary.path())?;
        {
            let backup = rusqlite::backup::Backup::new(&self.connection, &mut snapshot)?;
            loop {
                ensure!(!cancel.is_cancelled(), "{cancel_msg}");
                let step = backup.step(64)?;
                ensure!(!cancel.is_cancelled(), "{cancel_msg}");
                match step {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => {}
                    _ => bail!("{backup_err_msg}"),
                }
            }
        }
        snapshot.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;",
        )?;
        let mut staged = Self {
            cache: Mutex::new(StoreCache::default()),
            connection: snapshot,
            id: self.id,
            path: Some(temporary.path().to_owned()),
            temporary: None,
            intruder_sessions: intruder::IntruderSessions::default(),
        };
        staged.cancel_queries_with(cancel.clone());
        let output = mutate(&mut staged)?;
        ensure!(!cancel.is_cancelled(), "{cancel_msg}");
        staged.clear_connection_cache();
        staged.connection.close().map_err(|(_, err)| err)?;
        temporary.as_file().sync_all()?;
        check_new_project_path(destination)?;
        self.check_path_outside_temporary(destination)?;
        // A successful publish wins cancellation arriving after this check.
        ensure!(!cancel.is_cancelled(), "{cancel_msg}");
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| {
                format!(
                    "完整快照已发布至 {}，但目录同步失败，不能确认掉电持久性；原项目仍在使用",
                    destination.display()
                )
            })?;
        let reopened = Self::open(destination).with_context(|| open_err_msg.to_owned())?;
        Ok((reopened, output))
    }

    pub(super) fn adopt_snapshot(&mut self, mut reopened: Self) -> Result<()> {
        ensure!(
            reopened.id == self.id,
            "另存为的项目 ID 已改变；原项目与 Session 数据保留"
        );
        self.intruder().validate_session_snapshot(&reopened)?;
        reopened.intruder_sessions = std::mem::take(&mut self.intruder_sessions);
        *self = reopened;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn apply_pending(
        &mut self,
        workspace: Option<&Workspace>,
        records: &[CapturedFlow],
        websocket_records: &[CapturedWebSocket],
        intruder_mutations: &[IntruderMutation],
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.apply_pending_with_notes(
            (workspace, &[], &[], &[]),
            records,
            websocket_records,
            intruder_mutations,
            cancel,
        )
        .map(|_| ())
    }

    pub(super) fn apply_pending_with_notes(
        &mut self,
        edits: ProjectEdits<'_>,
        records: &[CapturedFlow],
        websocket_records: &[CapturedWebSocket],
        intruder_mutations: &[IntruderMutation],
        cancel: &CancellationToken,
    ) -> Result<NotesSaveReceipt> {
        ensure!(!cancel.is_cancelled(), "项目保存已取消");
        let (workspace, notes, http_notes, highlights) = edits;
        if workspace.is_none()
            && notes.is_empty()
            && http_notes.is_empty()
            && highlights.is_empty()
            && records.is_empty()
            && websocket_records.is_empty()
            && intruder_mutations.is_empty()
        {
            return Ok((Vec::new(), Vec::new(), Vec::new()));
        }
        let before_tc = query_total_changes(&self.connection)?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if !notes.is_empty() || !http_notes.is_empty() || !highlights.is_empty() {
            let schema: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
            ensure!(schema == SCHEMA, "项目注解版本已改变；未保存");
            schema_guard::validate(&tx)?;
            ensure!(
                metadata::project_id(&tx)? == self.id,
                "项目注解 ID 已改变；未保存"
            );
        }
        if let Some(workspace) = workspace {
            write_workspace(&tx, workspace)?;
        }
        let mut remaining = records;
        while let Some((capture, tail)) = remaining.split_first() {
            if let Some((next, after)) = tail.split_first()
                && capture.response_stream.is_none()
                && next.response_stream.is_none()
                && validate_repeater_completion(&capture.record, &next.record).is_ok()
            {
                write_repeater_completion(&tx, &capture.record, &next.record)?;
                remaining = after;
            } else {
                super::messages::write_capture_with_metrics(
                    &tx,
                    &capture.record,
                    capture.response_stream.as_ref(),
                    capture.listener_port,
                    &capture.capture_metrics,
                )?;
                remaining = tail;
            }
        }
        for record in websocket_records {
            websocket::write_record(&tx, record)?;
        }
        intruder::write_recovery(&tx, intruder_mutations)?;
        let mut receipts = Vec::with_capacity(notes.len());
        for note in notes {
            let saved = websocket_notes::write(
                &tx,
                note.message_id,
                note.expected_notes_revision,
                &note.text,
                cancel,
            )?;
            receipts.push((note.message_id, saved.revision));
        }
        let http_receipts = ext_archive::http_notes::write_changes(&tx, http_notes, cancel)?;
        let highlight_receipts = ext_archive::http_highlights::write_changes(&tx, highlights, cancel)?;
        if !records.is_empty() || !websocket_records.is_empty() {
            finish_pending_records(&tx, false, false)?;
        }
        ensure!(!cancel.is_cancelled(), "项目保存已取消");
        tx.commit()?;
        if let Some(workspace) = workspace {
            self.update_workspace_cache(workspace);
        } else if !records.is_empty() || !websocket_records.is_empty() {
            self.advance_workspace_cache_changes(before_tc)?;
        }
        Ok((receipts, http_receipts, highlight_receipts))
    }
}

pub(super) fn write_workspace(
    transaction: &rusqlite::Transaction<'_>,
    workspace: &Workspace,
) -> Result<()> {
    write_workspace_with_mode(transaction, workspace, false)
}

fn write_workspace_with_mode(
    transaction: &rusqlite::Transaction<'_>,
    workspace: &Workspace,
    allow_same_revision: bool,
) -> Result<()> {
    workspace.validate()?;
    ext_archive::validate_workspace_responses(transaction, workspace)?;
    repeater_history::validate_workspace(transaction, workspace)?;
    let previous = metadata::workspace_for_update(transaction)?;
    let revision_ok = previous.revision.checked_add(1) == Some(workspace.revision)
        || (allow_same_revision && previous.revision == workspace.revision);
    ensure!(revision_ok, "工作区已变更；保留当前草稿，请重新同步");
    for tab in workspace.tabs.iter().chain(&workspace.closed) {
        ensure!(
            !previous
                .intruder
                .tabs
                .iter()
                .chain(&previous.intruder.closed)
                .any(|old| old.id == tab.id),
            "工作标签不能更换所属工具"
        );
        if let Some(old) = previous
            .tabs
            .iter()
            .chain(&previous.closed)
            .find(|old| old.id == tab.id)
        {
            repeater_history::validate_change(transaction, workspace, old, tab)?;
            ensure!(
                tab.draft.revision >= old.draft.revision,
                "草稿 revision 已过期"
            );
            ensure!(
                tab.draft == old.draft || tab.draft.revision > old.draft.revision,
                "修改草稿必须更新 revision"
            );
        }
    }
    for tab in workspace
        .intruder
        .tabs
        .iter()
        .chain(&workspace.intruder.closed)
    {
        if let Some(id) = tab.last_run {
            intruder::ensure_not_discarded(transaction, id)?;
        }
        ensure!(
            !previous
                .tabs
                .iter()
                .chain(&previous.closed)
                .any(|old| old.id == tab.id),
            "工作标签不能更换所属工具"
        );
        if let Some(old) = previous
            .intruder
            .tabs
            .iter()
            .chain(&previous.intruder.closed)
            .find(|old| old.id == tab.id)
        {
            ensure!(
                tab.spec.revision >= old.spec.revision,
                "Intruder 配置 revision 已过期"
            );
            ensure!(
                tab.spec == old.spec || tab.spec.revision > old.spec.revision,
                "修改 Intruder 配置必须更新 revision"
            );
            ensure!(
                tab.spec.draft.revision >= old.spec.draft.revision,
                "Intruder 草稿 revision 已过期"
            );
            ensure!(
                tab.spec.draft == old.spec.draft
                    || tab.spec.draft.revision > old.spec.draft.revision,
                "修改 Intruder 草稿必须更新 revision"
            );
        }
    }
    transaction.execute(
        "UPDATE main.metadata SET value=?1 WHERE key='workspace'",
        [serde_json::to_vec(workspace)?],
    )?;
    Ok(())
}
