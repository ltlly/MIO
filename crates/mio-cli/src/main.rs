use std::{net::SocketAddr, path::Path, path::PathBuf, process::ExitCode, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, error::ErrorKind};
use mio_core::{
    AiIpcEnvelope, AiIpcRequest, Command, Event, Uuid, call_ai_ipc, default_ipc_socket_path,
};

mod render;

const CLI_TIMEOUT: Duration = Duration::from_secs(30);
const CLI_REPEATER_SEND_TIMEOUT: Duration = Duration::from_secs(305);

#[derive(Parser, Debug, PartialEq, Eq)]
#[command(
    name = "mio-cli",
    about = "MIO 本地 IPC 命令行与无头代理工具（支持环境变量 MIO_IPC_SOCKET 指定默认 socket 路径）"
)]
struct Cli {
    /// 指定 MIO IPC socket / endpoint 路径
    #[arg(long, global = true, value_parser = parse_non_empty_path)]
    socket: Option<PathBuf>,
    /// 以单行 JSON 输出结果
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum CliCommand {
    /// 查询当前 MIO 运行状态
    Status,
    /// 查询或读取 HTTP 流量
    Flows {
        #[command(subcommand)]
        command: FlowsCommand,
    },
    /// 查询 WebSocket 消息帧
    Ws {
        #[command(subcommand)]
        command: WsCommand,
    },
    /// 在 Repeater 中打开或发送请求
    Repeater {
        #[command(subcommand)]
        command: RepeaterCommand,
    },
    /// 瘦身另存当前项目
    SlimSave {
        /// 目标项目文件路径
        dest_path: String,
    },
    /// Proxy CA 证书相关操作
    Ca {
        #[command(subcommand)]
        command: CaCommand,
    },
    /// 代理监听与拦截控制
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    /// 读取或更新工作区配置
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// 优雅关闭 MIO 后台服务
    Shutdown,
    /// 启动无头服务端（不依赖 GUI）
    Serve {
        /// 代理监听地址，例如 127.0.0.1:8080
        #[arg(long)]
        listen: Option<String>,
        /// 启动后导出 CA 证书到指定路径
        #[arg(long = "export-ca")]
        export_ca: Option<String>,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum FlowsCommand {
    /// 列出 HTTP 流量摘要
    List {
        /// URL / 内容过滤关键字
        #[arg(long, default_value = "", allow_hyphen_values = true)]
        filter: String,
        /// 按 HTTP 方法过滤
        #[arg(long)]
        method: Option<String>,
        /// 按响应状态码过滤
        #[arg(long)]
        status: Option<u16>,
        /// 分页起始偏移量
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// 返回条数上限（1..=500）
        #[arg(long, default_value_t = 50, value_parser = parse_limit)]
        limit: usize,
    },
    /// 获取单条流量详情
    Get {
        /// 流量 UUID
        flow_id: Uuid,
        /// 显示拦截改写前的原始报文
        #[arg(long)]
        original: bool,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum WsCommand {
    /// 列出已记录的 WebSocket 帧
    List {
        /// 返回条数上限（1..=500）
        #[arg(long, default_value_t = 50, value_parser = parse_limit)]
        limit: usize,
    },
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
#[command(group(
    ArgGroup::new("source")
        .required(true)
        .args(["flow_id", "target"])
))]
struct RepeaterSourceArgs {
    /// 来源流量 UUID
    #[arg(long = "flow", conflicts_with_all = ["target", "raw_request"])]
    flow_id: Option<Uuid>,
    /// 使用拦截改写前的原始请求（仅配合 --flow）
    #[arg(long = "original", requires = "flow_id")]
    use_original: bool,
    /// 目标 origin，例如 http://127.0.0.1:8080（需配合 --raw）
    #[arg(long, requires = "raw_request")]
    target: Option<String>,
    /// 原始 HTTP 请求文本（需配合 --target）
    #[arg(long = "raw", requires = "target", allow_hyphen_values = true)]
    raw_request: Option<String>,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum RepeaterCommand {
    /// 在 Repeater 打开请求标签
    Open {
        #[command(flatten)]
        source: RepeaterSourceArgs,
        /// 自定义标签标题
        #[arg(long, allow_hyphen_values = true)]
        title: Option<String>,
    },
    /// 通过 Repeater 发送单次请求
    Send {
        #[command(flatten)]
        source: RepeaterSourceArgs,
        /// 新增或覆盖请求头（格式 K:V，可重复）
        #[arg(long = "add-header", value_name = "K:V", value_parser = parse_header_kv)]
        add_headers: Vec<HeaderKv>,
        /// 移除指定请求头（可重复）
        #[arg(long = "remove-header", value_name = "NAME", value_parser = parse_non_empty_trimmed)]
        remove_headers: Vec<String>,
        /// 覆盖请求体
        #[arg(long = "body", allow_hyphen_values = true)]
        override_body: Option<String>,
        /// 忽略上游 TLS 证书校验
        #[arg(long)]
        insecure: bool,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum CaCommand {
    /// 导出 Proxy CA 证书
    Export {
        /// 导出目标路径
        dest_path: String,
        /// 强制导出 PEM 格式
        #[arg(long, conflicts_with = "der")]
        pem: bool,
        /// 强制导出 DER 格式
        #[arg(long, conflicts_with = "pem")]
        der: bool,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum ProxyCommand {
    /// 启动代理监听
    Start {
        /// 监听地址，例如 127.0.0.1:8080
        #[arg(long = "listen")]
        bind_address: Option<String>,
    },
    /// 停止代理监听
    Stop,
    /// 查看或设置请求/响应拦截开关
    Intercept {
        /// 请求拦截开关（on|off）
        #[arg(long, value_name = "on|off", value_parser = parse_on_off)]
        requests: Option<bool>,
        /// 响应拦截开关（on|off）
        #[arg(long, value_name = "on|off", value_parser = parse_on_off)]
        responses: Option<bool>,
    },
    /// 列出当前挂起等待决定的拦截流量
    Pending,
    /// 对挂起拦截流量执行放行或丢弃
    Decide(ProxyDecideArgs),
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
#[command(group(
    ArgGroup::new("action")
        .required(true)
        .args(["forward", "drop_flow"])
))]
struct ProxyDecideArgs {
    /// 被拦截流量的 UUID
    flow_id: Uuid,
    /// 拦截修订号
    #[arg(long)]
    revision: u64,
    /// 放行请求或响应
    #[arg(long, conflicts_with = "drop_flow")]
    forward: bool,
    /// 丢弃该流量
    #[arg(long = "drop", conflicts_with = "forward")]
    drop_flow: bool,
    /// 新增或覆盖请求/响应头（格式 K:V，可重复）
    #[arg(long = "add-header", value_name = "K:V", value_parser = parse_header_kv)]
    add_headers: Vec<HeaderKv>,
    /// 移除指定请求/响应头（可重复）
    #[arg(long = "remove-header", value_name = "NAME", value_parser = parse_non_empty_trimmed)]
    remove_headers: Vec<String>,
    /// 覆盖报文体
    #[arg(long = "body", allow_hyphen_values = true)]
    override_body: Option<String>,
    /// 覆盖响应状态码
    #[arg(long = "status")]
    override_status: Option<u16>,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum WorkspaceCommand {
    /// 读取当前工作区 JSON
    Get,
    /// 从 JSON 文件更新工作区配置
    Set {
        /// 工作区 JSON 文件路径
        json_path: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HeaderKv {
    name: String,
    value: String,
}

impl HeaderKv {
    fn into_pair(self) -> (String, String) {
        (self.name, self.value)
    }
}

fn parse_non_empty_path(raw: &str) -> std::result::Result<PathBuf, String> {
    if raw.is_empty() {
        return Err("--socket 路径不能为空".into());
    }
    Ok(PathBuf::from(raw))
}

fn parse_non_empty_trimmed(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("名称不能为空".into());
    }
    Ok(trimmed.to_owned())
}

fn parse_header_kv(raw: &str) -> std::result::Result<HeaderKv, String> {
    let (k, v) = raw
        .split_once(':')
        .ok_or_else(|| "Header 参数必须包含冒号，格式 K:V".to_owned())?;
    let name = k.trim();
    if name.is_empty() {
        return Err("Header 名称不能为空".into());
    }
    Ok(HeaderKv {
        name: name.to_owned(),
        value: v.trim().to_owned(),
    })
}

fn parse_limit(raw: &str) -> std::result::Result<usize, String> {
    let limit = raw
        .parse::<usize>()
        .map_err(|_| "--limit 必须为正整数".to_owned())?;
    if !(1..=500).contains(&limit) {
        return Err("--limit 必须在 1..=500 之间".into());
    }
    Ok(limit)
}

fn parse_on_off(raw: &str) -> std::result::Result<bool, String> {
    match raw.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        other => Err(format!("开关参数必须为 on 或 off，收到 `{other}`")),
    }
}

fn parse_cli(args: &[String]) -> std::result::Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("mio-cli".to_owned()).chain(args.iter().cloned()))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        let _ = Cli::command().print_help();
        println!();
        return ExitCode::SUCCESS;
    }
    let cli = match parse_cli(&args) {
        Ok(cli) => cli,
        Err(err) => {
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                let _ = err.print();
                return ExitCode::SUCCESS;
            }
            if args.iter().any(|arg| arg == "--json") {
                println!("{}", AiIpcEnvelope::err_json(err.to_string().trim().to_owned()));
            } else {
                let _ = err.print();
            }
            return ExitCode::FAILURE;
        }
    };
    let wants_json = cli.json;
    match execute_cli(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let msg = format!("{error:#}");
            if wants_json {
                println!("{}", AiIpcEnvelope::err_json(msg));
            } else {
                eprintln!("mio-cli 错误：{msg}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn execute_cli(cli: Cli) -> Result<()> {
    let socket_path = cli.socket.unwrap_or_else(default_ipc_socket_path);
    let json = cli.json;
    let (req, show_original) = match cli.command {
        CliCommand::Serve { listen, export_ca } => {
            return run_headless_server(socket_path, listen, export_ca, json).await;
        }
        CliCommand::Status => (AiIpcRequest::Status, false),
        CliCommand::Flows { command } => match command {
            FlowsCommand::List {
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
            FlowsCommand::Get { flow_id, original } => {
                (AiIpcRequest::FlowsGet { flow_id }, original)
            }
        },
        CliCommand::Ws { command } => match command {
            WsCommand::List { limit } => (AiIpcRequest::WebSocketList { limit }, false),
        },
        CliCommand::Repeater { command } => match command {
            RepeaterCommand::Open { source, title } => (
                AiIpcRequest::RepeaterOpen {
                    flow_id: source.flow_id,
                    use_original: source.use_original,
                    target: source.target,
                    raw_request: source.raw_request,
                    title,
                },
                false,
            ),
            RepeaterCommand::Send {
                source,
                add_headers,
                remove_headers,
                override_body,
                insecure,
            } => (
                AiIpcRequest::RepeaterSend {
                    flow_id: source.flow_id,
                    use_original: source.use_original,
                    target: source.target,
                    raw_request: source.raw_request,
                    add_headers: add_headers.into_iter().map(HeaderKv::into_pair).collect(),
                    remove_headers,
                    override_body,
                    insecure,
                },
                false,
            ),
        },
        CliCommand::SlimSave { dest_path } => (
            AiIpcRequest::SlimSave {
                path: dest_path,
                options: None,
            },
            false,
        ),
        CliCommand::Ca { command } => match command {
            CaCommand::Export {
                dest_path,
                pem,
                der,
            } => (
                AiIpcRequest::ExportCa {
                    path: dest_path,
                    pem: if pem {
                        Some(true)
                    } else if der {
                        Some(false)
                    } else {
                        None
                    },
                },
                false,
            ),
        },
        CliCommand::Proxy { command } => match command {
            ProxyCommand::Start { bind_address } => {
                (AiIpcRequest::ProxyStart { bind_address }, false)
            }
            ProxyCommand::Stop => (AiIpcRequest::ProxyStop, false),
            ProxyCommand::Intercept {
                requests,
                responses,
            } => (
                AiIpcRequest::ProxyIntercept {
                    requests,
                    responses,
                },
                false,
            ),
            ProxyCommand::Pending => (AiIpcRequest::ProxyPending, false),
            ProxyCommand::Decide(args) => (
                AiIpcRequest::ProxyDecide {
                    flow_id: args.flow_id,
                    revision: args.revision,
                    drop_flow: args.drop_flow,
                    add_headers: args
                        .add_headers
                        .into_iter()
                        .map(HeaderKv::into_pair)
                        .collect(),
                    remove_headers: args.remove_headers,
                    override_body: args.override_body,
                    override_status: args.override_status,
                },
                false,
            ),
        },
        CliCommand::Workspace { command } => match command {
            WorkspaceCommand::Get => (AiIpcRequest::WorkspaceGet, false),
            WorkspaceCommand::Set { json_path } => {
                let workspace_json = std::fs::read_to_string(&json_path)
                    .with_context(|| format!("读取工作区 JSON 失败：{}", json_path.display()))?;
                (AiIpcRequest::WorkspacePatch { workspace_json }, false)
            }
        },
        CliCommand::Shutdown => (AiIpcRequest::Shutdown, false),
    };
    let timeout = if matches!(req, AiIpcRequest::RepeaterSend { .. }) {
        CLI_REPEATER_SEND_TIMEOUT
    } else {
        CLI_TIMEOUT
    };
    let (env, raw_line) =
        tokio::task::spawn_blocking(move || call_ai_ipc(&socket_path, &req, timeout))
            .await
            .context("执行 IPC 请求任务失败")??;
    if !env.ok {
        bail!("{}", env.error.unwrap_or_else(|| "IPC 请求失败".into()));
    }
    if json {
        println!("{}", raw_line.trim());
        return Ok(());
    }
    let payload = env.payload.context("IPC 成功响应缺少数据载荷")?;
    render::render_human(&payload, show_original);
    Ok(())
}

fn format_headless_ready_json(socket_path: &Path, active_proxy: Option<&str>) -> String {
    serde_json::json!({
        "ready": true,
        "socket": socket_path.to_string_lossy(),
        "proxy_address": active_proxy,
    })
    .to_string()
}

async fn await_headless_event<T>(
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    expected_op: &'static str,
    mut matcher: impl FnMut(Event) -> Option<T>,
) -> Result<T> {
    let deadline = tokio::time::Instant::now() + CLI_TIMEOUT;
    loop {
        let maybe_event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .with_context(|| format!("等待后台服务事件超时（{expected_op}）"))?;
        let event = maybe_event
            .with_context(|| format!("后台服务事件通道已关闭（{expected_op}）"))?;
        match event {
            Event::Error {
                operation, message, ..
            } if operation == expected_op => {
                bail!("{message}");
            }
            Event::Stopped => {
                bail!("后台服务在完成 {expected_op} 前已停止");
            }
            other => {
                if let Some(found) = matcher(other) {
                    return Ok(found);
                }
            }
        }
    }
}

async fn wait_ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

async fn run_headless_server(
    socket_path: PathBuf,
    listen: Option<String>,
    export_ca: Option<String>,
    json: bool,
) -> Result<()> {
    let ca_dest = match export_ca {
        Some(raw) => {
            let trimmed = raw.trim();
            ensure!(!trimmed.is_empty(), "导出 CA 目标路径不能为空");
            Some(PathBuf::from(trimmed))
        }
        None => None,
    };
    let listen_addr: Option<SocketAddr> = match listen.as_deref().map(str::trim) {
        Some(addr) if !addr.is_empty() => Some(
            addr.parse()
                .context("无效的监听 SocketAddr（例如 127.0.0.1:8080）")?,
        ),
        _ => None,
    };

    let socket_for_start = socket_path.clone();
    let (runtime, client) = tokio::task::spawn_blocking(move || {
        mio_core::Runtime::start_with_ipc_socket(socket_for_start)
    })
    .await
    .context("启动后台服务任务失败")??;
    let commands = client.commands;
    let mut events = client.events;
    let shutdown = client.shutdown;

    let startup = async {
        let project = await_headless_event(&mut events, "start_project", |ev| match ev {
            Event::ProjectOpened { project, .. } => Some(project),
            _ => None,
        })
        .await?;

        if let Some(ca_path) = ca_dest {
            commands
                .send(Command::ExportCa {
                    path: ca_path.clone(),
                })
                .await
                .context("发送导出 CA 命令失败")?;
            await_headless_event(&mut events, "export_ca", |ev| match ev {
                Event::CaExported { path } if path == ca_path => Some(()),
                _ => None,
            })
            .await?;
        }

        let mut active_proxy: Option<String> = None;
        if let Some(addr) = listen_addr {
            commands
                .send(Command::StartProxy {
                    project,
                    listen: addr,
                })
                .await
                .context("发送启动代理命令失败")?;
            let bound = await_headless_event(&mut events, "start_proxy", |ev| match ev {
                Event::ProxyState {
                    project: ev_project,
                    address: Some(bound),
                    ..
                } if ev_project == project => Some(bound),
                _ => None,
            })
            .await?;
            active_proxy = Some(bound.to_string());
        }
        Ok::<_, anyhow::Error>(active_proxy)
    }
    .await;

    let active_proxy = match startup {
        Ok(proxy) => proxy,
        Err(err) => {
            drop(events);
            drop(commands);
            let _ = tokio::task::spawn_blocking(move || runtime.shutdown()).await;
            return Err(err);
        }
    };

    if json {
        println!(
            "{}",
            format_headless_ready_json(&socket_path, active_proxy.as_deref())
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

    let mut drain_task = tokio::spawn(async move {
        while events.recv().await.is_some() {}
    });

    let (interrupted, drain_done) = tokio::select! {
        _ = wait_ctrl_c() => (true, false),
        _ = shutdown.cancelled() => (false, false),
        _ = &mut drain_task => (false, true),
    };

    let join_res = tokio::task::spawn_blocking(move || {
        if interrupted {
            runtime.shutdown()
        } else {
            runtime.join()
        }
    })
    .await
    .context("等待后台服务退出失败")?;
    drop(commands);
    if !drain_done {
        let _ = drain_task.await;
    }
    join_res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|v| (*v).to_owned()).collect()
    }

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(prefix: &str) -> Self {
            let suffix = &Uuid::new_v4().simple().to_string()[..12];
            #[cfg(unix)]
            let path = PathBuf::from(format!("/tmp/{prefix}-{suffix}"));
            #[cfg(not(unix))]
            let path = std::env::temp_dir().join(format!("{prefix}-{suffix}"));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: callers hold `ENV_LOCK` for the full guard lifetime.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.prev {
                // SAFETY: callers hold `ENV_LOCK` for the full guard lifetime.
                Some(old) => unsafe { std::env::set_var(self.key, old) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parses_all_cli_subcommands_and_global_flags() {
        let cli = parse_cli(&s(&["--socket", "/tmp/x.sock", "status", "--json"])).unwrap();
        assert_eq!(cli.socket, Some(PathBuf::from("/tmp/x.sock")));
        assert!(cli.json);
        assert_eq!(cli.command, CliCommand::Status);

        let cli = parse_cli(&s(&[
            "flows", "list", "--filter", "login", "--method", "POST", "--status", "200",
            "--offset", "5", "--limit", "20",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Flows {
                command: FlowsCommand::List {
                    filter: "login".into(),
                    method: Some("POST".into()),
                    status: Some(200),
                    offset: 5,
                    limit: 20,
                }
            }
        );

        let fid = Uuid::new_v4();
        let cli = parse_cli(&s(&["flows", "get", &fid.to_string(), "--original"])).unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Flows {
                command: FlowsCommand::Get {
                    flow_id: fid,
                    original: true,
                }
            }
        );

        let cli = parse_cli(&s(&["ws", "list", "--limit", "10"])).unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Ws {
                command: WsCommand::List { limit: 10 }
            }
        );

        let cli = parse_cli(&s(&[
            "repeater",
            "open",
            "--target",
            "http://127.0.0.1:8080",
            "--raw",
            "GET /ping HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n",
            "--title",
            "Probe",
        ]))
        .unwrap();
        assert!(matches!(
            cli.command,
            CliCommand::Repeater {
                command: RepeaterCommand::Open { title: Some(ref t), .. }
            } if t == "Probe"
        ));

        let cli = parse_cli(&s(&[
            "repeater",
            "send",
            "--flow",
            &fid.to_string(),
            "--original",
            "--add-header",
            "X-Test: val",
            "--remove-header",
            "Cookie",
            "--body",
            "{\"ok\":true}",
            "--insecure",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Repeater {
                command: RepeaterCommand::Send {
                    source: RepeaterSourceArgs {
                        flow_id: Some(fid),
                        use_original: true,
                        target: None,
                        raw_request: None,
                    },
                    add_headers: vec![HeaderKv {
                        name: "X-Test".into(),
                        value: "val".into(),
                    }],
                    remove_headers: vec!["Cookie".into()],
                    override_body: Some("{\"ok\":true}".into()),
                    insecure: true,
                }
            }
        );

        let cli = parse_cli(&s(&["slim-save", "/tmp/slimmed.mio", "--json"])).unwrap();
        assert!(cli.json);
        assert_eq!(
            cli.command,
            CliCommand::SlimSave {
                dest_path: "/tmp/slimmed.mio".into()
            }
        );

        let cli = parse_cli(&s(&["ca", "export", "/tmp/ca.pem", "--pem"])).unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Ca {
                command: CaCommand::Export {
                    dest_path: "/tmp/ca.pem".into(),
                    pem: true,
                    der: false,
                }
            }
        );

        // Option value named "--json" must not be swallowed as global --json flag.
        let cli = parse_cli(&s(&["flows", "list", "--filter", "--json"])).unwrap();
        assert!(!cli.json);
        assert!(matches!(
            cli.command,
            CliCommand::Flows {
                command: FlowsCommand::List { ref filter, .. }
            } if filter == "--json"
        ));

        // Multiple --add-header flags on proxy decide are all retained.
        let cli = parse_cli(&s(&[
            "proxy",
            "decide",
            &fid.to_string(),
            "--revision",
            "1",
            "--forward",
            "--add-header",
            "X-One: 1",
            "--add-header",
            "X-Two: 2",
            "--remove-header",
            "X-Drop",
            "--status",
            "201",
            "--body",
            "updated",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Proxy {
                command: ProxyCommand::Decide(ProxyDecideArgs {
                    flow_id: fid,
                    revision: 1,
                    forward: true,
                    drop_flow: false,
                    add_headers: vec![
                        HeaderKv {
                            name: "X-One".into(),
                            value: "1".into(),
                        },
                        HeaderKv {
                            name: "X-Two".into(),
                            value: "2".into(),
                        },
                    ],
                    remove_headers: vec!["X-Drop".into()],
                    override_body: Some("updated".into()),
                    override_status: Some(201),
                })
            }
        );

        let cli = parse_cli(&s(&[
            "proxy",
            "intercept",
            "--requests",
            "on",
            "--responses",
            "off",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Proxy {
                command: ProxyCommand::Intercept {
                    requests: Some(true),
                    responses: Some(false),
                }
            }
        );

        let cli = parse_cli(&s(&["proxy", "start", "--listen", "127.0.0.1:8081"])).unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Proxy {
                command: ProxyCommand::Start {
                    bind_address: Some("127.0.0.1:8081".into()),
                }
            }
        );
        assert_eq!(
            parse_cli(&s(&["proxy", "stop"])).unwrap().command,
            CliCommand::Proxy {
                command: ProxyCommand::Stop
            }
        );
        assert_eq!(
            parse_cli(&s(&["proxy", "pending"])).unwrap().command,
            CliCommand::Proxy {
                command: ProxyCommand::Pending
            }
        );

        assert_eq!(
            parse_cli(&s(&["workspace", "get"])).unwrap().command,
            CliCommand::Workspace {
                command: WorkspaceCommand::Get
            }
        );
        assert_eq!(
            parse_cli(&s(&["workspace", "set", "/tmp/ws.json"]))
                .unwrap()
                .command,
            CliCommand::Workspace {
                command: WorkspaceCommand::Set {
                    json_path: PathBuf::from("/tmp/ws.json")
                }
            }
        );
        assert_eq!(
            parse_cli(&s(&["shutdown"])).unwrap().command,
            CliCommand::Shutdown
        );
        assert_eq!(
            parse_cli(&s(&[
                "serve",
                "--listen",
                "127.0.0.1:8080",
                "--export-ca",
                "/tmp/ca.der"
            ]))
            .unwrap()
            .command,
            CliCommand::Serve {
                listen: Some("127.0.0.1:8080".into()),
                export_ca: Some("/tmp/ca.der".into()),
            }
        );
    }

    #[test]
    fn rejects_invalid_cli_arguments_and_removed_aliases() {
        let fid = Uuid::new_v4().to_string();

        // --set-header alias was intentionally removed in favor of --add-header.
        assert!(
            parse_cli(&s(&[
                "proxy",
                "decide",
                &fid,
                "--revision",
                "1",
                "--forward",
                "--set-header",
                "X-One: 1",
            ]))
            .is_err()
        );
        assert!(
            parse_cli(&s(&[
                "repeater",
                "send",
                "--flow",
                &fid,
                "--set-header",
                "X-One: 1",
            ]))
            .is_err()
        );

        // Help exits as DisplayHelp.
        let help_err = parse_cli(&s(&["repeater", "send", "--help"])).unwrap_err();
        assert_eq!(help_err.kind(), ErrorKind::DisplayHelp);

        assert!(parse_cli(&s(&["--socket", "", "status"])).is_err());
        assert!(parse_cli(&s(&["flows", "list", "--limit", "0"])).is_err());
        assert!(parse_cli(&s(&["flows", "list", "--limit", "501"])).is_err());
        assert!(parse_cli(&s(&["ws", "list", "--limit", "0"])).is_err());
        assert!(parse_cli(&s(&["ca", "export", "/tmp/ca", "--pem", "--der"])).is_err());
        assert!(parse_cli(&s(&["proxy", "intercept", "--requests", "maybe"])).is_err());
        assert!(parse_cli(&s(&["proxy", "decide", &fid, "--revision", "1"])).is_err());
        assert!(
            parse_cli(&s(&[
                "proxy",
                "decide",
                &fid,
                "--revision",
                "1",
                "--forward",
                "--drop",
            ]))
            .is_err()
        );
        assert!(
            parse_cli(&s(&[
                "repeater",
                "open",
                "--target",
                "http://127.0.0.1:8080",
            ]))
            .is_err()
        );
        assert!(parse_cli(&s(&["repeater", "open", "--original"])).is_err());
        assert!(
            parse_cli(&s(&[
                "repeater",
                "send",
                "--flow",
                &fid,
                "--target",
                "http://127.0.0.1:8080",
                "--raw",
                "GET / HTTP/1.1\r\n\r\n",
            ]))
            .is_err()
        );
        assert!(
            parse_cli(&s(&[
                "repeater",
                "send",
                "--flow",
                &fid,
                "--add-header",
                "MalformedHeader",
            ]))
            .is_err()
        );
        assert!(
            parse_cli(&s(&[
                "repeater",
                "send",
                "--flow",
                &fid,
                "--remove-header",
                "   ",
            ]))
            .is_err()
        );
    }

    #[test]
    fn formats_valid_json_for_headless_readiness_and_errors() {
        let ready_json =
            format_headless_ready_json(Path::new("/tmp/mio-test.sock"), Some("127.0.0.1:8080"));
        let parsed: serde_json::Value = serde_json::from_str(&ready_json).unwrap();
        assert_eq!(parsed["ready"], serde_json::json!(true));
        assert_eq!(parsed["socket"], serde_json::json!("/tmp/mio-test.sock"));
        assert_eq!(parsed["proxy_address"], serde_json::json!("127.0.0.1:8080"));

        let no_proxy_json = format_headless_ready_json(Path::new("/tmp/mio-test.sock"), None);
        let parsed_no_proxy: serde_json::Value = serde_json::from_str(&no_proxy_json).unwrap();
        assert_eq!(parsed_no_proxy["ready"], serde_json::json!(true));
        assert_eq!(parsed_no_proxy["proxy_address"], serde_json::Value::Null);

        let err_json = AiIpcEnvelope::err_json("bad argument");
        let parsed_err: serde_json::Value = serde_json::from_str(&err_json).unwrap();
        assert_eq!(parsed_err["ok"], serde_json::json!(false));
        assert_eq!(parsed_err["error"], serde_json::json!("bad argument"));
    }

    #[tokio::test]
    async fn cli_executes_against_live_runtime_ipc_socket() {
        let dir = TempDirGuard::new("mio-cli");
        let sock = dir.path.join("cli.sock");
        let slim = dir.path.join("slim.mio");
        let sock_for_rt = sock.clone();
        let (runtime, _client) = tokio::task::spawn_blocking(move || {
            mio_core::Runtime::start_with_ipc_socket(sock_for_rt)
        })
        .await
        .unwrap()
        .unwrap();
        let sock_str = sock.to_string_lossy().into_owned();

        execute_cli(parse_cli(&s(&["--socket", &sock_str, "status", "--json"])).unwrap())
            .await
            .unwrap();
        execute_cli(parse_cli(&s(&["--socket", &sock_str, "flows", "list", "--limit", "5"])).unwrap())
            .await
            .unwrap();
        execute_cli(
            parse_cli(&s(&[
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
            .unwrap(),
        )
        .await
        .unwrap();
        execute_cli(
            parse_cli(&s(&[
                "--socket",
                &sock_str,
                "slim-save",
                &slim.to_string_lossy(),
                "--json",
            ]))
            .unwrap(),
        )
        .await
        .unwrap();
        assert!(slim.exists());

        tokio::task::spawn_blocking(move || runtime.shutdown())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn headless_server_exports_ca_starts_proxy_and_shuts_down_via_ipc() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = TempDirGuard::new("mio-srv");
        let identity_dir = dir.path.join("identity");
        let _id_guard = EnvVarGuard::set("MIO_IDENTITY_DIR", &identity_dir);

        let sock = dir.path.join("headless.sock");
        let ca_pem = dir.path.join("ca.pem");

        let server_sock = sock.clone();
        let server_ca = ca_pem.to_string_lossy().into_owned();
        let server_task = tokio::spawn(async move {
            run_headless_server(
                server_sock,
                Some("127.0.0.1:0".into()),
                Some(server_ca),
                true,
            )
            .await
        });

        let status_payload = {
            let mut found = None;
            for _ in 0..100 {
                let s = sock.clone();
                let res = tokio::task::spawn_blocking(move || {
                    call_ai_ipc(&s, &AiIpcRequest::Status, Duration::from_secs(2))
                })
                .await
                .unwrap();
                if let Ok((env, _)) = res
                    && env.ok
                    && let Some(mio_core::AiIpcPayload::Status {
                        proxy_address: Some(addr),
                        ..
                    }) = env.payload
                {
                    found = Some(addr);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            found.expect("headless server should become ready with an active proxy address")
        };
        assert!(status_payload.starts_with("127.0.0.1:"));

        let ca_contents = std::fs::read_to_string(&ca_pem).unwrap();
        assert!(ca_contents.contains("BEGIN CERTIFICATE"));

        let s_down = sock.clone();
        let (down_env, _) = tokio::task::spawn_blocking(move || {
            call_ai_ipc(&s_down, &AiIpcRequest::Shutdown, Duration::from_secs(5))
        })
        .await
        .unwrap()
        .unwrap();
        assert!(down_env.ok);

        server_task.await.unwrap().unwrap();
        assert!(!sock.exists());
    }

    #[tokio::test]
    async fn headless_server_rejects_invalid_listen_address_cleanly() {
        let dir = TempDirGuard::new("mio-bad");
        let sock = dir.path.join("bad.sock");
        let err = run_headless_server(sock, Some("not-a-socket-addr".into()), None, true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SocketAddr"));
    }
}
