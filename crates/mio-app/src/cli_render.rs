use mio_core::{AiIpcPayload, request_to_raw, response_to_raw};

pub(super) fn render_human(payload: &AiIpcPayload, show_original: bool) {
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
            println!("项目占用磁盘:  {project_disk_bytes} bytes");
            println!("进程物理内存:  {process_memory_bytes} bytes");
        }
        AiIpcPayload::FlowsList { total, flows } => {
            println!("共匹配 {total} 条流量（当前返回 {} 条）：", flows.len());
            for f in flows {
                let code = f
                    .status
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".into());
                println!(
                    "{}  {:6}  {:4}  {:>7}B  {}",
                    f.id, f.method, code, f.response_bytes, f.url
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
            println!(
                "# Flow {} [{}] {} {}",
                summary.id,
                summary.state.label(),
                summary.method,
                summary.url
            );
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
            let chosen_req = if show_original {
                original_request.as_ref().unwrap_or(request)
            } else {
                request
            };
            println!("\n--- Request ---");
            print!("{}", String::from_utf8_lossy(&request_to_raw(chosen_req)));
            let chosen_resp = if show_original {
                original_response.as_ref().or(response.as_ref())
            } else {
                response.as_ref()
            };
            if let Some(resp) = chosen_resp {
                println!("\n--- Response ---");
                print!("{}", String::from_utf8_lossy(&response_to_raw(resp)));
            }
        }
        AiIpcPayload::RepeaterOpen {
            tab_id,
            title,
            target,
        } => {
            println!("已在 Repeater 打开标签 #{title} ({tab_id}) -> {target}");
        }
        AiIpcPayload::SlimSave { report } => {
            println!(
                "瘦身另存完成：{} -> {} bytes（清理流量 {} 条，压缩归档块 {} 个）",
                report.old_bytes,
                report.new_bytes,
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
        AiIpcPayload::WorkspaceState { workspace_json, .. } => {
            println!("{workspace_json}");
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

pub(super) fn print_usage() {
    println!(
        "用法 (支持环境变量 MIO_IPC_SOCKET / MIO_SOCKET 指定默认 socket 路径):
  mio-cli [--socket <path>] status [--json]
  mio-cli [--socket <path>] flows list [--filter <str>] [--method <M>] [--status <code>] [--offset <n>] [--limit <n>] [--json]
  mio-cli [--socket <path>] flows get <flow-id> [--original] [--json]
  mio-cli [--socket <path>] ws list [--limit <n>] [--json]
  mio-cli [--socket <path>] repeater open (--flow <flow-id> [--original] | --target <origin> --raw <raw-request>) [--title <title>] [--json]
  mio-cli [--socket <path>] repeater send (--flow <flow-id> [--original] | --target <origin> --raw <raw-request>) [--set-header|--add-header <K:V>] [--remove-header <K>] [--body <str>] [--insecure] [--json]
  mio-cli [--socket <path>] slim-save <dest-path> [--json]
  mio-cli [--socket <path>] ca export <dest-path> [--pem|--der] [--json]
  mio-cli [--socket <path>] proxy start [--listen <addr>] [--json]
  mio-cli [--socket <path>] proxy stop [--json]
  mio-cli [--socket <path>] proxy intercept [--requests on|off] [--responses on|off] [--json]
  mio-cli [--socket <path>] proxy pending [--json]
  mio-cli [--socket <path>] proxy decide <flow-id> --revision <rev> (--forward [--set-header|--add-header <K:V>] [--remove-header <K>] [--body <str>] [--status <code>] | --drop) [--json]
  mio-cli [--socket <path>] workspace get [--json]
  mio-cli [--socket <path>] workspace set <json-file> [--json]
  mio-cli [--socket <path>] shutdown [--json]
  mio-cli [--socket <path>] serve [--listen <addr>] [--export-ca <path>]"
    );
}
