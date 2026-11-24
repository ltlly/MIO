use mio_plugin_sdk::ui;

use super::*;

impl Lab {
    pub(crate) fn set_header(headers: &mut Vec<HeaderDto>, name: &str, value: &[u8]) -> bool {
        let mut found = false;
        for header in headers.iter_mut() {
            if header.name.eq_ignore_ascii_case(name) {
                header.value = value.to_vec();
                found = true;
            }
        }
        if found {
            return true;
        }
        if headers.len() >= 512 {
            return false;
        }
        headers.push(HeaderDto {
            name: name.into(),
            value: value.to_vec(),
        });
        true
    }

    pub(crate) fn http_hook(&self, call: HttpHookCall) -> Result<HttpHookResult, String> {
        let mut req = call.request.ok_or("缺少请求")?;
        req.validate()?;
        if !call.response_stage {
            let action_val = req
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("x-mio-lab-action"))
                .and_then(|h| std::str::from_utf8(&h.value).ok())
                .map(|v| v.trim());
            if action_val.is_some_and(|v| v.eq_ignore_ascii_case("drop")) {
                return Ok(HttpHookResult {
                    action: HookAction::Drop as i32,
                    request: None,
                    response: None,
                });
            }
            if action_val.is_some_and(|v| v.eq_ignore_ascii_case("intercept")) {
                return Ok(HttpHookResult {
                    action: HookAction::Intercept as i32,
                    request: None,
                    response: None,
                });
            }
            if (self.auto_tag_hooks || action_val.is_some_and(|v| v.eq_ignore_ascii_case("modify")))
                && Self::set_header(&mut req.headers, "x-protocol-lab", b"hooked")
            {
                return Ok(HttpHookResult {
                    action: HookAction::Modify as i32,
                    request: Some(req),
                    response: None,
                });
            }
            return Ok(HttpHookResult {
                action: HookAction::Pass as i32,
                request: None,
                response: None,
            });
        }
        if self.auto_tag_hooks
            && let Some(mut resp) = call.response
        {
            resp.validate()?;
            if Self::set_header(&mut resp.headers, "x-protocol-lab-resp", b"observed") {
                return Ok(HttpHookResult {
                    action: HookAction::Modify as i32,
                    request: None,
                    response: Some(resp),
                });
            }
        }
        Ok(HttpHookResult {
            action: HookAction::Pass as i32,
            request: None,
            response: None,
        })
    }

    pub(crate) fn ws_hook(&self, call: WebSocketHookCall) -> WebSocketHookResult {
        if call.payload == b"[lab:drop]" {
            return WebSocketHookResult {
                action: HookAction::Drop as i32,
                payload: vec![],
                ..Default::default()
            };
        }
        if call.payload.starts_with(b"[lab:intercept]") {
            return WebSocketHookResult {
                action: HookAction::Intercept as i32,
                payload: vec![],
                ..Default::default()
            };
        }
        if (self.auto_tag_hooks || call.payload.starts_with(b"[lab:tag]"))
            && call.payload.len() + 6 <= MAX_INLINE_BODY
        {
            let mut tagged = Vec::with_capacity(6 + call.payload.len());
            tagged.extend_from_slice(b"[lab] ");
            tagged.extend_from_slice(&call.payload);
            return WebSocketHookResult {
                action: HookAction::Modify as i32,
                payload: tagged,
                ..Default::default()
            };
        }
        WebSocketHookResult {
            action: HookAction::Pass as i32,
            payload: vec![],
            ..Default::default()
        }
    }

    pub(crate) fn session_action(
        &self,
        call: SessionActionCall,
    ) -> Result<SessionActionResult, String> {
        let mut req = call.request.ok_or("缺少请求")?;
        req.validate()?;
        let has_marker = req
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("x-lab-session"));
        if (self.auto_tag_hooks || has_marker)
            && Self::set_header(&mut req.headers, "x-protocol-lab-session", b"signed-v1")
        {
            return Ok(SessionActionResult {
                modified: true,
                request: Some(req),
            });
        }
        Ok(SessionActionResult {
            modified: false,
            request: None,
        })
    }

    pub(crate) fn payload(&self, call: PayloadCall) -> Result<PayloadResult, String> {
        match PayloadOp::try_from(call.op).map_err(|_| "未知 Payload 操作")? {
            PayloadOp::Generate => {
                let max_items = (call.max_items as usize).min(10_000);
                let mut outputs = Vec::new();
                let mut total_bytes = 0usize;
                let configured: Vec<&str> = call
                    .config
                    .split([',', '\n'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if !configured.is_empty() {
                    for item in configured.into_iter().take(max_items) {
                        let bytes = item.as_bytes();
                        if total_bytes + bytes.len() > MAX_INLINE_BODY {
                            break;
                        }
                        total_bytes += bytes.len();
                        outputs.push(bytes.to_vec());
                    }
                } else {
                    let count = call.max_items.clamp(1, 5);
                    for i in 1..=count {
                        outputs.push(format!("lab-payload-{i}").into_bytes());
                    }
                }
                Ok(PayloadResult { outputs })
            }
            PayloadOp::Process => {
                let prefix = if call.config.is_empty() {
                    b"lab:".as_slice()
                } else {
                    call.config.as_bytes()
                };
                let mut outputs = Vec::with_capacity(call.inputs.len().min(10_000));
                let mut total_bytes = 0usize;
                for input in call.inputs.into_iter().take(10_000) {
                    let len = prefix.len().saturating_add(input.len());
                    if total_bytes.saturating_add(len) > MAX_INLINE_BODY {
                        break;
                    }
                    let mut item = Vec::with_capacity(len);
                    item.extend_from_slice(prefix);
                    item.extend_from_slice(&input);
                    total_bytes += item.len();
                    outputs.push(item);
                }
                Ok(PayloadResult { outputs })
            }
        }
    }

    pub(crate) fn commit_response(
        &mut self,
        request: CommitResponseEditor,
    ) -> Result<ResponseCommitted, String> {
        let context = self
            .response_context
            .as_ref()
            .ok_or("没有响应消息编辑上下文")?;
        if context.read_only
            || protocol::uuid(&request.resource_id)? != context.resource
            || request.message_revision != context.revision
        {
            return Err("只读、旧 revision 或跨资源的响应编辑提交已拒绝".into());
        }
        let base: HttpResponse = request.response.ok_or("缺少基准响应")?.try_into()?;
        if base != context.response {
            return Err("相同 message revision 的基准响应已改变".into());
        }
        let mut updated = context.response.clone();
        let snapshot = request.editor.ok_or("缺少最新 EditorSnapshot")?;
        self.sync_editor(&snapshot)?;
        updated.body = self.body(snapshot.document_id)?;
        Ok(ResponseCommitted {
            resource_id: request.resource_id,
            message_revision: request.message_revision,
            document_id: snapshot.document_id,
            document_revision: snapshot.revision,
            response: Some(updated.into()),
        })
    }

    pub(crate) fn context_menu(
        &mut self,
        call: ContextMenuCall,
    ) -> Result<ContextMenuResult, String> {
        let req = call.request.ok_or("缺少上下文请求")?;
        req.validate()?;
        if let Some(resp) = &call.response {
            resp.validate()?;
        }
        let mut next = req.clone();
        let _ = Self::set_header(&mut next.headers, "x-protocol-lab-context", b"menu-v1");
        let replace_request = if self.grants.contains(&Permission::DraftWrite)
            && (call.action_id == "replace"
                || (!call.selected_bytes.is_empty() && call.action_id != "repeater"))
        {
            if !call.selected_bytes.is_empty() {
                next.body = self.encode_b64(&call.selected_bytes).into_bytes();
                let len = next.body.len().to_string().into_bytes();
                let _ = Self::set_header(&mut next.headers, "Content-Length", &len);
            }
            Some(Self::request_dto_to_raw(&next))
        } else {
            None
        };
        let open_repeater = if self.grants.contains(&Permission::RepeaterOpen) {
            Some(next)
        } else {
            None
        };
        let log_line = format!(
            "lab.protocol 上下文菜单：{} {}（动作={}）",
            req.method, req.url, call.action_id
        );
        let status_message = if open_repeater.is_some() {
            format!(
                "已由插件上下文菜单生成 Repeater 草稿：{} {}",
                req.method, req.url
            )
        } else {
            format!("已执行插件上下文动作：{} {}", req.method, req.url)
        };
        self.status.clone_from(&status_message);
        self.error = false;
        Ok(ContextMenuResult {
            status_message,
            log_line,
            open_repeater,
            replace_request,
        })
    }

    fn request_dto_to_raw(req: &HttpRequestDto) -> Vec<u8> {
        let path = req
            .url
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|idx| &rest[idx..]))
            .unwrap_or("/");
        let ver = match HttpVersionDto::try_from(req.version).unwrap_or(HttpVersionDto::Http11) {
            HttpVersionDto::Http10 => "HTTP/1.0",
            HttpVersionDto::Http11 => "HTTP/1.1",
            HttpVersionDto::Http2 => "HTTP/2",
        };
        let mut raw = format!("{} {} {}\r\n", req.method, path, ver).into_bytes();
        for h in &req.headers {
            raw.extend_from_slice(h.name.as_bytes());
            raw.extend_from_slice(b": ");
            raw.extend_from_slice(&h.value);
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(&req.body);
        raw
    }

    pub(crate) fn passive_check(
        &self,
        call: PassiveCheckCall,
    ) -> Result<PassiveCheckResult, String> {
        let flow_id = protocol::uuid(&call.flow_id)?;
        let req = call.request.ok_or("缺少请求")?;
        req.validate()?;
        let resp = call.response.ok_or("缺少响应")?;
        resp.validate()?;
        let header_hit = resp.headers.iter().find_map(|h| {
            let name_hit = h.name.eq_ignore_ascii_case("x-plugin-audit-flag")
                || h.name.to_ascii_lowercase().contains("x-plugin-audit-flag");
            let val_str = String::from_utf8_lossy(&h.value);
            let val_hit = val_str.to_ascii_lowercase().contains("x-plugin-audit-flag")
                || val_str.contains("PLUGIN_VULN_MARKER");
            if name_hit {
                Some((h.name.clone(), format!("{}: {}", h.name, val_str.trim())))
            } else if val_hit {
                Some((h.name.clone(), val_str.trim().to_owned()))
            } else {
                None
            }
        });
        let body_str = String::from_utf8_lossy(&resp.body);
        let body_hit = if body_str.contains("PLUGIN_VULN_MARKER") {
            Some(("response_body".to_owned(), "PLUGIN_VULN_MARKER".to_owned()))
        } else if body_str
            .to_ascii_lowercase()
            .contains("x-plugin-audit-flag")
        {
            Some(("response_body".to_owned(), "x-plugin-audit-flag".to_owned()))
        } else {
            None
        };
        let mut findings = Vec::new();
        if let Some((parameter, evidence)) = header_hit.or(body_hit) {
            findings.push(
                ui::finding("插件被动合规检查命中敏感标记")
                    .severity(FindingSeverity::High)
                    .confidence(FindingConfidence::Certain)
                    .detail(format!(
                        "lab.protocol 在流量 {} ({}) 的响应中检测到调试或漏洞标记 `{}`。",
                        flow_id, call.url, evidence
                    ))
                    .remediation(
                        "移除响应中的 x-plugin-audit-flag 调试头与 PLUGIN_VULN_MARKER 标记。",
                    )
                    .parameter(parameter)
                    .evidence(evidence)
                    .build(),
            );
        }
        let log_line = format!(
            "lab.protocol passive_check: flow={} findings={}",
            flow_id,
            findings.len()
        );
        Ok(PassiveCheckResult { findings, log_line })
    }

    pub fn active_check(&self, call: ActiveCheckCall) -> Result<ActiveCheckResult, String> {
        let flow_id = protocol::uuid(&call.flow_id)?;
        let req = call.base_request.ok_or("缺少基准请求")?;
        req.validate()?;
        if let Some(resp) = &call.base_response {
            resp.validate()?;
        }
        let resp_body = call
            .base_response
            .as_ref()
            .map(|r| String::from_utf8_lossy(&r.body).into_owned())
            .unwrap_or_default();
        let param_name = if call.parameter_name.trim().is_empty() {
            "query".to_owned()
        } else {
            call.parameter_name.trim().to_owned()
        };
        let val = call.parameter_value.trim();
        let mut findings = Vec::new();
        if val.contains("PLUGIN_ACTIVE_VULN")
            || val.contains("PLUGIN_VULN_MARKER")
            || resp_body.contains("PLUGIN_ACTIVE_VULN")
            || resp_body.contains("PLUGIN_VULN_MARKER")
            || (!val.is_empty() && resp_body.contains(val))
        {
            let evidence = if !val.is_empty() {
                val.to_owned()
            } else {
                "PLUGIN_ACTIVE_VULN".to_owned()
            };
            findings.push(
                ui::finding("插件主动扫描检查命中注入或回显点")
                    .severity(FindingSeverity::High)
                    .confidence(FindingConfidence::Firm)
                    .detail(format!(
                        "lab.protocol 在流量 {} ({}) 的参数 `{}` 上检测到主动扫描特征 `{}`。",
                        flow_id, req.url, param_name, evidence
                    ))
                    .remediation("对参数输入执行上下文转义与白名单校验。")
                    .parameter(param_name)
                    .evidence(evidence)
                    .build(),
            );
        }
        Ok(ActiveCheckResult { findings })
    }
}

#[cfg(test)]
mod active_check_tests {
    use super::*;

    #[test]
    fn active_check_detects_reflected_or_flagged_parameter() {
        let lab = Lab::new();
        let flow_id = Uuid::new_v4().as_bytes().to_vec();
        let base_request = Some(HttpRequestDto {
            method: "GET".into(),
            url: "http://127.0.0.1/item?id=1".into(),
            version: HttpVersionDto::Http11 as i32,
            ..Default::default()
        });
        let base_response = Some(HttpResponseDto {
            status: 200,
            version: HttpVersionDto::Http11 as i32,
            body: b"value=probe_token_123".to_vec(),
            ..Default::default()
        });
        let hit = lab
            .active_check(ActiveCheckCall {
                flow_id: flow_id.clone(),
                base_request: base_request.clone(),
                base_response,
                parameter_name: "id".into(),
                parameter_value: "probe_token_123".into(),
            })
            .unwrap();
        assert_eq!(hit.findings.len(), 1);
        assert_eq!(hit.findings[0].parameter, "id");

        let clean = lab
            .active_check(ActiveCheckCall {
                flow_id,
                base_request,
                base_response: None,
                parameter_name: "id".into(),
                parameter_value: "harmless".into(),
            })
            .unwrap();
        assert!(clean.findings.is_empty());
    }
}
