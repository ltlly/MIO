use std::{path::PathBuf, process::ExitCode, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use mio_core::{AiIpcRequest, Uuid, call_ai_ipc, default_ipc_socket_path};

#[path = "../cli_render.rs"]
mod render;

const CLI_TIMEOUT: Duration = Duration::from_secs(30);
const CLI_REPEATER_SEND_TIMEOUT: Duration = Duration::from_secs(305);

#[derive(Clone, Debug, PartialEq, Eq)]
struct CliInvocation {
    socket: Option<PathBuf>,
    json: bool,
    command: CliCommand,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CliCommand {
    Help,
    Status,
    FlowsList {
        filter: String,
        method: Option<String>,
        status: Option<u16>,
        offset: usize,
        limit: usize,
    },
    FlowsGet {
        flow_id: Uuid,
        original: bool,
    },
    RepeaterOpen {
        flow_id: Option<Uuid>,
        use_original: bool,
        target: Option<String>,
        raw_request: Option<String>,
        title: Option<String>,
    },
    RepeaterSend {
        flow_id: Option<Uuid>,
        use_original: bool,
        target: Option<String>,
        raw_request: Option<String>,
        add_headers: Vec<(String, String)>,
        remove_headers: Vec<String>,
        override_body: Option<String>,
        insecure: bool,
    },
    SlimSave {
        dest_path: String,
    },
    CaExport {
        dest_path: String,
        pem: Option<bool>,
    },
    ProxyStart {
        bind_address: Option<String>,
    },
    ProxyStop,
    ProxyIntercept {
        requests: Option<bool>,
        responses: Option<bool>,
    },
    ProxyPending,
    ProxyDecide {
        flow_id: Uuid,
        revision: u64,
        drop_flow: bool,
        add_headers: Vec<(String, String)>,
        remove_headers: Vec<String>,
        override_body: Option<String>,
        override_status: Option<u16>,
    },
    WorkspaceGet,
    WorkspaceSet {
        json_path: PathBuf,
    },
    WebSocketList {
        limit: usize,
    },
    Shutdown,
    Serve {
        listen: Option<String>,
        export_ca: Option<String>,
    },
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wants_json = has_global_json_flag(&args);
    match execute_cli(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let msg = format!("{error:#}");
            if wants_json {
                println!("{}", mio_core::AiIpcEnvelope::err_json(msg));
            } else {
                eprintln!("mio-cli 错误：{msg}");
            }
            ExitCode::FAILURE
        }
    }
}

fn execute_cli(args: &[String]) -> Result<()> {
    let invocation = parse_cli(args)?;
    if matches!(invocation.command, CliCommand::Help) {
        render::print_usage();
        return Ok(());
    }
    let socket_path = invocation.socket.unwrap_or_else(default_ipc_socket_path);
    if let CliCommand::Serve { listen, export_ca } = invocation.command {
        return run_headless_server(socket_path, listen, export_ca, invocation.json);
    }
    let (req, show_original) = match invocation.command {
        CliCommand::Help | CliCommand::Serve { .. } => unreachable!(),
        CliCommand::Status => (AiIpcRequest::Status, false),
        CliCommand::FlowsList {
            filter,
            method,
            status,
            offset,
            limit,
        } => (
            AiIpcRequest::FlowsList {
                offset,
                limit,
                filter,
                method,
                status,
            },
            false,
        ),
        CliCommand::FlowsGet { flow_id, original } => {
            (AiIpcRequest::FlowsGet { flow_id }, original)
        }
        CliCommand::RepeaterOpen {
            flow_id,
            use_original,
            target,
            raw_request,
            title,
        } => (
            AiIpcRequest::RepeaterOpen {
                flow_id,
                use_original,
                target,
                raw_request,
                title,
            },
            false,
        ),
        CliCommand::RepeaterSend {
            flow_id,
            use_original,
            target,
            raw_request,
            add_headers,
            remove_headers,
            override_body,
            insecure,
        } => (
            AiIpcRequest::RepeaterSend {
                flow_id,
                use_original,
                target,
                raw_request,
                add_headers,
                remove_headers,
                override_body,
                insecure,
            },
            false,
        ),
        CliCommand::SlimSave { dest_path } => (
            AiIpcRequest::SlimSave {
                path: dest_path,
                options: None,
            },
            false,
        ),
        CliCommand::CaExport { dest_path, pem } => (
            AiIpcRequest::ExportCa {
                path: dest_path,
                pem,
            },
            false,
        ),
        CliCommand::ProxyStart { bind_address } => {
            (AiIpcRequest::ProxyStart { bind_address }, false)
        }
        CliCommand::ProxyStop => (AiIpcRequest::ProxyStop, false),
        CliCommand::ProxyIntercept {
            requests,
            responses,
        } => (
            AiIpcRequest::ProxyIntercept {
                requests,
                responses,
            },
            false,
        ),
        CliCommand::ProxyPending => (AiIpcRequest::ProxyPending, false),
        CliCommand::ProxyDecide {
            flow_id,
            revision,
            drop_flow,
            add_headers,
            remove_headers,
            override_body,
            override_status,
        } => (
            AiIpcRequest::ProxyDecide {
                flow_id,
                revision,
                drop_flow,
                add_header: None,
                add_headers,
                remove_headers,
                override_body,
                override_status,
            },
            false,
        ),
        CliCommand::WorkspaceGet => (AiIpcRequest::WorkspaceGet, false),
        CliCommand::WorkspaceSet { json_path } => {
            let workspace_json = std::fs::read_to_string(&json_path)
                .with_context(|| format!("读取工作区 JSON 失败：{}", json_path.display()))?;
            (AiIpcRequest::WorkspacePatch { workspace_json }, false)
        }
        CliCommand::WebSocketList { limit } => (AiIpcRequest::WebSocketList { limit }, false),
        CliCommand::Shutdown => (AiIpcRequest::Shutdown, false),
    };
    let timeout = if matches!(req, AiIpcRequest::RepeaterSend { .. }) {
        CLI_REPEATER_SEND_TIMEOUT
    } else {
        CLI_TIMEOUT
    };
    let (env, raw_line) = call_ai_ipc(&socket_path, &req, timeout)?;
    if invocation.json {
        if !env.ok {
            bail!("{}", env.error.unwrap_or_else(|| "IPC 请求失败".into()));
        }
        println!("{}", raw_line.trim());
        return Ok(());
    }
    if !env.ok {
        bail!("{}", env.error.unwrap_or_else(|| "IPC 请求失败".into()));
    }
    let payload = env.payload.context("IPC 成功响应缺少数据载荷")?;
    render::render_human(&payload, show_original);
    Ok(())
}

fn subcommand_flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--filter"
            | "--method"
            | "--status"
            | "--offset"
            | "--limit"
            | "--flow"
            | "--target"
            | "--raw"
            | "--title"
            | "--add-header"
            | "--set-header"
            | "--remove-header"
            | "--body"
            | "--revision"
            | "--listen"
            | "--export-ca"
            | "--requests"
            | "--responses"
    )
}

fn has_global_json_flag(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" => i += 2,
            "--json" => return true,
            flag if subcommand_flag_takes_value(flag) => i += 2,
            _ => i += 1,
        }
    }
    false
}

fn parse_cli(args: &[String]) -> Result<CliInvocation> {
    if args.is_empty() {
        return Ok(CliInvocation {
            socket: None,
            json: false,
            command: CliCommand::Help,
        });
    }
    let mut socket: Option<PathBuf> = None;
    let mut json = false;
    let mut rest = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" => {
                let val = args.get(i + 1).context("--socket 需要提供路径参数")?;
                ensure!(!val.is_empty(), "--socket 路径不能为空");
                socket = Some(PathBuf::from(val));
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "-h" | "--help" => {
                return Ok(CliInvocation {
                    socket,
                    json,
                    command: CliCommand::Help,
                });
            }
            "help" if rest.is_empty() && args.len() == i + 1 => {
                return Ok(CliInvocation {
                    socket,
                    json,
                    command: CliCommand::Help,
                });
            }
            flag if subcommand_flag_takes_value(flag) => {
                rest.push(flag.to_owned());
                if let Some(val) = args.get(i + 1) {
                    rest.push(val.clone());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            other => {
                rest.push(other.to_owned());
                i += 1;
            }
        }
    }
    ensure!(!rest.is_empty(), "缺少子命令；运行 mio-cli --help 查看用法");
    let command = match rest[0].as_str() {
        "status" => {
            ensure!(rest.len() == 1, "status 不接受额外位置参数");
            CliCommand::Status
        }
        "flows" => parse_flows_subcommand(&rest[1..])?,
        "ws" => parse_ws_subcommand(&rest[1..])?,
        "repeater" => parse_repeater_subcommand(&rest[1..])?,
        "slim-save" => {
            ensure!(rest.len() == 2, "用法：mio-cli slim-save <dest-path>");
            CliCommand::SlimSave {
                dest_path: rest[1].clone(),
            }
        }
        "ca" => parse_ca_subcommand(&rest[1..])?,
        "proxy" => parse_proxy_subcommand(&rest[1..])?,
        "workspace" => parse_workspace_subcommand(&rest[1..])?,
        "shutdown" => {
            ensure!(rest.len() == 1, "shutdown 不接受额外位置参数");
            CliCommand::Shutdown
        }
        "serve" => parse_serve_subcommand(&rest[1..])?,
        other => bail!(
            "未知子命令 `{other}`；支持 status, flows, ws, repeater, slim-save, ca, proxy, workspace, shutdown, serve"
        ),
    };
    Ok(CliInvocation {
        socket,
        json,
        command,
    })
}

fn run_headless_server(
    socket_path: PathBuf,
    listen: Option<String>,
    export_ca: Option<String>,
    json: bool,
) -> Result<()> {
    let (runtime, mut client) = mio_core::Runtime::start_with_ipc_socket(socket_path.clone())?;
    let shutdown_token = client.shutdown.clone();
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();
    let drain_handle = std::thread::spawn(move || {
        while client.events.blocking_recv().is_some() {}
        let _ = done_tx.send(());
    });
    if let Some(ca_dest) = export_ca {
        let (env, _) = call_ai_ipc(
            &socket_path,
            &AiIpcRequest::ExportCa {
                path: ca_dest,
                pem: None,
            },
            CLI_TIMEOUT,
        )?;
        if !env.ok {
            let _ = shutdown_token.cancel();
            bail!("{}", env.error.unwrap_or_else(|| "导出 CA 失败".into()));
        }
    }
    let mut active_proxy: Option<String> = None;
    if let Some(addr) = listen {
        let (env, _) = call_ai_ipc(
            &socket_path,
            &AiIpcRequest::ProxyStart {
                bind_address: Some(addr),
            },
            CLI_TIMEOUT,
        )?;
        if !env.ok {
            let _ = shutdown_token.cancel();
            bail!("{}", env.error.unwrap_or_else(|| "启动代理失败".into()));
        }
        if let Some(mio_core::AiIpcPayload::ProxyControl { proxy_address, .. }) = env.payload {
            active_proxy = proxy_address;
        }
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ready": true,
                "socket": socket_path.to_string_lossy(),
                "proxy_address": active_proxy,
            })
        );
    } else {
        println!(
            "MIO headless 服务已就绪 | IPC={} | Proxy={}",
            socket_path.display(),
            active_proxy.as_deref().unwrap_or("未启动")
        );
    }
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let interrupted = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
        .is_some_and(|rt| {
            rt.block_on(async {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => true,
                    _ = &mut done_rx => false,
                }
            })
        });
    let join_res = if interrupted {
        runtime.shutdown()
    } else {
        runtime.join()
    };
    let _ = drain_handle.join();
    join_res
}

fn parse_ws_subcommand(args: &[String]) -> Result<CliCommand> {
    let sub = args
        .first()
        .map(String::as_str)
        .context("ws 需要子命令：list")?;
    ensure!(sub == "list", "未知 ws 子命令 `{sub}`；仅支持 list");
    let mut limit = 50usize;
    let mut idx = 1;
    while idx < args.len() {
        match args[idx].as_str() {
            "--limit" => {
                let raw = args.get(idx + 1).context("--limit 需要正整数")?;
                limit = raw.parse::<usize>().context("--limit 必须为正整数")?;
                ensure!((1..=500).contains(&limit), "--limit 必须在 1..=500 之间");
                idx += 2;
            }
            flag => bail!("ws list 不支持参数 `{flag}`"),
        }
    }
    Ok(CliCommand::WebSocketList { limit })
}

fn parse_ca_subcommand(args: &[String]) -> Result<CliCommand> {
    let sub = args
        .first()
        .map(String::as_str)
        .context("ca 需要子命令：export")?;
    ensure!(sub == "export", "未知 ca 子命令 `{sub}`；仅支持 export");
    let mut dest_path: Option<String> = None;
    let mut pem: Option<bool> = None;
    for arg in &args[1..] {
        match arg.as_str() {
            "--pem" => {
                ensure!(pem != Some(false), "不能同时指定 --pem 与 --der");
                pem = Some(true);
            }
            "--der" => {
                ensure!(pem != Some(true), "不能同时指定 --pem 与 --der");
                pem = Some(false);
            }
            val if !val.starts_with('-') && dest_path.is_none() => {
                dest_path = Some(val.to_owned());
            }
            other => bail!("ca export 不支持参数 `{other}`"),
        }
    }
    Ok(CliCommand::CaExport {
        dest_path: dest_path.context("用法：mio-cli ca export <dest-path> [--pem|--der]")?,
        pem,
    })
}

fn parse_proxy_subcommand(args: &[String]) -> Result<CliCommand> {
    let sub = args
        .first()
        .map(String::as_str)
        .context("proxy 需要子命令：start, stop, intercept, pending 或 decide")?;
    match sub {
        "start" => {
            let mut bind_address = None;
            let mut idx = 1;
            while idx < args.len() {
                match args[idx].as_str() {
                    "--listen" => {
                        bind_address =
                            Some(args.get(idx + 1).context("--listen 需要地址参数")?.clone());
                        idx += 2;
                    }
                    flag => bail!("proxy start 不支持参数 `{flag}`"),
                }
            }
            Ok(CliCommand::ProxyStart { bind_address })
        }
        "stop" => {
            ensure!(args.len() == 1, "proxy stop 不接受额外参数");
            Ok(CliCommand::ProxyStop)
        }
        "intercept" => {
            let mut requests = None;
            let mut responses = None;
            let mut idx = 1;
            while idx < args.len() {
                match args[idx].as_str() {
                    "--requests" => {
                        let val = args.get(idx + 1).context("--requests 需要 on 或 off")?;
                        requests = Some(parse_on_off(val)?);
                        idx += 2;
                    }
                    "--responses" => {
                        let val = args.get(idx + 1).context("--responses 需要 on 或 off")?;
                        responses = Some(parse_on_off(val)?);
                        idx += 2;
                    }
                    flag => bail!("proxy intercept 不支持参数 `{flag}`"),
                }
            }
            Ok(CliCommand::ProxyIntercept {
                requests,
                responses,
            })
        }
        "pending" => {
            ensure!(args.len() == 1, "proxy pending 不接受额外参数");
            Ok(CliCommand::ProxyPending)
        }
        "decide" => parse_proxy_decide_subcommand(&args[1..]),
        other => bail!("未知 proxy 子命令 `{other}`；支持 start, stop, intercept, pending, decide"),
    }
}

fn parse_proxy_decide_subcommand(args: &[String]) -> Result<CliCommand> {
    let mut flow_id = None;
    let mut revision = None;
    let mut forward = false;
    let mut drop_flow = false;
    let mut add_headers = Vec::new();
    let mut remove_headers = Vec::new();
    let mut override_body = None;
    let mut override_status = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--revision" => {
                let raw = args.get(idx + 1).context("--revision 需要整数")?;
                revision = Some(raw.parse::<u64>().context("--revision 必须是非负整数")?);
                idx += 2;
            }
            "--forward" => {
                forward = true;
                idx += 1;
            }
            "--drop" => {
                drop_flow = true;
                idx += 1;
            }
            "--add-header" | "--set-header" => {
                let raw = args.get(idx + 1).context("--set-header 需要 K:V 字符串")?;
                let (k, v) = raw
                    .split_once(':')
                    .context("Header 参数必须包含冒号，格式 K:V")?;
                add_headers.push((k.trim().to_owned(), v.trim().to_owned()));
                idx += 2;
            }
            "--remove-header" => {
                let raw = args
                    .get(idx + 1)
                    .context("--remove-header 需要请求头名称")?;
                ensure!(!raw.trim().is_empty(), "--remove-header 名称不能为空");
                remove_headers.push(raw.trim().to_owned());
                idx += 2;
            }
            "--body" => {
                override_body = Some(args.get(idx + 1).context("--body 需要字符串")?.clone());
                idx += 2;
            }
            "--status" => {
                let raw = args.get(idx + 1).context("--status 需要状态码")?;
                override_status = Some(raw.parse::<u16>().context("--status 必须为 16 位整数")?);
                idx += 2;
            }
            val if !val.starts_with('-') && flow_id.is_none() => {
                flow_id = Some(Uuid::parse_str(val).context("无效的 flow-id UUID")?);
                idx += 1;
            }
            other => bail!("proxy decide 不支持参数 `{other}`"),
        }
    }
    ensure!(
        forward ^ drop_flow,
        "必须且只能指定 --forward 或 --drop 之一"
    );
    Ok(CliCommand::ProxyDecide {
        flow_id: flow_id.context(
            "用法：mio-cli proxy decide <flow-id> --revision <rev> (--forward | --drop)",
        )?,
        revision: revision.context("缺少 --revision <rev>")?,
        drop_flow,
        add_headers,
        remove_headers,
        override_body,
        override_status,
    })
}

fn parse_workspace_subcommand(args: &[String]) -> Result<CliCommand> {
    let sub = args
        .first()
        .map(String::as_str)
        .context("workspace 需要子命令：get 或 set")?;
    match sub {
        "get" => {
            ensure!(args.len() == 1, "workspace get 不接受额外参数");
            Ok(CliCommand::WorkspaceGet)
        }
        "set" => {
            ensure!(args.len() == 2, "用法：mio-cli workspace set <json-file>");
            Ok(CliCommand::WorkspaceSet {
                json_path: PathBuf::from(&args[1]),
            })
        }
        other => bail!("未知 workspace 子命令 `{other}`；支持 get, set"),
    }
}

fn parse_on_off(raw: &str) -> Result<bool> {
    match raw.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        other => bail!("开关参数必须为 on 或 off，收到 `{other}`"),
    }
}

fn parse_serve_subcommand(args: &[String]) -> Result<CliCommand> {
    let mut listen = None;
    let mut export_ca = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--listen" => {
                listen = Some(args.get(idx + 1).context("--listen 需要地址参数")?.clone());
                idx += 2;
            }
            "--export-ca" => {
                export_ca = Some(
                    args.get(idx + 1)
                        .context("--export-ca 需要路径参数")?
                        .clone(),
                );
                idx += 2;
            }
            flag => bail!("serve 不支持参数 `{flag}`"),
        }
    }
    Ok(CliCommand::Serve { listen, export_ca })
}

fn parse_flows_subcommand(args: &[String]) -> Result<CliCommand> {
    let sub = args
        .first()
        .map(String::as_str)
        .context("flows 需要子命令：list 或 get")?;
    match sub {
        "list" => {
            let mut filter = String::new();
            let mut method = None;
            let mut status = None;
            let mut offset = 0usize;
            let mut limit = 50usize;
            let mut idx = 1;
            while idx < args.len() {
                match args[idx].as_str() {
                    "--filter" => {
                        filter = args.get(idx + 1).context("--filter 需要字符串")?.clone();
                        idx += 2;
                    }
                    "--method" => {
                        method = Some(args.get(idx + 1).context("--method 需要方法名")?.clone());
                        idx += 2;
                    }
                    "--status" => {
                        let raw = args.get(idx + 1).context("--status 需要状态码")?;
                        status = Some(raw.parse::<u16>().context("--status 必须为 16 位整数")?);
                        idx += 2;
                    }
                    "--offset" => {
                        let raw = args.get(idx + 1).context("--offset 需要非负整数")?;
                        offset = raw.parse::<usize>().context("--offset 必须为非负整数")?;
                        idx += 2;
                    }
                    "--limit" => {
                        let raw = args.get(idx + 1).context("--limit 需要正整数")?;
                        limit = raw.parse::<usize>().context("--limit 必须为正整数")?;
                        ensure!((1..=500).contains(&limit), "--limit 必须在 1..=500 之间");
                        idx += 2;
                    }
                    flag => bail!("flows list 不支持参数 `{flag}`"),
                }
            }
            Ok(CliCommand::FlowsList {
                filter,
                method,
                status,
                offset,
                limit,
            })
        }
        "get" => {
            let mut flow_id = None;
            let mut original = false;
            for arg in &args[1..] {
                if arg == "--original" {
                    original = true;
                } else if !arg.starts_with('-') && flow_id.is_none() {
                    flow_id = Some(Uuid::parse_str(arg).context("无效的 flow-id UUID")?);
                } else {
                    bail!("flows get 不支持参数 `{arg}`");
                }
            }
            Ok(CliCommand::FlowsGet {
                flow_id: flow_id.context("用法：mio-cli flows get <flow-id> [--original]")?,
                original,
            })
        }
        other => bail!("未知 flows 子命令 `{other}`；支持 list 或 get"),
    }
}

fn parse_repeater_subcommand(args: &[String]) -> Result<CliCommand> {
    let sub = args
        .first()
        .map(String::as_str)
        .context("repeater 需要子命令：open 或 send")?;
    ensure!(
        sub == "open" || sub == "send",
        "未知 repeater 子命令 `{sub}`；支持 open 或 send"
    );
    let mut flow_id = None;
    let mut use_original = false;
    let mut target = None;
    let mut raw_request = None;
    let mut title = None;
    let mut add_headers = Vec::new();
    let mut remove_headers = Vec::new();
    let mut override_body = None;
    let mut insecure = false;
    let mut idx = 1;
    while idx < args.len() {
        match args[idx].as_str() {
            "--flow" => {
                let val = args.get(idx + 1).context("--flow 需要 UUID")?;
                flow_id = Some(Uuid::parse_str(val).context("无效的 --flow UUID")?);
                idx += 2;
            }
            "--original" => {
                use_original = true;
                idx += 1;
            }
            "--target" => {
                target = Some(args.get(idx + 1).context("--target 需要 origin")?.clone());
                idx += 2;
            }
            "--raw" => {
                raw_request = Some(args.get(idx + 1).context("--raw 需要请求原文")?.clone());
                idx += 2;
            }
            "--title" if sub == "open" => {
                title = Some(args.get(idx + 1).context("--title 需要标题字符串")?.clone());
                idx += 2;
            }
            "--add-header" | "--set-header" if sub == "send" => {
                let raw = args.get(idx + 1).context("--set-header 需要 K:V 字符串")?;
                let (k, v) = raw
                    .split_once(':')
                    .context("Header 参数必须包含冒号，格式 K:V")?;
                add_headers.push((k.trim().to_owned(), v.trim().to_owned()));
                idx += 2;
            }
            "--remove-header" if sub == "send" => {
                let raw = args
                    .get(idx + 1)
                    .context("--remove-header 需要请求头名称")?;
                ensure!(!raw.trim().is_empty(), "--remove-header 名称不能为空");
                remove_headers.push(raw.trim().to_owned());
                idx += 2;
            }
            "--body" if sub == "send" => {
                override_body = Some(args.get(idx + 1).context("--body 需要字符串")?.clone());
                idx += 2;
            }
            "--insecure" if sub == "send" => {
                insecure = true;
                idx += 1;
            }
            flag => bail!("repeater {sub} 不支持参数 `{flag}`"),
        }
    }
    match (&flow_id, &target, &raw_request) {
        (Some(_), None, None) => {}
        (None, Some(_), Some(_)) => {
            ensure!(!use_original, "--original 只能配合 --flow 使用");
        }
        _ => {
            bail!(
                "用法：mio-cli repeater {sub} (--flow <flow-id> [--original] | --target <origin> --raw <raw-request>)"
            );
        }
    }
    if sub == "open" {
        Ok(CliCommand::RepeaterOpen {
            flow_id,
            use_original,
            target,
            raw_request,
            title,
        })
    } else {
        Ok(CliCommand::RepeaterSend {
            flow_id,
            use_original,
            target,
            raw_request,
            add_headers,
            remove_headers,
            override_body,
            insecure,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|v| (*v).to_owned()).collect()
    }

    #[test]
    fn parses_all_cli_subcommands_and_global_flags() {
        let inv = parse_cli(&s(&["--socket", "/tmp/x.sock", "status", "--json"])).unwrap();
        assert_eq!(inv.socket, Some(PathBuf::from("/tmp/x.sock")));
        assert!(inv.json);
        assert_eq!(inv.command, CliCommand::Status);

        let inv = parse_cli(&s(&[
            "flows", "list", "--filter", "login", "--method", "POST", "--status", "200",
            "--offset", "5", "--limit", "20",
        ]))
        .unwrap();
        assert_eq!(
            inv.command,
            CliCommand::FlowsList {
                filter: "login".into(),
                method: Some("POST".into()),
                status: Some(200),
                offset: 5,
                limit: 20,
            }
        );

        let fid = Uuid::new_v4();
        let inv = parse_cli(&s(&["flows", "get", &fid.to_string(), "--original"])).unwrap();
        assert_eq!(
            inv.command,
            CliCommand::FlowsGet {
                flow_id: fid,
                original: true
            }
        );

        let inv = parse_cli(&s(&[
            "repeater",
            "open",
            "--target",
            "http://127.0.0.1:8080",
            "--raw",
            "GET /ping HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n",
            "--title",
            "AI Probe",
        ]))
        .unwrap();
        assert!(
            matches!(inv.command, CliCommand::RepeaterOpen { title: Some(t), .. } if t == "AI Probe")
        );

        let inv = parse_cli(&s(&["slim-save", "/tmp/slimmed.mio", "--json"])).unwrap();
        assert!(inv.json);
        assert_eq!(
            inv.command,
            CliCommand::SlimSave {
                dest_path: "/tmp/slimmed.mio".into()
            }
        );

        // Option value named "--json" must not be swallowed as global --json flag.
        let inv = parse_cli(&s(&["flows", "list", "--filter", "--json"])).unwrap();
        assert!(!inv.json);
        assert!(matches!(
            inv.command,
            CliCommand::FlowsList { ref filter, .. } if filter == "--json"
        ));

        // Subcommand --help exits cleanly as Help.
        let inv = parse_cli(&s(&["repeater", "send", "--help"])).unwrap();
        assert_eq!(inv.command, CliCommand::Help);

        // Multiple --set-header flags on proxy decide are all retained.
        let inv = parse_cli(&s(&[
            "proxy",
            "decide",
            &fid.to_string(),
            "--revision",
            "1",
            "--forward",
            "--set-header",
            "X-One: 1",
            "--set-header",
            "X-Two: 2",
        ]))
        .unwrap();
        assert_eq!(
            inv.command,
            CliCommand::ProxyDecide {
                flow_id: fid,
                revision: 1,
                drop_flow: false,
                add_headers: vec![("X-One".into(), "1".into()), ("X-Two".into(), "2".into())],
                remove_headers: Vec::new(),
                override_body: None,
                override_status: None,
            }
        );
    }

    #[test]
    fn cli_executes_against_live_runtime_ipc_socket() {
        #[cfg(unix)]
        let dir = PathBuf::from(format!(
            "/tmp/mio-cli-{}",
            &Uuid::new_v4().simple().to_string()[..12]
        ));
        #[cfg(not(unix))]
        let dir = std::env::temp_dir().join(format!(
            "mio-cli-{}",
            &Uuid::new_v4().simple().to_string()[..12]
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("cli.sock");
        let slim = dir.join("slim.mio");
        let (runtime, _client) = mio_core::Runtime::start_with_ipc_socket(sock.clone()).unwrap();
        let sock_str = sock.to_string_lossy().into_owned();

        execute_cli(&s(&["--socket", &sock_str, "status", "--json"])).unwrap();
        execute_cli(&s(&[
            "--socket", &sock_str, "flows", "list", "--limit", "5",
        ]))
        .unwrap();
        execute_cli(&s(&[
            "--socket",
            &sock_str,
            "repeater",
            "open",
            "--target",
            "http://127.0.0.1:9090",
            "--raw",
            "GET /check HTTP/1.1\r\nHost: 127.0.0.1:9090\r\n\r\n",
            "--title",
            "CLI Tab",
            "--json",
        ]))
        .unwrap();
        execute_cli(&s(&[
            "--socket",
            &sock_str,
            "slim-save",
            &slim.to_string_lossy(),
            "--json",
        ]))
        .unwrap();
        assert!(slim.exists());

        runtime.shutdown().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
