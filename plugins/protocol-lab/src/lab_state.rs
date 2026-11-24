use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE},
};
use mio_plugin_sdk::Header;

use super::*;

impl Lab {
    pub(crate) fn encode_b64(&self, bytes: &[u8]) -> String {
        if self.url_safe_base64 {
            URL_SAFE.encode(bytes)
        } else {
            STANDARD.encode(bytes)
        }
    }

    pub(crate) fn decode_b64(&self, bytes: &[u8]) -> Result<Vec<u8>, base64::DecodeError> {
        if self.url_safe_base64 {
            URL_SAFE.decode(bytes)
        } else {
            STANDARD.decode(bytes)
        }
    }

    pub(crate) fn set_editor(editor: &mut Editor, contents: Vec<u8>) -> Result<(), String> {
        if contents.len() > MAX_EDITOR_BYTES {
            return Err("编辑内容超过 256 KiB；没有截断数据".into());
        }
        editor.revision = editor.revision.checked_add(1).ok_or("文档 revision 耗尽")?;
        editor.format = if std::str::from_utf8(&contents).is_ok() {
            EditorFormat::Utf8
        } else {
            EditorFormat::Binary
        } as i32;
        editor.contents = contents;
        Ok(())
    }

    pub(crate) fn sync_editor(&mut self, snapshot: &EditorSnapshot) -> Result<(), String> {
        snapshot.validate()?;
        let editor = match snapshot.document_id {
            RAW_DOCUMENT => &mut self.raw,
            DECODED_DOCUMENT => &mut self.decoded,
            RESPONSE_DOCUMENT => self.response.as_mut().ok_or("响应编辑器尚不存在")?,
            _ => return Err("未知编辑器".into()),
        };
        if snapshot.revision < editor.revision
            || (snapshot.revision == editor.revision && snapshot.contents != editor.contents)
        {
            return Err("编辑快照已过期或冲突".into());
        }
        if editor.read_only
            && (snapshot.revision != editor.revision || snapshot.contents != editor.contents)
        {
            return Err("只读编辑器拒绝修改".into());
        }
        editor.revision = snapshot.revision;
        editor.format = if std::str::from_utf8(&snapshot.contents).is_ok() {
            EditorFormat::Utf8
        } else {
            EditorFormat::Binary
        } as i32;
        editor.contents.clone_from(&snapshot.contents);
        Ok(())
    }

    pub(crate) fn body(&self, document: u64) -> Result<Vec<u8>, String> {
        match document {
            RAW_DOCUMENT => Ok(self.raw.contents.clone()),
            DECODED_DOCUMENT => Ok(self.encode_b64(&self.decoded.contents).into_bytes()),
            _ => Err("请选择 Raw 或 Decoded 编辑器，不能发送响应视图".into()),
        }
    }

    pub(crate) fn request(&self, document: u64) -> Result<HttpRequest, String> {
        let mut request = self
            .context
            .as_ref()
            .ok_or("尚未选择实际请求")?
            .request
            .clone();
        request.body = self.body(document)?;
        let length = request.body.len().to_string().into_bytes();
        let mut found = false;
        for header in &mut request.headers {
            if header.name.eq_ignore_ascii_case("content-length") {
                header.value.clone_from(&length);
                found = true;
            }
        }
        if !found {
            request.headers.push(Header {
                name: "Content-Length".into(),
                value: length,
            });
        }
        Ok(request)
    }

    pub(crate) fn load(&mut self, context: Context) -> Result<(), String> {
        self.selection_revision = self
            .selection_revision
            .checked_add(1)
            .ok_or("选择 revision 耗尽")?;
        Self::set_editor(&mut self.raw, context.request.body.clone())?;
        match self.decode_b64(&context.request.body) {
            Ok(decoded) => {
                Self::set_editor(&mut self.decoded, decoded)?;
                self.status = "已读取实际请求；未发送。".into();
                self.error = false;
            }
            Err(_) => {
                Self::set_editor(&mut self.decoded, vec![])?;
                self.status =
                    "请求正文不是有效 Base64；Raw 保留原内容，Decoded 尚无有效结果。".into();
                self.error = true;
            }
        }
        if let Some(response) = &mut self.response {
            Self::set_editor(response, vec![])?;
        }
        self.raw.read_only = context.read_only;
        self.decoded.read_only = context.read_only;
        self.context_generation = self
            .context_generation
            .checked_add(1)
            .ok_or("消息上下文代际耗尽")?;
        self.context = Some(context);
        self.response_context = None;
        self.active_document = RAW_DOCUMENT;
        Ok(())
    }

    pub(crate) fn load_response(&mut self, context: ResponseContext) -> Result<(), String> {
        self.selection_revision = self
            .selection_revision
            .checked_add(1)
            .ok_or("选择 revision 耗尽")?;
        Self::set_editor(&mut self.raw, context.response.body.clone())?;
        match self.decode_b64(&context.response.body) {
            Ok(decoded) => {
                Self::set_editor(&mut self.decoded, decoded)?;
                self.status = "已读取实际响应；未发送。".into();
                self.error = false;
            }
            Err(_) => {
                Self::set_editor(&mut self.decoded, vec![])?;
                self.status =
                    "响应正文不是有效 Base64；Raw 保留原内容，Decoded 尚无有效结果。".into();
                self.error = true;
            }
        }
        self.raw.read_only = context.read_only;
        self.decoded.read_only = context.read_only;
        self.context_generation = self
            .context_generation
            .checked_add(1)
            .ok_or("消息上下文代际耗尽")?;
        self.context = None;
        self.response_context = Some(context);
        self.active_document = RAW_DOCUMENT;
        Ok(())
    }

    pub(crate) fn close(
        &mut self,
        ctx: &mut dyn HostClient,
        resource: Uuid,
    ) -> Result<bool, String> {
        let matches_req = self
            .context
            .as_ref()
            .is_some_and(|context| context.resource == resource);
        let matches_resp = self
            .response_context
            .as_ref()
            .is_some_and(|context| context.resource == resource);
        if !matches_req && !matches_resp {
            return Ok(false);
        }
        let previous_generation = self.context_generation;
        let generation = previous_generation
            .checked_add(1)
            .ok_or_else(|| "消息上下文代际耗尽".to_owned())?;
        let selection = self
            .selection_revision
            .checked_add(1)
            .ok_or_else(|| "选择 revision 耗尽".to_owned())?;
        if matches_req {
            self.context = None;
        }
        if matches_resp {
            self.response_context = None;
        }
        self.context_generation = generation;
        self.selection_revision = selection;
        self.raw.read_only = false;
        self.decoded.read_only = false;
        self.status = "请求上下文已关闭；编辑内容保留供复制或本地编解码，未发送或回写。".into();
        self.error = false;
        for (&id, pending) in &self.pending {
            if matches!(pending,
                PendingService::Send { context_generation }
                | PendingService::WriteBack { context_generation, .. }
                if *context_generation == previous_generation)
            {
                ctx.cancel_host_call(id)?;
            }
        }
        Ok(true)
    }

    pub(crate) fn host_result(
        &mut self,
        ctx: &mut dyn HostClient,
        id: u64,
        result: HostResult,
    ) -> Result<PanelUpdate, String> {
        let Some(pending) = self.pending.remove(&id) else {
            return Err("未知宿主结果 ID".into());
        };
        let history_viewport = match &pending {
            PendingService::History { viewport_revision } => Some(*viewport_revision),
            _ => None,
        };
        if matches!(pending, PendingService::Send { .. }) {
            self.sending = false;
        }
        let result: Result<(), String> = (|| {
            let outcome = result.outcome.ok_or("宿主结果为空")?;
            if let host_result::Outcome::Error(error) = outcome {
                return Err(error.message);
            }
            match (pending, outcome) {
                (
                    PendingService::History { viewport_revision },
                    host_result::Outcome::History(page),
                ) => {
                    if viewport_revision != self.viewport_revision {
                        return Ok(());
                    }
                    if page.rows.len() > MAX_TABLE_ROWS
                        || page.offset > page.total
                        || page.rows.len() as u64 > page.total - page.offset
                    {
                        return Err("宿主流量分页无效".into());
                    }
                    for row in &page.rows {
                        let _: mio_plugin_sdk::FlowSummary = row.clone().try_into()?;
                    }
                    self.history = page;
                    self.positions.retain(|id, _| {
                        self.history
                            .rows
                            .iter()
                            .any(|r| protocol::uuid(&r.id).is_ok_and(|row| row.to_string() == *id))
                    });
                    self.hit_ids.retain(|id, _| {
                        self.history
                            .rows
                            .iter()
                            .any(|r| protocol::uuid(&r.id).is_ok_and(|row| row.to_string() == *id))
                    });
                    for row in &self.history.rows {
                        let id = protocol::uuid(&row.id)?.to_string();
                        if !self.hit_ids.contains_key(&id) {
                            self.hit_ids.insert(id, self.next_hit_id);
                            self.next_hit_id = self
                                .next_hit_id
                                .checked_add(1)
                                .ok_or("时间线命中 ID 耗尽")?;
                        }
                    }
                    self.status = format!(
                        "共 {} 条实际流量；当前页 {} 条。",
                        self.history.total,
                        self.history.rows.len()
                    );
                }
                (PendingService::Detail { selection }, host_result::Outcome::Record(record)) => {
                    if selection != self.selection_revision {
                        return Ok(());
                    }
                    let record: mio_plugin_sdk::FlowRecord = (*record).try_into()?;
                    self.load(Context {
                        resource: record.summary.id,
                        revision: record.summary.revision,
                        read_only: false,
                        is_draft: false,
                        request: record.request,
                    })?;
                }
                (
                    PendingService::Send { context_generation },
                    host_result::Outcome::Response(response),
                ) => {
                    if context_generation != self.context_generation {
                        self.status = "先前请求已完成；响应未覆盖新的编辑上下文。".into();
                        return Ok(());
                    }
                    let response: mio_plugin_sdk::HttpResponse = response.try_into()?;
                    let editor = self
                        .response
                        .get_or_insert_with(|| empty_editor(RESPONSE_DOCUMENT, true));
                    Self::set_editor(editor, response.body)?;
                    self.status = format!(
                        "实际响应 HTTP {}；{} 字节。",
                        response.status,
                        editor.contents.len()
                    );
                }
                (PendingService::OpenRepeater, host_result::Outcome::Resource(_)) => {
                    self.status = "已打开 Repeater；未自动发送。".into()
                }
                (PendingService::OpenIntruder, host_result::Outcome::Done(_)) => {
                    self.status = "已发送到 Intruder；未自动启动攻击。".into()
                }
                (PendingService::SaveKv, host_result::Outcome::Done(_)) => {
                    self.status = "已存入插件 KV（scratch）。".into()
                }
                (PendingService::LoadKv, host_result::Outcome::StorageValue(val)) => {
                    if self.decoded.read_only {
                        return Err("只读上下文拒绝将 KV 写入 Decoded".into());
                    }
                    if val.present {
                        Self::set_editor(&mut self.decoded, val.value)?;
                        self.active_document = DECODED_DOCUMENT;
                        self.status = format!(
                            "已从 KV 读取 {} 字节到 Decoded。",
                            self.decoded.contents.len()
                        );
                    } else {
                        self.status = "插件 KV（scratch）尚无保存内容。".into();
                    }
                }
                (PendingService::SavePref, host_result::Outcome::Done(_)) => {
                    self.status = "已存入跨项目偏好（scratch_pref）。".into()
                }
                (PendingService::LoadPref, host_result::Outcome::StorageValue(val)) => {
                    if self.decoded.read_only {
                        return Err("只读上下文拒绝将偏好写入 Decoded".into());
                    }
                    if val.present {
                        Self::set_editor(&mut self.decoded, val.value)?;
                        self.active_document = DECODED_DOCUMENT;
                        self.status = format!(
                            "已从跨项目偏好读取 {} 字节到 Decoded。",
                            self.decoded.contents.len()
                        );
                    } else {
                        self.status = "跨项目偏好（scratch_pref）尚无保存内容。".into();
                    }
                }
                (
                    PendingService::WriteBack {
                        request,
                        context_generation,
                    },
                    host_result::Outcome::Resource(resource),
                ) => {
                    if context_generation != self.context_generation {
                        self.status = "先前草稿回写已结束；结果未覆盖新的编辑上下文。".into();
                        return Ok(());
                    }
                    let context = self.context.as_mut().ok_or("草稿上下文已关闭")?;
                    if protocol::uuid(&resource.resource_id)? != context.resource
                        || resource.revision <= context.revision
                    {
                        return Err("草稿回写结果已过期".into());
                    }
                    context.revision = resource.revision;
                    context.request = request;
                    self.status = "已回写实际草稿；未自动发送。".into();
                }
                _ => return Err("宿主结果类型不匹配".into()),
            }
            Ok(())
        })();
        self.error = result.is_err();
        if let Err(error) = result {
            if history_viewport == Some(self.viewport_revision) {
                self.viewport_revision = self.viewport_revision.saturating_add(1);
            }
            self.status = error;
        }
        let rev = ctx.next_panel_revision();
        Ok(PanelUpdate::Snapshot(self.snapshot_at(rev)?))
    }

    pub(crate) fn commit(&mut self, request: CommitEditor) -> Result<Committed, String> {
        let context = self.context.as_ref().ok_or("没有消息编辑上下文")?;
        if context.read_only
            || protocol::uuid(&request.resource_id)? != context.resource
            || request.message_revision != context.revision
        {
            return Err("只读、旧 revision 或跨资源的编辑提交已拒绝".into());
        }
        let base: HttpRequest = request.request.ok_or("缺少基准请求")?.try_into()?;
        if base != context.request {
            return Err("相同 message revision 的基准请求已改变".into());
        }
        let snapshot = request.editor.ok_or("缺少最新 EditorSnapshot")?;
        self.sync_editor(&snapshot)?;
        Ok(Committed {
            resource_id: request.resource_id,
            message_revision: request.message_revision,
            document_id: snapshot.document_id,
            document_revision: snapshot.revision,
            request: Some(self.request(snapshot.document_id)?.into()),
        })
    }

    pub(crate) fn resync(&mut self, snapshot: &PanelSnapshot) -> Result<(), String> {
        snapshot.validate()?;
        let mut editors = vec![];
        if let Some(root) = &snapshot.root {
            root.editors(&mut editors);
        }
        for (_, editor) in editors {
            let current = match editor.document_id {
                RAW_DOCUMENT => &mut self.raw,
                DECODED_DOCUMENT => &mut self.decoded,
                RESPONSE_DOCUMENT => self
                    .response
                    .as_mut()
                    .ok_or_else(|| "响应编辑器尚不存在".to_owned())?,
                _ => return Err("未知重同步编辑器".into()),
            };
            if editor.revision >= current.revision {
                // Only explicit host Resync can resolve equal-revision content conflicts.
                *current = editor.clone();
            }
        }
        if let Some(node::Kind::VirtualTable(table)) = snapshot
            .root
            .as_ref()
            .and_then(|root| root.find(TABLE_NODE))
            .and_then(|node| node.kind.as_ref())
        {
            self.viewport_revision = self.viewport_revision.max(table.viewport_revision);
        }
        if let Some(node::Kind::Split(split)) = snapshot
            .root
            .as_ref()
            .and_then(|root| root.find(3))
            .and_then(|node| node.kind.as_ref())
        {
            self.split_fraction = split.fraction;
        }
        self.revision = self.revision.max(snapshot.revision);
        Ok(())
    }
}
