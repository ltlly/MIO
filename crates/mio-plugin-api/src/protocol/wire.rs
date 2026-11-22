use std::{
    io::{self, Read, Write},
    time::Duration,
};

use prost::Message;

use super::*;

impl Envelope {
    pub fn encode_bytes(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn decode_bytes(bytes: &[u8]) -> Result<Self, String> {
        Self::decode(bytes).map_err(|_| "无法解码 Envelope".into())
    }

    pub fn new(
        identity: ChannelIdentity,
        request_id: u64,
        revision: u64,
        body: envelope::Body,
        timeout: Duration,
    ) -> Self {
        Self {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            request_id,
            instance_id: identity.instance_id.as_bytes().to_vec(),
            project_id: identity.project.id.as_bytes().to_vec(),
            project_generation: identity.project.generation,
            panel_id: PANEL_ID,
            revision,
            deadline_unix_ms: unix_ms()
                .saturating_add(timeout.as_millis().min(u64::MAX as u128) as u64),
            in_reply_to: 0,
            body: Some(body),
        }
    }

    pub fn with_panel_id(mut self, panel_id: u64) -> Self {
        self.panel_id = panel_id;
        self
    }

    pub fn with_minor(mut self, minor: u32) -> Self {
        self.minor = minor;
        self
    }

    pub fn identity(&self) -> Result<ChannelIdentity, String> {
        if self.project_generation == 0 {
            return Err("project generation 不能为零".into());
        }
        Ok(ChannelIdentity {
            instance_id: uuid(&self.instance_id)?,
            project: ProjectKey {
                id: uuid(&self.project_id)?,
                generation: self.project_generation,
            },
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_for_minor(PROTOCOL_MINOR)
    }

    pub fn validate_for_minor(&self, expected_minor: u32) -> Result<(), String> {
        if self.major != PROTOCOL_MAJOR
            || !matches!(expected_minor, LEGACY_COMPAT_MINOR | PROTOCOL_MINOR)
            || self.minor != expected_minor
        {
            return Err("插件协议版本不兼容".into());
        }
        self.identity()?;
        if self.request_id == 0
            || self.panel_id == 0
            || self.deadline_unix_ms == 0
            || self.body.is_none()
        {
            return Err("Envelope 缺少身份、deadline 或消息".into());
        }
        if self.encoded_len() > MAX_FRAME_BYTES {
            return Err("插件消息超过 1 MiB 帧限额".into());
        }
        let body = self.body.as_ref().unwrap();
        if expected_minor < PROTOCOL_MINOR
            && (matches!(
                body,
                envelope::Body::PassiveCheckCall(_)
                    | envelope::Body::PassiveCheckResult(_)
                    | envelope::Body::ActiveCheckCall(_)
                    | envelope::Body::ActiveCheckResult(_)
            ) || matches!(
                body,
                envelope::Body::HostCall(call)
                    if matches!(call.operation, Some(host_call::Operation::SubmitAuditIssue(_)))
            ))
        {
            return Err("当前兼容协议版本不支持该检查/审计消息，请升级插件至协议 v1.6".into());
        }
        match body {
            envelope::Body::OpenRequest(open) => {
                uuid(&open.resource_id)?;
                if open.message_revision == 0 {
                    return Err("消息 revision 不能为零".into());
                }
                open.request.as_ref().ok_or("缺少请求")?.validate()?;
            }
            envelope::Body::OpenResponse(open) => {
                uuid(&open.resource_id)?;
                if open.message_revision == 0 {
                    return Err("消息 revision 不能为零".into());
                }
                open.response.as_ref().ok_or("缺少响应")?.validate()?;
            }
            envelope::Body::CommitResponseEditor(commit) => {
                uuid(&commit.resource_id)?;
                if commit.message_revision == 0 {
                    return Err("消息 revision 不能为零".into());
                }
                commit.response.as_ref().ok_or("缺少响应")?.validate()?;
                commit.editor.as_ref().ok_or("缺少编辑快照")?.validate()?;
            }
            envelope::Body::ResponseCommitted(committed) => {
                uuid(&committed.resource_id)?;
                if committed.message_revision == 0 || committed.document_revision == 0 {
                    return Err("消息或文档 revision 不能为零".into());
                }
                committed.response.as_ref().ok_or("缺少响应")?.validate()?;
            }
            envelope::Body::CloseRequest(close) => {
                uuid(&close.resource_id)?;
            }
            envelope::Body::HostResult(result) => result.validate()?,
            envelope::Body::HttpHookCall(call) => {
                uuid(&call.flow_id)?;
                SourceDto::try_from(call.source).map_err(|_| "未知流量来源")?;
                call.request.as_ref().ok_or("缺少请求")?.validate()?;
                if let Some(resp) = &call.response {
                    resp.validate()?;
                }
            }
            envelope::Body::HttpHookResult(res) => {
                HookAction::try_from(res.action).map_err(|_| "未知 Hook 动作")?;
                if let Some(req) = &res.request {
                    req.validate()?;
                }
                if let Some(resp) = &res.response {
                    resp.validate()?;
                }
            }
            envelope::Body::WebSocketHookCall(call) => {
                uuid(&call.flow_id)?;
                uuid(&call.message_id)?;
                if call.url.is_empty()
                    || call.url.len() > 16 * 1024
                    || call.payload.len() > MAX_INLINE_BODY
                {
                    return Err("WebSocket Hook 调用超过内联限额".into());
                }
            }
            envelope::Body::WebSocketHookResult(res) => {
                HookAction::try_from(res.action).map_err(|_| "未知 Hook 动作")?;
                if res.payload.len() > MAX_INLINE_BODY {
                    return Err("WebSocket Hook 结果超过内联限额".into());
                }
            }
            envelope::Body::SessionActionCall(call) => {
                SourceDto::try_from(call.source).map_err(|_| "未知流量来源")?;
                call.request.as_ref().ok_or("缺少请求")?.validate()?;
            }
            envelope::Body::SessionActionResult(res) => {
                if let Some(req) = &res.request {
                    req.validate()?;
                } else if res.modified {
                    return Err("Session Hook 声明修改但缺少请求".into());
                }
            }
            envelope::Body::PayloadCall(call) => {
                PayloadOp::try_from(call.op).map_err(|_| "未知 Payload 操作")?;
                if call.config.len() > 4096 || call.inputs.len() > 10_000 {
                    return Err("Payload 调用参数超限".into());
                }
                let total: usize = call.inputs.iter().map(Vec::len).sum();
                if total > MAX_INLINE_BODY {
                    return Err("Payload 输入总字节超限".into());
                }
            }
            envelope::Body::PayloadResult(res) => {
                if res.outputs.len() > 10_000 {
                    return Err("Payload 输出条数超限".into());
                }
                let total: usize = res.outputs.iter().map(Vec::len).sum();
                if total > MAX_INLINE_BODY {
                    return Err("Payload 输出总字节超限".into());
                }
            }
            envelope::Body::ContextMenuCall(call) => {
                SourceDto::try_from(call.source).map_err(|_| "未知流量来源")?;
                if call.action_id.len() > 256
                    || call.selection_end < call.selection_start
                    || call.selected_bytes.len() > MAX_INLINE_BODY
                {
                    return Err("上下文菜单调用参数或选区无效".into());
                }
                call.request.as_ref().ok_or("缺少请求")?.validate()?;
                if let Some(resp) = &call.response {
                    resp.validate()?;
                }
            }
            envelope::Body::ContextMenuResult(res) => {
                if res.status_message.len() > 4096 || res.log_line.len() > 4096 {
                    return Err("上下文菜单返回文本超过限额".into());
                }
                if res
                    .replace_request
                    .as_ref()
                    .is_some_and(|raw| raw.len() > MAX_INLINE_BODY)
                {
                    return Err("上下文菜单原地替换报文超过内联限额".into());
                }
                if let Some(req) = &res.open_repeater {
                    req.validate()?;
                }
            }
            envelope::Body::PassiveCheckCall(call) => {
                uuid(&call.flow_id)?;
                if call.url.is_empty() || call.url.len() > 16 * 1024 {
                    return Err("被动检查调用 URL 无效".into());
                }
                call.request.as_ref().ok_or("缺少请求")?.validate()?;
                call.response.as_ref().ok_or("缺少响应")?.validate()?;
            }
            envelope::Body::PassiveCheckResult(res) => {
                if res.findings.len() > 64 || res.log_line.len() > 4096 {
                    return Err("被动检查返回结果超过限额".into());
                }
                validate_findings(&res.findings)?;
            }
            envelope::Body::ActiveCheckCall(call) => {
                uuid(&call.flow_id)?;
                if call.parameter_name.len() > 512 || call.parameter_value.len() > 16 * 1024 {
                    return Err("主动检查调用参数无效或超限".into());
                }
                call.base_request
                    .as_ref()
                    .ok_or("缺少基准请求")?
                    .validate()?;
                if let Some(resp) = &call.base_response {
                    resp.validate()?;
                }
            }
            envelope::Body::ActiveCheckResult(res) => {
                if res.findings.len() > 64 {
                    return Err("主动检查返回结果超过限额".into());
                }
                validate_findings(&res.findings)?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn is_expired(&self) -> bool {
        self.deadline_unix_ms <= unix_ms()
    }

    pub fn check_channel_header(
        &self,
        identity: ChannelIdentity,
        previous_sequence: u64,
    ) -> Result<(), String> {
        self.check_channel_header_for_minor(identity, previous_sequence, PROTOCOL_MINOR)
    }

    pub fn check_channel_header_for_minor(
        &self,
        identity: ChannelIdentity,
        previous_sequence: u64,
        expected_minor: u32,
    ) -> Result<(), String> {
        self.validate_for_minor(expected_minor)?;
        if self.identity()? != identity {
            return Err("实例或项目代际不匹配".into());
        }
        if self.request_id <= previous_sequence {
            return Err("重复或乱序插件消息".into());
        }
        Ok(())
    }

    pub fn check_channel(
        &self,
        identity: ChannelIdentity,
        previous_sequence: u64,
    ) -> Result<(), String> {
        self.check_channel_for_minor(identity, previous_sequence, PROTOCOL_MINOR)
    }

    pub fn check_channel_for_minor(
        &self,
        identity: ChannelIdentity,
        previous_sequence: u64,
        expected_minor: u32,
    ) -> Result<(), String> {
        self.check_channel_header_for_minor(identity, previous_sequence, expected_minor)?;
        if self.is_expired() {
            return Err("插件消息 deadline 已过期".into());
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn validate_finding(finding: &PluginFindingDto) -> Result<(), String> {
    FindingSeverity::try_from(finding.severity).map_err(|_| "插件检查告警 severity 无效")?;
    FindingConfidence::try_from(finding.confidence)
        .map_err(|_| "插件检查告警 confidence 无效")?;
    if finding.title.is_empty()
        || finding.title.len() > 512
        || finding.detail.len() > 8192
        || finding.remediation.len() > 4096
        || finding.parameter.len() > 512
        || finding.evidence.len() > 4096
    {
        return Err("插件检查告警字段无效或超限".into());
    }
    Ok(())
}

fn validate_findings(findings: &[PluginFindingDto]) -> Result<(), String> {
    for finding in findings {
        validate_finding(finding)?;
    }
    Ok(())
}

pub fn frame_length(prefix: [u8; 4]) -> io::Result<usize> {
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(invalid("插件帧长度为零或超过 1 MiB 限额"));
    }
    Ok(length)
}

pub fn decode_frame(payload: &[u8]) -> io::Result<Envelope> {
    decode_frame_for_minor(payload, PROTOCOL_MINOR)
}

pub fn decode_frame_for_minor(payload: &[u8], expected_minor: u32) -> io::Result<Envelope> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(invalid("插件帧长度无效"));
    }
    let envelope = Envelope::decode(payload).map_err(|_| invalid("无法解析插件 Protobuf 帧"))?;
    envelope.validate_for_minor(expected_minor).map_err(invalid)?;
    Ok(envelope)
}

pub fn encode_frame(envelope: &Envelope) -> io::Result<Vec<u8>> {
    encode_frame_for_minor(envelope, PROTOCOL_MINOR)
}

pub fn encode_frame_for_minor(envelope: &Envelope, expected_minor: u32) -> io::Result<Vec<u8>> {
    envelope.validate_for_minor(expected_minor).map_err(invalid)?;
    let length = envelope.encoded_len();
    let mut frame = Vec::with_capacity(length + 4);
    frame.extend_from_slice(&(length as u32).to_be_bytes());
    envelope
        .encode(&mut frame)
        .map_err(|_| invalid("无法编码插件 Protobuf 帧"))?;
    Ok(frame)
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<Envelope> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let length = frame_length(prefix)?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    decode_frame(&payload)
}

pub fn write_frame(writer: &mut impl Write, envelope: &Envelope) -> io::Result<()> {
    writer.write_all(&encode_frame(envelope)?)?;
    writer.flush()
}

/// Synchronous transport belongs in an independent worker, never on the GPUI thread.
pub struct WorkerSession<R, W> {
    reader: R,
    writer: W,
    pub identity: ChannelIdentity,
    pub grants: Vec<Permission>,
    incoming_sequence: u64,
    outgoing_sequence: u64,
    timeout: Duration,
}

impl<R: Read, W: Write> WorkerSession<R, W> {
    pub fn negotiated_minor(&self) -> u32 {
        PROTOCOL_MINOR
    }

    pub fn accept(mut reader: R, mut writer: W, registration: Register) -> io::Result<Self> {
        let hello = read_frame(&mut reader)?;
        let identity = hello.identity().map_err(invalid)?;
        hello.check_channel(identity, 0).map_err(invalid)?;
        let Some(envelope::Body::Hello(limits)) = &hello.body else {
            return Err(invalid("首次消息必须是 Hello"));
        };
        if limits.max_frame_bytes as usize != MAX_FRAME_BYTES || limits.max_operation_ms == 0 {
            return Err(invalid("宿主协议限额不兼容"));
        }
        if limits.host_minor != 0 && limits.host_minor != PROTOCOL_MINOR {
            return Err(invalid("宿主协议版本不兼容"));
        }
        let grants = limits
            .granted_permissions
            .iter()
            .map(|p| Permission::try_from(*p).map_err(|_| invalid("未知宿主授权")))
            .collect::<io::Result<Vec<_>>>()?;
        registration
            .panel
            .as_ref()
            .ok_or_else(|| invalid("注册缺少 Panel"))?
            .validate()
            .map_err(invalid)?;
        let timeout = Duration::from_millis(u64::from(limits.max_operation_ms));
        let mut register = Envelope::new(
            identity,
            1,
            registration.panel.as_ref().unwrap().revision,
            envelope::Body::Register(registration),
            timeout,
        );
        register.in_reply_to = hello.request_id;
        write_frame(&mut writer, &register)?;
        let ready = read_frame(&mut reader)?;
        ready
            .check_channel(identity, hello.request_id)
            .map_err(invalid)?;
        if ready.in_reply_to != 1 || !matches!(ready.body, Some(envelope::Body::Ready(_))) {
            return Err(invalid("宿主未确认插件注册"));
        }
        Ok(Self {
            reader,
            writer,
            identity,
            grants,
            incoming_sequence: ready.request_id,
            outgoing_sequence: 1,
            timeout,
        })
    }

    pub fn receive(&mut self) -> io::Result<Envelope> {
        let envelope = read_frame(&mut self.reader)?;
        envelope
            .check_channel_header(self.identity, self.incoming_sequence)
            .map_err(invalid)?;
        self.incoming_sequence = envelope.request_id;
        Ok(envelope)
    }

    pub fn send(
        &mut self,
        body: envelope::Body,
        revision: u64,
        in_reply_to: u64,
    ) -> io::Result<u64> {
        self.send_on_panel(PANEL_ID, body, revision, in_reply_to)
    }

    pub fn send_on_panel(
        &mut self,
        panel_id: u64,
        body: envelope::Body,
        revision: u64,
        in_reply_to: u64,
    ) -> io::Result<u64> {
        self.outgoing_sequence = self
            .outgoing_sequence
            .checked_add(1)
            .ok_or_else(|| invalid("消息序号耗尽"))?;
        let mut envelope = Envelope::new(
            self.identity,
            self.outgoing_sequence,
            revision,
            body,
            self.timeout,
        )
        .with_panel_id(panel_id);
        envelope.in_reply_to = in_reply_to;
        write_frame(&mut self.writer, &envelope)?;
        Ok(self.outgoing_sequence)
    }
}
