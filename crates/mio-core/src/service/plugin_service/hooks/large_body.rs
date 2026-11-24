use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use super::*;

#[derive(Default)]
pub(in crate::service) struct HookSpillScope {
    dir: Option<tempfile::TempDir>,
    input_paths: Vec<PathBuf>,
}

#[cfg(windows)]
impl Drop for HookSpillScope {
    fn drop(&mut self) {
        for path in &self.input_paths {
            if let Ok(meta) = fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_readonly(false);
                let _ = fs::set_permissions(path, perms);
            }
        }
    }
}

impl HookSpillScope {
    fn ensure_dir(&mut self) -> Result<&Path> {
        if self.dir.is_none() {
            let dir = tempfile::Builder::new()
                .prefix("mio-hook-spill-")
                .tempdir()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))?;
            }
            self.dir = Some(dir);
        }
        Ok(self.dir.as_ref().expect("dir initialized").path())
    }

    fn write_read_only_spill(&mut self, name: &str, bytes: &[u8]) -> Result<String> {
        ensure!(
            bytes.len() <= protocol::MAX_SPILL_BODY_BYTES,
            "大报文正文超过 64 MiB 共享上限"
        );
        let dir = self.ensure_dir()?.to_path_buf();
        let file_path = dir.join(name);
        let path_str = file_path
            .to_str()
            .context("共享大报文文件路径包含非 UTF-8 字符")?
            .to_owned();
        ensure!(
            path_str.len() <= protocol::MAX_BODY_SPILL_PATH_BYTES && !path_str.contains('\0'),
            "共享大报文文件路径超出协议限额"
        );
        fs::write(&file_path, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file_path, fs::Permissions::from_mode(0o400))?;
        }
        #[cfg(windows)]
        {
            let mut perms = fs::metadata(&file_path)?.permissions();
            perms.set_readonly(true);
            fs::set_permissions(&file_path, perms)?;
        }
        if let Ok(canon) = fs::canonicalize(&file_path) {
            self.input_paths.push(canon);
        }
        self.input_paths.push(file_path);
        Ok(path_str)
    }

    fn is_input_spill_path(&self, returned_path: &str) -> bool {
        let candidate = Path::new(returned_path);
        if self.input_paths.iter().any(|p| p == candidate) {
            return true;
        }
        let Ok(canon) = fs::canonicalize(candidate) else {
            return false;
        };
        self.input_paths
            .iter()
            .any(|p| p == &canon || fs::canonicalize(p).is_ok_and(|cp| cp == canon))
    }

    pub(in crate::service) fn resolve_body(
        &self,
        original: &[u8],
        omitted: bool,
        returned_path: &str,
        inline_body: Vec<u8>,
    ) -> std::result::Result<Vec<u8>, String> {
        if !omitted {
            return Ok(inline_body);
        }
        if returned_path.is_empty() || self.is_input_spill_path(returned_path) {
            return Ok(original.to_vec());
        }
        if returned_path.contains('\0')
            || returned_path.len() > protocol::MAX_BODY_SPILL_PATH_BYTES
        {
            return Err("插件返回的共享大报文路径无效或超限".into());
        }
        let Some(dir) = &self.dir else {
            return Err("插件返回了未授权的共享大报文路径（当前调用未创建共享目录）".into());
        };
        let meta = fs::symlink_metadata(returned_path)
            .map_err(|e| format!("无法访问插件返回的共享大报文文件：{e}"))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("插件返回的共享大报文路径不是常规文件（禁止符号链接）".into());
        }
        let canon_dir =
            fs::canonicalize(dir.path()).map_err(|e| format!("解析宿主共享目录失败：{e}"))?;
        let canon_file =
            fs::canonicalize(returned_path).map_err(|e| format!("解析插件共享文件路径失败：{e}"))?;
        if canon_file.parent() != Some(canon_dir.as_path()) {
            return Err("插件返回的共享大报文路径超出当前调用隔离目录".into());
        }
        let canon_meta = fs::symlink_metadata(&canon_file)
            .map_err(|e| format!("无法校验插件共享大报文文件：{e}"))?;
        if !canon_meta.is_file() || canon_meta.len() > protocol::MAX_SPILL_BODY_BYTES as u64 {
            return Err("插件返回的共享大报文文件超过 64 MiB 限额或类型无效".into());
        }
        let mut opts = fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = opts
            .open(&canon_file)
            .map_err(|e| format!("打开插件修改的大报文文件失败：{e}"))?;
        let opened_meta = file
            .metadata()
            .map_err(|e| format!("读取插件大报文文件元数据失败：{e}"))?;
        if !opened_meta.is_file() || opened_meta.len() > protocol::MAX_SPILL_BODY_BYTES as u64 {
            return Err("插件返回的共享大报文文件不是有效常规文件或超过 64 MiB 限额".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if opened_meta.nlink() != 1 {
                return Err("插件返回的共享大报文文件禁止为硬链接".into());
            }
        }
        let mut buf = Vec::with_capacity(
            (opened_meta.len() as usize).min(protocol::MAX_SPILL_BODY_BYTES),
        );
        file.take(protocol::MAX_SPILL_BODY_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| format!("读取插件修改的大报文文件失败：{e}"))?;
        if buf.len() > protocol::MAX_SPILL_BODY_BYTES {
            return Err("插件修改的大报文正文超过 64 MiB 限额".into());
        }
        Ok(buf)
    }
}

fn can_use_v17_spill(protocol_minor: u32, supports_large_body: bool) -> bool {
    protocol_minor >= protocol::PROTOCOL_MINOR && supports_large_body
}

pub(in crate::service) fn request_structure_valid(request: &HttpRequest) -> bool {
    !request.method.is_empty()
        && request.method.len() <= 32
        && !request.url.is_empty()
        && request.url.len() <= 16 * 1024
        && request.body.len() <= protocol::MAX_SPILL_BODY_BYTES
        && request.headers.len() <= 512
        && request.trailers.len() <= 512
}

pub(in crate::service) fn response_structure_valid(response: &HttpResponse) -> bool {
    (100..=599).contains(&response.status)
        && response.body.len() <= protocol::MAX_SPILL_BODY_BYTES
        && response.headers.len() <= 512
        && response.trailers.len() <= 512
}

fn build_request_dto(
    request: &HttpRequest,
    name: &str,
    spill: &mut HookSpillScope,
    force_spill: bool,
) -> Option<protocol::RequestDto> {
    if !force_spill && request.body.len() <= protocol::MAX_INLINE_BODY {
        return Some(request.clone().into());
    }
    let path = spill.write_read_only_spill(name, &request.body).ok()?;
    let mut dto: protocol::RequestDto = HttpRequest {
        method: request.method.clone(),
        url: request.url.clone(),
        version: request.version,
        headers: request.headers.clone(),
        body: Vec::new(),
        trailers: request.trailers.clone(),
    }
    .into();
    dto.body_omitted = true;
    dto.original_body_len = request.body.len() as u64;
    dto.body_file_path = path;
    Some(dto)
}

fn build_response_dto(
    response: &HttpResponse,
    name: &str,
    spill: &mut HookSpillScope,
    force_spill: bool,
) -> Option<protocol::ResponseDto> {
    if !force_spill && response.body.len() <= protocol::MAX_INLINE_BODY {
        return Some(response.clone().into());
    }
    let path = spill.write_read_only_spill(name, &response.body).ok()?;
    let mut dto: protocol::ResponseDto = HttpResponse {
        status: response.status,
        version: response.version,
        reason_phrase: None,
        headers: response.headers.clone(),
        body: Vec::new(),
        trailers: response.trailers.clone(),
    }
    .into();
    dto.body_omitted = true;
    dto.original_body_len = response.body.len() as u64;
    dto.body_file_path = path;
    Some(dto)
}

#[allow(clippy::too_many_arguments)]
fn try_build_pair<T>(
    request: &HttpRequest,
    response: Option<&HttpResponse>,
    req_name: &str,
    resp_name: &str,
    protocol_minor: u32,
    supports_large_body: bool,
    mut make_body: impl FnMut(protocol::RequestDto, Option<protocol::ResponseDto>) -> protocol::envelope::Body,
    extract: impl Fn(protocol::envelope::Body) -> Option<T>,
) -> Option<(T, HookSpillScope)> {
    if !request_structure_valid(request) || !response.is_none_or(response_structure_valid) {
        return None;
    }
    let can_spill = can_use_v17_spill(protocol_minor, supports_large_body);
    if !can_spill {
        if !super::http_request_fits_inline_fast(request)
            || !response.is_none_or(super::http_response_fits_inline_fast)
        {
            return None;
        }
        let body = make_body(request.clone().into(), response.cloned().map(Into::into));
        let valid = protocol::try_into_wire_body(body, protocol_minor).ok()?;
        return extract(valid).map(|t| (t, HookSpillScope::default()));
    }

    let mut spill = HookSpillScope::default();
    let mut spill_req = request.body.len() > protocol::MAX_INLINE_BODY;
    let mut spill_resp = response.is_some_and(|r| r.body.len() > protocol::MAX_INLINE_BODY);

    for _ in 0..3 {
        let req_dto = build_request_dto(request, req_name, &mut spill, spill_req)?;
        let resp_dto = match response {
            Some(r) => Some(build_response_dto(r, resp_name, &mut spill, spill_resp)?),
            None => None,
        };
        let wire = make_body(req_dto, resp_dto);
        if let Ok(valid) = protocol::try_into_wire_body(wire, protocol_minor) {
            return extract(valid).map(|t| (t, spill));
        }
        let req_inline = (!spill_req && !request.body.is_empty()).then_some(request.body.len());
        let resp_inline = response
            .filter(|r| !spill_resp && !r.body.is_empty())
            .map(|r| r.body.len());
        match (req_inline, resp_inline) {
            (Some(rl), Some(sl)) => {
                if rl >= sl {
                    spill_req = true;
                } else {
                    spill_resp = true;
                }
            }
            (Some(_), None) => spill_req = true,
            (None, Some(_)) => spill_resp = true,
            (None, None) => return None,
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(in crate::service) fn prepare_http_hook_call(
    flow_id: FlowId,
    source: i32,
    plugin_id: String,
    response_stage: bool,
    response_head_only: bool,
    request: &HttpRequest,
    response: Option<&HttpResponse>,
    protocol_minor: u32,
    supports_large_body: bool,
) -> Option<(protocol::HttpHookCall, HookSpillScope)> {
    try_build_pair(
        request,
        response,
        "req-body.bin",
        "resp-body.bin",
        protocol_minor,
        supports_large_body,
        |req_dto, resp_dto| {
            protocol::envelope::Body::HttpHookCall(protocol::HttpHookCall {
                flow_id: flow_id.as_bytes().to_vec(),
                source,
                plugin_id: plugin_id.clone(),
                response_stage,
                response_head_only,
                request: Some(req_dto),
                response: resp_dto,
            })
        },
        |body| match body {
            protocol::envelope::Body::HttpHookCall(call) => Some(call),
            _ => None,
        },
    )
}

pub(in crate::service) fn prepare_session_action_call(
    source: i32,
    request: &HttpRequest,
    protocol_minor: u32,
    supports_large_body: bool,
) -> Option<(protocol::SessionActionCall, HookSpillScope)> {
    try_build_pair(
        request,
        None,
        "session-req-body.bin",
        "unused.bin",
        protocol_minor,
        supports_large_body,
        |req_dto, _| {
            protocol::envelope::Body::SessionActionCall(protocol::SessionActionCall {
                source,
                request: Some(req_dto),
            })
        },
        |body| match body {
            protocol::envelope::Body::SessionActionCall(call) => Some(call),
            _ => None,
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::service) fn prepare_ws_hook_call(
    flow_id: FlowId,
    msg_id: Uuid,
    url: &str,
    direction: mio_plugin_api::WebSocketDirection,
    kind: mio_plugin_api::WebSocketKind,
    payload: &[u8],
    protocol_minor: u32,
    supports_large_body: bool,
) -> Option<(protocol::WebSocketHookCall, HookSpillScope)> {
    if url.is_empty()
        || url.len() > 16 * 1024
        || payload.len() > protocol::MAX_SPILL_BODY_BYTES
    {
        return None;
    }
    let can_spill = can_use_v17_spill(protocol_minor, supports_large_body);
    if !can_spill && payload.len() > protocol::MAX_INLINE_BODY {
        return None;
    }
    let mut spill = HookSpillScope::default();
    let mut force_spill = can_spill && payload.len() > protocol::MAX_INLINE_BODY;
    for _ in 0..2 {
        let call = if force_spill {
            let path = spill.write_read_only_spill("ws-payload.bin", payload).ok()?;
            protocol::WebSocketHookCall {
                flow_id: flow_id.as_bytes().to_vec(),
                message_id: msg_id.as_bytes().to_vec(),
                url: url.to_owned(),
                client_to_server: direction == mio_plugin_api::WebSocketDirection::ClientToServer,
                is_binary: kind == mio_plugin_api::WebSocketKind::Binary,
                payload: Vec::new(),
                payload_omitted: true,
                original_payload_len: payload.len() as u64,
                payload_file_path: path,
            }
        } else {
            protocol::WebSocketHookCall {
                flow_id: flow_id.as_bytes().to_vec(),
                message_id: msg_id.as_bytes().to_vec(),
                url: url.to_owned(),
                client_to_server: direction == mio_plugin_api::WebSocketDirection::ClientToServer,
                is_binary: kind == mio_plugin_api::WebSocketKind::Binary,
                payload: payload.to_vec(),
                payload_omitted: false,
                original_payload_len: 0,
                payload_file_path: String::new(),
            }
        };
        if let Ok(protocol::envelope::Body::WebSocketHookCall(valid)) =
            protocol::try_into_wire_body(
                protocol::envelope::Body::WebSocketHookCall(call),
                protocol_minor,
            )
        {
            return Some((valid, spill));
        }
        if can_spill && !force_spill && !payload.is_empty() {
            force_spill = true;
            continue;
        }
        break;
    }
    None
}

pub(in crate::service) fn prepare_passive_check_call(
    flow_id: FlowId,
    url: &str,
    request: &HttpRequest,
    response: &HttpResponse,
    protocol_minor: u32,
    supports_large_body: bool,
) -> Option<(protocol::PassiveCheckCall, HookSpillScope)> {
    if url.is_empty() || url.len() > 16 * 1024 || protocol_minor < protocol::V16_COMPAT_MINOR {
        return None;
    }
    try_build_pair(
        request,
        Some(response),
        "passive-req-body.bin",
        "passive-resp-body.bin",
        protocol_minor,
        supports_large_body,
        |req_dto, resp_dto| {
            protocol::envelope::Body::PassiveCheckCall(protocol::PassiveCheckCall {
                flow_id: flow_id.as_bytes().to_vec(),
                url: url.to_owned(),
                request: Some(req_dto),
                response: resp_dto,
            })
        },
        |body| match body {
            protocol::envelope::Body::PassiveCheckCall(call) => Some(call),
            _ => None,
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::service) fn prepare_active_check_call(
    flow_id: FlowId,
    base_request: &HttpRequest,
    base_response: Option<&HttpResponse>,
    parameter_name: &str,
    parameter_value: &str,
    protocol_minor: u32,
    supports_large_body: bool,
) -> Option<(protocol::ActiveCheckCall, HookSpillScope)> {
    if parameter_name.len() > 512
        || parameter_value.len() > 16 * 1024
        || protocol_minor < protocol::V16_COMPAT_MINOR
    {
        return None;
    }
    try_build_pair(
        base_request,
        base_response,
        "active-req-body.bin",
        "active-resp-body.bin",
        protocol_minor,
        supports_large_body,
        |req_dto, resp_dto| {
            protocol::envelope::Body::ActiveCheckCall(protocol::ActiveCheckCall {
                flow_id: flow_id.as_bytes().to_vec(),
                base_request: Some(req_dto),
                base_response: resp_dto,
                parameter_name: parameter_name.to_owned(),
                parameter_value: parameter_value.to_owned(),
            })
        },
        |body| match body {
            protocol::envelope::Body::ActiveCheckCall(call) => Some(call),
            _ => None,
        },
    )
}

pub(in crate::service) fn resolve_request_dto(
    dto: protocol::RequestDto,
    original_body: &[u8],
    spill: &HookSpillScope,
) -> std::result::Result<HttpRequest, String> {
    dto.validate()?;
    if dto.body_omitted {
        let resolved = spill.resolve_body(original_body, true, &dto.body_file_path, Vec::new())?;
        dto.try_into_with_resolved_body(Some(resolved))
    } else {
        dto.try_into_with_resolved_body(None)
    }
}

pub(in crate::service) fn resolve_response_dto(
    dto: protocol::ResponseDto,
    original_body: &[u8],
    spill: &HookSpillScope,
) -> std::result::Result<HttpResponse, String> {
    dto.validate()?;
    if dto.body_omitted {
        let resolved = spill.resolve_body(original_body, true, &dto.body_file_path, Vec::new())?;
        dto.try_into_with_resolved_body(Some(resolved))
    } else {
        dto.try_into_with_resolved_body(None)
    }
}

pub(in crate::service) fn resolve_ws_result(
    res: protocol::WebSocketHookResult,
    original_payload: &[u8],
    spill: &HookSpillScope,
) -> std::result::Result<Vec<u8>, String> {
    if res.payload.len() > protocol::MAX_INLINE_BODY {
        return Err("WebSocket Hook 结果超过内联限额".into());
    }
    if res.payload_omitted && !res.payload.is_empty() {
        return Err("设置 payload_omitted 时内联载荷必须为空".into());
    }
    if !res.payload_omitted && !res.payload_file_path.is_empty() {
        return Err("未设置 payload_omitted 时不能携带共享文件路径".into());
    }
    spill.resolve_body(
        original_payload,
        res.payload_omitted,
        &res.payload_file_path,
        res.payload,
    )
}
