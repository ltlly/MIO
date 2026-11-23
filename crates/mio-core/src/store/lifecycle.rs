use super::*;

const STORE_BUSY_TIMEOUT: Duration = Duration::from_millis(1000);

impl ProjectStore {
    #[cfg(test)]
    pub fn test_actual_path(&self) -> PathBuf {
        self.actual_path()
    }

    #[cfg(test)]
    pub fn reject_writes(&self, reject: bool) {
        self.connection
            .pragma_update(None, "query_only", reject)
            .unwrap();
    }

    pub fn cancel_queries_with(&self, cancel: tokio_util::sync::CancellationToken) {
        self.connection
            .progress_handler(1000, Some(move || cancel.is_cancelled()));
    }

    pub(crate) fn cancel_queries_with_operation(
        &self,
        shutdown: CancellationToken,
        operation: CancellationToken,
    ) {
        self.connection.progress_handler(
            1000,
            Some(move || shutdown.is_cancelled() || operation.is_cancelled()),
        );
    }

    pub fn create(path: Option<&Path>) -> Result<Self> {
        if let Some(path) = path {
            ensure!(
                !crate::is_external_archive_file(path),
                "不能覆盖外部归档项目文件"
            );
            check_new_project_path(path)?;
        }
        let temporary = Some(tempfile::tempdir()?);
        let actual = temporary.as_ref().unwrap().path().join("project.mio");
        let mut create_opts = OpenOptions::new();
        create_opts.write(true).create_new(true);
        #[cfg(unix)]
        create_opts.mode(0o600);
        create_opts
            .open(&actual)
            .with_context(|| format!("不能新建项目 {}；已有文件不会被覆盖", actual.display()))?;
        let connection = Connection::open_with_flags(
            &actual,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let id = Uuid::new_v4();
        let mut store = Self {
            cache: Mutex::new(StoreCache::default()),
            connection,
            id,
            path: None,
            temporary,
            intruder_sessions: intruder::IntruderSessions::default(),
        };
        store.configure()?;
        store.connection.execute_batch(&format!("
            PRAGMA application_id = {APPLICATION_ID}; PRAGMA user_version = {SCHEMA};
            CREATE TABLE metadata (key TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;
            CREATE TABLE flows (id TEXT PRIMARY KEY, revision INTEGER NOT NULL, started INTEGER NOT NULL,
                url TEXT NOT NULL, method TEXT NOT NULL, state TEXT NOT NULL, summary BLOB NOT NULL, {FLOW_LISTENER_PORT_COLUMN}, {FLOW_SOURCE_COLUMN}, {FLOW_STATUS_COLUMN}, {FLOW_RESPONSE_BYTES_COLUMN}, {FLOW_ELAPSED_MS_COLUMN}, {FLOW_CAPTURE_NUMBER_COLUMN}, {FLOW_RESPONSE_LENGTH_COLUMN});
            CREATE INDEX flows_started ON flows(started DESC, id);
            CREATE INDEX flows_state ON flows(state);
            CREATE INDEX flows_source_started ON flows(source, started DESC, id);
            CREATE INDEX flows_source_status ON flows(source, status);
            CREATE INDEX flows_source_capture ON flows(source, capture_number);
            CREATE TABLE messages (flow TEXT NOT NULL REFERENCES flows(id) ON DELETE CASCADE,
                side INTEGER NOT NULL, head BLOB NOT NULL, body BLOB NOT NULL, PRIMARY KEY(flow, side)) WITHOUT ROWID;
        "))?;
        websocket::create_schema(&store.connection)?;
        websocket::create_originals(&store.connection)?;
        websocket::create_connections(&store.connection)?;
        websocket_notes::create_schema(&store.connection)?;
        ext_archive::create_schema(&store.connection)?;
        ext_archive::http_notes::create_schema(&store.connection)?;
        ext_archive::http_highlights::create_schema(&store.connection)?;
        intruder::create_schema(&store.connection)?;
        repeater_history::create_schema(&store.connection)?;
        target_site_map::create_schema(&store.connection)?;
        store
            .connection
            .execute("INSERT INTO metadata VALUES ('id', ?1)", [id.to_string()])?;
        store.connection.execute(
            "INSERT INTO metadata VALUES ('workspace', ?1)",
            [serde_json::to_vec(&Workspace::default())?],
        )?;
        store.checkpoint()?;
        store.record_opened_cache(Some(Workspace::default()))?;
        if let Some(path) = path {
            store.save(Some(path))?;
        }
        Ok(store)
    }

    pub fn open(path: &Path) -> Result<Self> {
        if crate::is_external_archive_file(path) {
            bail!("外部归档项目须通过只读导入转换，不能直接作为 MIO 项目打开；源文件未修改");
        }
        {
            let ro_flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let ro_conn =
                Connection::open_with_flags(path, ro_flags).context("不能打开项目文件")?;
            ro_conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
            ro_conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
            let ro_tx = ro_conn.unchecked_transaction()?;
            let app_id: i32 =
                ro_tx.pragma_query_value(None, "application_id", |row| row.get(0))?;
            let schema: i32 =
                ro_tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
            ensure!(
                app_id == APPLICATION_ID && schema == SCHEMA,
                "不是受支持的 MIO 项目；未修改文件"
            );
            schema_guard::validate(&ro_tx)?;
            ro_tx.commit()?;
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(path, flags).context("不能打开项目文件")?;
        connection.busy_timeout(STORE_BUSY_TIMEOUT)?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
        let id = {
            let transaction = connection.unchecked_transaction()?;
            let app_id: i32 =
                transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
            let schema: i32 =
                transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
            ensure!(
                app_id == APPLICATION_ID && schema == SCHEMA,
                "不是受支持的 MIO 项目；未修改文件"
            );
            schema_guard::validate(&transaction)?;
            let id = metadata::project_id(&transaction)?;
            let _workspace = metadata::workspace(&transaction)?;
            transaction.commit()?;
            id
        };
        connection.pragma_update(None, "query_only", false)?;
        let store = Self {
            cache: Mutex::new(StoreCache::default()),
            connection,
            id,
            path: Some(path.to_owned()),
            temporary: None,
            intruder_sessions: intruder::IntruderSessions::default(),
        };
        store.configure()?;
        store.record_opened_cache(None)?;
        Ok(store)
    }

    pub(super) fn configure(&self) -> Result<()> {
        self.connection.busy_timeout(STORE_BUSY_TIMEOUT)?;
        self.connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-4096; PRAGMA temp_store=MEMORY; PRAGMA mmap_size=33554432; PRAGMA trusted_schema=OFF;",
        )?;
        Ok(())
    }

    pub(super) fn record_opened_cache(&self, workspace: Option<Workspace>) -> Result<()> {
        let Some(ws) = workspace else {
            return Ok(());
        };
        let sv: i64 = self
            .connection
            .pragma_query_value(None, "schema_version", |row| row.get(0))?;
        let dv: i64 = self
            .connection
            .pragma_query_value(None, "data_version", |row| row.get(0))?;
        let tc = query_total_changes(&self.connection)?;
        if let Ok(mut guard) = self.cache.lock() {
            guard.validated_schema = Some((sv, dv));
            guard.workspace = Some(WorkspaceCacheEntry {
                data_version: dv,
                schema_version: sv,
                total_changes: tc,
                workspace: Arc::new(ws),
            });
        }
        Ok(())
    }

    pub(super) fn clear_connection_cache(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = StoreCache::default();
        }
    }

    pub(super) fn actual_path(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| self.temporary.as_ref().unwrap().path().join("project.mio"))
    }

    pub(crate) fn query_path(&self) -> PathBuf {
        self.actual_path()
    }

    pub fn checkpoint(&mut self) -> Result<()> {
        let (busy, _, _): (i32, i32, i32) =
            self.connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        ensure!(busy == 0, "项目仍被其它连接占用，WAL 尚未完全合并");
        Ok(())
    }

    pub fn close(self) -> Result<()> {
        self.clear_connection_cache();
        self.connection
            .close()
            .map_err(|(_, error)| error)
            .context("关闭项目数据库失败")?;
        if let Some(temporary) = self.temporary {
            temporary.close().context("清理临时项目目录失败")?;
        }
        Ok(())
    }

    pub fn total_bytes(&self) -> Result<u64> {
        crate::sys_metrics::measure_project_disk_bytes(&self.actual_path())
    }
}
