use std::path::PathBuf;

use anyhow::{Context, Result};
use mio_core::{AiIpcPayload, FlowState, request_to_raw, response_to_raw};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RenderOptions {
    pub show_original: bool,
    pub request_only: bool,
    pub response_only: bool,
    pub headers_only: bool,
    pub save_body: Option<PathBuf>,
}

pub(super) fn format_human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GiB ({bytes} B)", bytes as f64 / GIB)
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MiB ({bytes} B)", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.2} KiB ({bytes} B)", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn headers_only_text(raw: &[u8]) -> String {
    let head = if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        &raw[..pos + 2]
    } else if let Some(pos) = raw.windows(2).position(|w| w == b"\n\n") {
        &raw[..pos + 1]
    } else {
        raw
    };
    String::from_utf8_lossy(head).into_owned()
}

fn abnormal_state_tag(state: &FlowState) -> Option<String> {
    match state {
        FlowState::Complete => None,
        FlowState::Pending => Some("[PENDING]".into()),
        FlowState::Intercepted => Some("[INTERCEPTED]".into()),
        FlowState::Dropped => Some("[DROPPED]".into()),
        FlowState::Cancelled => Some("[CANCELLED]".into()),
        FlowState::Failed(reason) => {
            let brief: String = reason.chars().take(48).collect();
            if brief.is_empty() {
                Some("[FAILED]".into())
            } else {
                Some(format!("[FAILED: {brief}]"))
            }
        }
    }
}

pub(super) fn save_flow_body_if_requested(
    payload: &AiIpcPayload,
    opts: &RenderOptions,
) -> Result<Option<(PathBuf, usize)>> {
    let Some(path) = &opts.save_body else {
        return Ok(None);
    };
    let AiIpcPayload::FlowsGet(detail) = payload else {
        return Ok(None);
    };
    let chosen_req = if opts.show_original {
        detail.original_request.as_ref().unwrap_or(&detail.request)
    } else {
        &detail.request
    };
    let chosen_resp = if opts.show_original {
        detail
            .original_response
            .as_ref()
            .or(detail.response.as_ref())
    } else {
        detail.response.as_ref()
    };
    let body_bytes: &[u8] = if opts.request_only {
        &chosen_req.body
    } else if let Some(resp) = chosen_resp {
        &resp.body
    } else {
        anyhow::bail!("该流量尚无响应报文，无法保存响应体（如需保存请求体请配合 --request-only）");
    };
    std::fs::write(path, body_bytes)
        .with_context(|| format!("写入正文到文件失败：{}", path.display()))?;
    Ok(Some((path.clone(), body_bytes.len())))
}

pub(super) fn render_human(payload: &AiIpcPayload, opts: &RenderOptions) {
    match payload {
        AiIpcPayload::Status {
            project_id,
            project_generation,
            proxy_address,
            intercept_requests,
            intercept_responses,
            total_flows,
            project_disk_bytes,
            process_memory_bytes,
        } => {
            println!("项目 ID:       {project_id} (gen {project_generation})");
            println!(
                "代理监听:      {}",
                proxy_address.as_deref().unwrap_or("未启动")
            );
            println!(
                "拦截开关:      请求={} | 响应={}",
                if *intercept_requests {
                    "开启"
                } else {
                    "关闭"
                },
                if *intercept_responses {
                    "开启"
                } else {
                    "关闭"
                }
            );
            println!("总流量数:      {total_flows}");
            println!("项目占用磁盘:  {}", format_human_bytes(*project_disk_bytes));
            println!(
                "进程物理内存:  {}",
                format_human_bytes(*process_memory_bytes)
            );
        }
        AiIpcPayload::FlowsList { total, flows } => {
            println!("共匹配 {total} 条流量（当前返回 {} 条）：", flows.len());
            for f in flows {
                let num = f
                    .number
                    .map(|n| format!("#{n}"))
                    .unwrap_or_else(|| "#-".into());
                let code = f
                    .status
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".into());
                let state_tag = abnormal_state_tag(&f.state)
                    .map(|tag| format!("  {tag}"))
                    .unwrap_or_default();
                println!(
                    "{:<5}  {}  {:6}  {:4}  {:>7}B  {}{}",
                    num, f.id, f.method, code, f.response_bytes, f.url, state_tag
                );
            }
        }
        AiIpcPayload::FlowsGet(detail) => {
            let summary = &detail.summary;
            let request = &detail.request;
            let response = &detail.response;
            let original_request = &detail.original_request;
            let original_response = &detail.original_response;
            let provenance = &detail.provenance;
            let num_prefix = detail
                .number
                .map(|n| format!("#{n} "))
                .unwrap_or_default();
            println!(
                "# Flow {}{} [{}] {} {}",
                num_prefix,
                summary.id,
                summary.state.label(),
                summary.method,
                summary.url
            );
            if let Some(tid) = detail.tab_id {
                println!("关联 Repeater 标签: {tid}");
            }
            if let FlowState::Failed(reason) = &summary.state
                && !reason.is_empty()
            {
                println!("失败原因: {reason}");
            }
            if !provenance.is_empty() {
                let req_labels: Vec<String> = provenance
                    .request_steps
                    .iter()
                    .map(|s| s.label.clone())
                    .collect();
                if !req_labels.is_empty() {
                    println!("请求变更链: {}", req_labels.join(" -> "));
                }
                let resp_labels: Vec<String> = provenance
                    .response_steps
                    .iter()
                    .map(|s| s.label.clone())
                    .collect();
                if !resp_labels.is_empty() {
                    println!("响应变更链: {}", resp_labels.join(" -> "));
                }
            }
            let chosen_req = if opts.show_original {
                original_request.as_ref().unwrap_or(request)
            } else {
                request
            };
            if !opts.response_only {
                println!("\n--- Request ---");
                let raw = request_to_raw(chosen_req);
                if opts.headers_only {
                    print!("{}", headers_only_text(&raw));
                } else {
                    print!("{}", String::from_utf8_lossy(&raw));
                }
            }
            let chosen_resp = if opts.show_original {
                original_response.as_ref().or(response.as_ref())
            } else {
                response.as_ref()
            };
            if !opts.request_only
                && let Some(resp) = chosen_resp
            {
                println!("\n--- Response ---");
                let raw = response_to_raw(resp);
                if opts.headers_only {
                    print!("{}", headers_only_text(&raw));
                } else {
                    print!("{}", String::from_utf8_lossy(&raw));
                }
            }
        }
        AiIpcPayload::RepeaterList {
            repeater_tabs,
            active_tab_id,
        } => {
            println!("共 {} 个 Repeater 标签：", repeater_tabs.len());
            for tab in repeater_tabs {
                let active_mark = if tab.active || *active_tab_id == Some(tab.tab_id) {
                    "*"
                } else {
                    " "
                };
                let target = if tab.target.is_empty() {
                    "(未配置目标)"
                } else {
                    tab.target.as_str()
                };
                let last = tab
                    .last_flow
                    .map(|f| format!("  last_flow={f}"))
                    .unwrap_or_default();
                println!(
                    "{active_mark} {}  {}  -> {}{}",
                    tab.tab_id, tab.title, target, last
                );
            }
        }
        AiIpcPayload::RepeaterOpen {
            tab_id,
            title,
            target,
        } => {
            let target_str = if target.is_empty() {
                "(空白模板)"
            } else {
                target.as_str()
            };
            println!("已在 Repeater 打开标签 #{title} ({tab_id}) -> {target_str}");
        }
        AiIpcPayload::RepeaterRenamed {
            renamed_tab_id,
            title,
        } => {
            println!("已重命名 Repeater 标签 ({renamed_tab_id}) -> {title}");
        }
        AiIpcPayload::RepeaterClosed {
            closed_tab_id,
            active_tab_id,
        } => {
            let next = active_tab_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "无".into());
            println!("已关闭 Repeater 标签 ({closed_tab_id})，当前激活标签：{next}");
        }
        AiIpcPayload::SlimSave { report } => {
            println!(
                "瘦身另存完成：{} -> {}（清理流量 {} 条，压缩归档块 {} 个）",
                format_human_bytes(report.old_bytes),
                format_human_bytes(report.new_bytes),
                report.removed_flows,
                report.compacted_ext_chunks
            );
        }
        AiIpcPayload::CaExported {
            ca_path,
            ca_format,
            ca_subject,
        } => {
            println!("已导出 Proxy CA ({ca_format}, CN={ca_subject}) -> {ca_path}");
        }
        AiIpcPayload::CaPresets { presets } => {
            for preset in presets {
                println!(
                    "{}  {} — {}",
                    preset.id, preset.display_name, preset.description
                );
            }
        }
        AiIpcPayload::CaPresetApplied {
            preset_id,
            ca_fingerprint,
            ca_subject,
        } => {
            println!(
                "已切换 Proxy CA 为内置预置 {preset_id}（CN={ca_subject}，SHA-256={ca_fingerprint}）"
            );
        }
        AiIpcPayload::ProxyControl {
            proxy_running,
            proxy_address,
            intercept_requests,
            intercept_responses,
        } => {
            println!(
                "代理状态: {} ({}) | 请求拦截: {} | 响应拦截: {}",
                if *proxy_running {
                    "运行中"
                } else {
                    "已停止"
                },
                proxy_address.as_deref().unwrap_or("-"),
                if *intercept_requests {
                    "开启"
                } else {
                    "关闭"
                },
                if *intercept_responses {
                    "开启"
                } else {
                    "关闭"
                },
            );
        }
        AiIpcPayload::ProxyPending { pending_items } => {
            println!("当前挂起拦截数: {}", pending_items.len());
            for item in pending_items {
                println!(
                    "{}  rev={}  {}  {} {}  status={}",
                    item.flow_id,
                    item.revision,
                    item.stage,
                    item.method,
                    item.url,
                    item.status
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "-".into())
                );
            }
        }
        AiIpcPayload::ProxyDecided {
            decided_flow_id,
            dropped,
        } => {
            println!(
                "已处理拦截流量 {decided_flow_id} ({})",
                if *dropped { "已丢弃" } else { "已放行" }
            );
        }
        AiIpcPayload::WorkspaceState { workspace } => {
            println!(
                "{}",
                serde_json::to_string_pretty(workspace).unwrap_or_else(|_| workspace.to_string())
            );
        }
        AiIpcPayload::WebSocketList {
            ws_total,
            ws_messages,
            ..
        } => {
            println!(
                "共记录 {ws_total} 条 WebSocket 帧（当前返回 {} 条）：",
                ws_messages.len()
            );
            for m in ws_messages {
                println!(
                    "{}  flow={}  seq={}  {:?}  {:?}  {}B  {}",
                    m.id,
                    m.flow_id,
                    m.sequence,
                    m.direction,
                    m.kind,
                    m.payload.len(),
                    String::from_utf8_lossy(&m.payload)
                );
            }
        }
        AiIpcPayload::Shutdown { shutting_down } => {
            println!("MIO 后台服务正在关闭: {shutting_down}");
        }
    }
}
