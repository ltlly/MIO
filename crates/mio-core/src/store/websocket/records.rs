use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use super::{
    CapturedWebSocket, FlowId, ProjectStore, Uuid, WebSocketDirection, WebSocketKind,
    WebSocketMessage, WebSocketSummary, http::MAX_BODY,
};

pub(super) const MAX_EXCLUDED_IDS: usize = 512;
pub(super) const SUMMARY_COLUMNS: &str = "w.id,w.revision,f.id AS flow,w.sequence,w.direction,w.kind,w.payload_bytes,w.observed,f.url,w.ordinal,c.id,c.listener_port,EXISTS(SELECT 1 FROM websocket_originals o WHERE w.id=o.message) AS edited";

pub(in crate::store) fn create_originals(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "CREATE TABLE websocket_originals (
            message TEXT PRIMARY KEY NOT NULL REFERENCES websocket_messages(id) ON DELETE CASCADE,
            body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<={MAX_BODY})
        ) WITHOUT ROWID;"
    ))?;
    Ok(())
}

pub(in crate::store) fn create_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "CREATE TABLE websocket_messages (
            ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            revision INTEGER NOT NULL CHECK(revision=1),
            flow TEXT NOT NULL REFERENCES flows(id) ON DELETE CASCADE,
            sequence INTEGER NOT NULL CHECK(sequence>0),
            direction INTEGER NOT NULL CHECK(direction IN (0,1)),
            kind INTEGER NOT NULL CHECK(kind IN (1,2,8,9,10)),
            payload_bytes INTEGER NOT NULL CHECK(payload_bytes BETWEEN 0 AND {MAX_BODY}),
            observed INTEGER NOT NULL,
            UNIQUE(flow,sequence)
        );
        CREATE INDEX websocket_flow_ordinal ON websocket_messages(flow,ordinal DESC);
        CREATE TABLE websocket_payloads (
            message TEXT PRIMARY KEY REFERENCES websocket_messages(id) ON DELETE CASCADE,
            body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<={MAX_BODY})
        ) WITHOUT ROWID;"
    ))?;
    Ok(())
}

pub(in crate::store) fn create_connections(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE websocket_connections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            flow TEXT NOT NULL UNIQUE REFERENCES flows(id) ON DELETE CASCADE,
            listener_port INTEGER CHECK(listener_port BETWEEN 1 AND 65535)
        );
        INSERT INTO websocket_connections(flow)
            SELECT f.id FROM flows f JOIN websocket_messages w ON f.id=w.flow
            GROUP BY f.id ORDER BY min(w.ordinal);",
    )?;
    Ok(())
}

impl<S: std::ops::Deref<Target = ProjectStore>> crate::store::WebSocketRepo<S> {
    pub(crate) fn list_messages(
        &self,
        limit: usize,
    ) -> Result<(usize, Vec<WebSocketMessage>)> {
        let clamped = limit.clamp(1, 500);
        let total: usize = self.0.connection.query_row(
            "SELECT count(*) FROM main.websocket_messages",
            [],
            |row| row.get::<_, usize>(0),
        )?;
        let mut stmt = self.0.connection.prepare(&format!(
            "SELECT {SUMMARY_COLUMNS}, p.body
             FROM main.websocket_messages w
             LEFT JOIN main.flows f ON f.id=w.flow
             LEFT JOIN main.websocket_connections c ON f.id=c.flow
             LEFT JOIN main.websocket_payloads p ON w.id=p.message
             WHERE typeof(p.body)='blob' AND length(p.body)<=?1
             ORDER BY w.ordinal DESC
             LIMIT ?2"
        ))?;
        let mut rows = stmt.query(params![MAX_BODY as i64, clamped as i64])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let summary = read_summary(row)?;
            let payload: Vec<u8> = row.get(13)?;
            out.push(WebSocketMessage {
                id: summary.id,
                revision: summary.revision,
                flow_id: summary.flow_id,
                sequence: summary.sequence,
                direction: summary.direction,
                kind: summary.kind,
                payload,
                observed_unix_ms: summary.observed_unix_ms,
            });
        }
        Ok((total, out))
    }

    #[cfg(test)]
    pub fn read(&self, id: Uuid) -> Result<WebSocketMessage> {
        let live_exists: bool = self.0.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM websocket_messages WHERE id=?1)",
            [id.to_string()],
            |row| row.get(0),
        )?;
        if live_exists {
            return read_record(&self.0.connection, id);
        }
        crate::service::websocket::ext_archive::read_ext_websocket_detail(&self.0, id)
            .map(|(record, _)| record)
    }

    pub fn read_detail(&self, id: Uuid) -> Result<(WebSocketMessage, Option<Vec<u8>>)> {
        let live_exists: bool = self.0.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM websocket_messages WHERE id=?1)",
            [id.to_string()],
            |row| row.get(0),
        )?;
        if live_exists {
            let transaction = self.0.connection.unchecked_transaction()?;
            let detail = read_record_with_original(&transaction, id)?;
            transaction.commit()?;
            return Ok(detail);
        }
        crate::service::websocket::ext_archive::read_ext_websocket_detail(&self.0, id)
    }

    pub fn flow_url(&self, id: FlowId) -> Result<String> {
        self.0
            .connection
            .query_row(
                "SELECT url FROM flows WHERE id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .context("WebSocket 对应的 HTTP 流量不存在")
    }

    pub fn numbers(&self) -> Result<(u64, u64)> {
        let next = |table: &str| -> Result<u64> {
            let last: Option<u64> = self
                .0
                .connection
                .query_row(
                    "SELECT seq FROM sqlite_sequence WHERE name=?1",
                    [table],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(last.unwrap_or_default() + 1)
        };
        Ok((next("websocket_messages")?, next("websocket_connections")?))
    }

    pub fn connection(&self, flow: FlowId) -> Result<Option<(u64, Option<u16>)>> {
        self.0
            .connection
            .query_row(
                "SELECT c.id,c.listener_port FROM flows f
                 JOIN websocket_connections c ON f.id=c.flow WHERE f.id=?1",
                [flow.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn ordinal(&self, id: Uuid) -> Result<Option<u64>> {
        self.0
            .connection
            .query_row(
                "SELECT ordinal FROM websocket_messages WHERE id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }
}

impl crate::store::WebSocketRepo<&mut ProjectStore> {
    #[cfg(test)]
    pub fn put(&mut self, record: &WebSocketMessage) -> Result<()> {
        self.put_captured(&record.clone().into())
    }

    pub fn put_captured(&mut self, record: &CapturedWebSocket) -> Result<()> {
        let transaction = self.0.connection.transaction()?;
        write_record(&transaction, record)?;
        transaction.commit()?;
        Ok(())
    }
}

impl ProjectStore {
    pub(crate) fn from_read_only(connection: Connection, id: Uuid) -> Self {
        Self {
            connection,
            id,
            path: None,
            temporary: None,
            intruder_sessions: super::super::intruder::IntruderSessions::default(),
            cache: std::sync::Mutex::new(super::super::StoreCache::default()),
        }
    }
}

pub(in crate::store) fn write_record(
    transaction: &Transaction<'_>,
    capture: &CapturedWebSocket,
) -> Result<()> {
    let record = capture.record.as_ref();
    ensure!(
        record.revision == 1,
        "不支持此 WebSocket 消息 revision；未替换原消息"
    );
    ensure!(record.sequence > 0, "WebSocket 消息序号必须大于零");
    let sequence = i64::try_from(record.sequence).context("WebSocket 消息序号溢出")?;
    // Matches the proxy's default 16 MiB message bound; larger payloads fail, never truncate.
    ensure!(
        record.payload.len() <= MAX_BODY,
        "WebSocket 消息超过 16 MiB 限额"
    );
    if let Some(original) = &capture.original_payload {
        ensure!(
            original.len() <= MAX_BODY,
            "WebSocket 消息原始正文超过 16 MiB 限额"
        );
    }
    let id = record.id.to_string();
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM websocket_messages WHERE id=?1)",
        [&id],
        |row| row.get(0),
    )?;
    if exists {
        let (stored_record, stored_original) = read_record_with_original(transaction, record.id)?;
        ensure!(
            stored_record == *record,
            "WebSocket 消息 ID 或内容冲突；未替换原消息"
        );
        ensure!(
            stored_original.as_deref() == capture.edited_original(),
            "WebSocket 消息原始正文冲突；未替换原消息"
        );
        let port: Option<u16> = transaction.query_row(
            "SELECT c.listener_port FROM flows f
             JOIN websocket_connections c ON f.id=c.flow WHERE f.id=?1",
            [record.flow_id.to_string()],
            |row| row.get(0),
        )?;
        ensure!(
            capture.listener_port.is_none() || capture.listener_port == port,
            "WebSocket 连接监听端口冲突；未替换原连接"
        );
        return Ok(());
    }
    let flow: String = transaction
        .query_row(
            "SELECT id FROM flows WHERE id=?1",
            [record.flow_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .context("WebSocket 对应的 HTTP 流量不存在；未保存孤立消息")?;
    let duplicate: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM flows f JOIN websocket_messages w ON f.id=w.flow
         WHERE f.id=?1 AND w.sequence=?2)",
        params![flow, sequence],
        |row| row.get(0),
    )?;
    ensure!(!duplicate, "WebSocket 流量内消息序号冲突；未替换原消息");
    let connection: Option<Option<u16>> = transaction
        .query_row(
            "SELECT c.listener_port FROM flows f
             JOIN websocket_connections c ON f.id=c.flow WHERE f.id=?1",
            [&flow],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(port) = connection {
        ensure!(
            capture.listener_port.is_none() || capture.listener_port == port,
            "WebSocket 连接监听端口冲突；未替换原连接"
        );
    } else {
        transaction.execute(
            "INSERT INTO websocket_connections(flow,listener_port) VALUES(?1,?2)",
            params![flow, capture.listener_port],
        )?;
    }
    let direction = direction_value(record.direction);
    let kind = kind_value(record.kind);
    transaction.execute(
        "INSERT INTO websocket_messages(id,revision,flow,sequence,direction,kind,payload_bytes,observed)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            id,
            record.revision,
            flow,
            sequence,
            direction,
            kind,
            i64::try_from(record.payload.len())?,
            record.observed_unix_ms,
        ],
    )?;
    transaction.execute(
        "INSERT INTO websocket_payloads(message,body) VALUES(?1,?2)",
        params![id, record.payload],
    )?;
    if let Some(original) = capture.edited_original() {
        transaction.execute(
            "INSERT INTO websocket_originals(message,body) VALUES(?1,?2)",
            params![id, original],
        )?;
    }
    Ok(())
}

pub(super) fn direction_value(direction: WebSocketDirection) -> i64 {
    match direction {
        WebSocketDirection::ClientToServer => 0,
        WebSocketDirection::ServerToClient => 1,
    }
}

pub(super) fn kind_value(kind: WebSocketKind) -> i64 {
    match kind {
        WebSocketKind::Text => 1,
        WebSocketKind::Binary => 2,
        WebSocketKind::Close => 8,
        WebSocketKind::Ping => 9,
        WebSocketKind::Pong => 10,
    }
}

pub(super) fn read_summary(row: &Row<'_>) -> Result<WebSocketSummary> {
    let revision: u64 = row.get(1)?;
    ensure!(revision == 1, "不支持此 WebSocket 消息 revision");
    let sequence: u64 = row.get(3)?;
    ensure!(sequence > 0, "WebSocket 消息序号损坏");
    let payload_bytes: u64 = row.get(6)?;
    ensure!(
        payload_bytes <= MAX_BODY as u64,
        "WebSocket 消息超过 16 MiB 限额"
    );
    let ordinal: u64 = row.get(9)?;
    let connection_id: u64 = row
        .get::<_, Option<u64>>(10)?
        .context("WebSocket 连接编号缺失")?;
    let listener_port: Option<u16> = row.get(11)?;
    ensure!(ordinal > 0 && connection_id > 0, "WebSocket 编号损坏");
    ensure!(listener_port != Some(0), "WebSocket 监听端口损坏");
    let edited = match row.get::<_, i64>(12)? {
        0 => false,
        1 => true,
        _ => bail!("WebSocket 消息编辑状态损坏"),
    };
    Ok(WebSocketSummary {
        id: Uuid::parse_str(&row.get::<_, String>(0)?).context("WebSocket 消息 ID 损坏")?,
        ordinal,
        connection_id,
        listener_port,
        revision,
        flow_id: Uuid::parse_str(&row.get::<_, String>(2)?).context("WebSocket 流量 ID 损坏")?,
        sequence,
        direction: match row.get::<_, i64>(4)? {
            0 => WebSocketDirection::ClientToServer,
            1 => WebSocketDirection::ServerToClient,
            _ => bail!("WebSocket 消息方向损坏"),
        },
        kind: match row.get::<_, i64>(5)? {
            1 => WebSocketKind::Text,
            2 => WebSocketKind::Binary,
            8 => WebSocketKind::Close,
            9 => WebSocketKind::Ping,
            10 => WebSocketKind::Pong,
            _ => bail!("WebSocket 消息类型损坏"),
        },
        payload_bytes,
        observed_unix_ms: row.get(7)?,
        url: row
            .get::<_, Option<String>>(8)?
            .context("WebSocket 对应的 HTTP 流量缺失")?,
        edited,
        notes_revision: 0,
        notes_preview: String::new(),
    })
}

#[cfg(test)]
pub(super) fn read_record(connection: &Connection, id: Uuid) -> Result<WebSocketMessage> {
    let mut query = connection.prepare(&format!(
        "SELECT {SUMMARY_COLUMNS},
            typeof(p.body),CASE WHEN typeof(p.body)='blob' THEN length(p.body) END,
            CASE WHEN typeof(p.body)='blob' THEN CASE WHEN length(p.body)<=?2 THEN p.body END END
        FROM websocket_messages w LEFT JOIN flows f ON f.id=w.flow
        LEFT JOIN websocket_connections c ON f.id=c.flow
        LEFT JOIN websocket_payloads p ON w.id=p.message WHERE w.id=?1"
    ))?;
    let mut rows = query.query(params![id.to_string(), MAX_BODY as i64])?;
    let row = rows.next()?.context("WebSocket 消息不存在")?;
    let summary = read_summary(row)?;
    match row.get::<_, String>(13)?.as_str() {
        "blob" => (),
        "null" => bail!("WebSocket 消息正文缺失"),
        _ => bail!("WebSocket 消息正文不是二进制"),
    }
    let length: u64 = row.get(14)?;
    ensure!(
        length <= MAX_BODY as u64,
        "WebSocket 消息正文超过 16 MiB 限额"
    );
    ensure!(
        length == summary.payload_bytes,
        "WebSocket 消息正文长度与摘要不一致"
    );
    // SQL gates the blob before Rust allocates it, including when a project has corrupt lengths.
    let payload: Vec<u8> = row
        .get::<_, Option<Vec<u8>>>(15)?
        .context("WebSocket 消息正文不是二进制")?;
    ensure!(
        rows.next()?.is_none(),
        "WebSocket 消息存在重复正文归属；未选择或读取其中一条"
    );
    Ok(WebSocketMessage {
        id: summary.id,
        revision: summary.revision,
        flow_id: summary.flow_id,
        sequence: summary.sequence,
        direction: summary.direction,
        kind: summary.kind,
        payload,
        observed_unix_ms: summary.observed_unix_ms,
    })
}

pub(super) fn read_record_with_original(
    connection: &Connection,
    id: Uuid,
) -> Result<(WebSocketMessage, Option<Vec<u8>>)> {
    let mut query = connection.prepare(&format!(
        "SELECT {SUMMARY_COLUMNS},
            typeof(p.body),CASE WHEN typeof(p.body)='blob' THEN length(p.body) END,
            CASE WHEN typeof(p.body)='blob' THEN CASE WHEN length(p.body)<=?2 THEN p.body END END,
            o.message,
            typeof(o.body),CASE WHEN typeof(o.body)='blob' THEN length(o.body) END,
            CASE WHEN typeof(o.body)='blob' THEN CASE WHEN length(o.body)<=?2 THEN o.body END END
        FROM websocket_messages w LEFT JOIN flows f ON f.id=w.flow
        LEFT JOIN websocket_connections c ON f.id=c.flow
        LEFT JOIN websocket_payloads p ON w.id=p.message
        LEFT JOIN websocket_originals o ON w.id=o.message WHERE w.id=?1"
    ))?;
    let mut rows = query.query(params![id.to_string(), MAX_BODY as i64])?;
    let row = rows.next()?.context("WebSocket 消息不存在")?;
    let summary = read_summary(row)?;
    let record = {
        match row.get::<_, String>(13)?.as_str() {
            "blob" => (),
            "null" => bail!("WebSocket 消息正文缺失"),
            _ => bail!("WebSocket 消息正文不是二进制"),
        }
        let length: u64 = row.get(14)?;
        ensure!(
            length <= MAX_BODY as u64,
            "WebSocket 消息正文超过 16 MiB 限额"
        );
        ensure!(
            length == summary.payload_bytes,
            "WebSocket 消息正文长度与摘要不一致"
        );
        let payload: Vec<u8> = row
            .get::<_, Option<Vec<u8>>>(15)?
            .context("WebSocket 消息正文不是二进制")?;
        WebSocketMessage {
            id: summary.id,
            revision: summary.revision,
            flow_id: summary.flow_id,
            sequence: summary.sequence,
            direction: summary.direction,
            kind: summary.kind,
            payload,
            observed_unix_ms: summary.observed_unix_ms,
        }
    };
    let original = if row.get::<_, Option<String>>(16)?.is_some() {
        match row.get::<_, String>(17)?.as_str() {
            "blob" => (),
            "null" => bail!("WebSocket 消息原始正文缺失"),
            _ => bail!("WebSocket 消息原始正文不是二进制"),
        }
        let length: u64 = row.get(18)?;
        ensure!(
            length <= MAX_BODY as u64,
            "WebSocket 消息原始正文超过 16 MiB 限额"
        );
        Some(
            row.get::<_, Option<Vec<u8>>>(19)?
                .context("WebSocket 消息原始正文不是二进制")?,
        )
    } else {
        None
    };
    ensure!(
        rows.next()?.is_none(),
        "WebSocket 消息存在重复正文或原始正文归属；未选择或读取其中一条"
    );
    Ok((record, original))
}
