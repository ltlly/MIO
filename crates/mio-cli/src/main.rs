use std::{
    io::Read as _,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64_STANDARD};
use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, error::ErrorKind};
use mio_core::{
    AiIpcEnvelope, AiIpcRequest, Command, Event, TabId, Uuid, call_ai_ipc,
    default_ipc_socket_path,
};

mod render;

const CLI_TIMEOUT: Duration = Duration::from_secs(30);
const CLI_REPEATER_SEND_TIMEOUT: Duration = Duration::from_secs(305);

#[derive(Parser, Debug, PartialEq, Eq)]
#[command(
    name = "mio-cli",
    version,
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
        /// 启动后切换为内置 CA 预置（见 ca presets）
        #[arg(long = "ca-preset")]
        ca_preset: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CliFlowRef {
    Uuid(Uuid),
    Number(u64),
}

impl CliFlowRef {
    fn into_pair(self) -> (Option<Uuid>, Option<u64>) {
        match self {
            Self::Uuid(id) => (Some(id), None),
            Self::Number(num) => (None, Some(num)),
        }
    }
}

fn parse_flow_ref(raw: &str) -> std::result::Result<CliFlowRef, String> {
    let trimmed = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    if let Ok(id) = Uuid::parse_str(trimmed) {
        return Ok(CliFlowRef::Uuid(id));
    }
    if let Ok(num) = trimmed.parse::<u64>() {
        if num == 0 {
            return Err("流量序号必须为正整数（从 1 开始）".into());
        }
        return Ok(CliFlowRef::Number(num));
    }
    Err(format!("无效的流量标识 `{raw}`：请提供流量 UUID 或正整数序号"))
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
        /// 隐藏 CONNECT 隧道流量（与 GUI 默认视图一致）
        #[arg(long = "hide-connect")]
        hide_connect: bool,
        /// 按主机名（或 host:port）过滤
        #[arg(long)]
        host: Option<String>,
        /// 按来源过滤（proxy|repeater|intruder|scanner|plugin）
        #[arg(long)]
        source: Option<String>,
        /// 按状态过滤（complete|pending|intercepted|dropped|cancelled|failed|abnormal）
        #[arg(long)]
        state: Option<String>,
        /// 按时间排序方向（asc|desc，默认 desc）
        #[arg(long)]
        order: Option<String>,
        /// 分页起始偏移量
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// 返回条数上限（1..=500）
        #[arg(long, default_value_t = 50, value_parser = parse_limit)]
        limit: usize,
    },
    /// 获取单条流量详情
    Get {
        /// 流量 UUID 或正整数序号（与 flows list 中的 #N 一致）
        #[arg(value_parser = parse_flow_ref)]
        flow: CliFlowRef,
        /// 显示拦截改写前的原始报文
        #[arg(long)]
        original: bool,
        /// 仅输出请求报文
        #[arg(long = "request-only", conflicts_with = "response_only")]
        request_only: bool,
        /// 仅输出响应报文
        #[arg(long = "response-only", conflicts_with = "request_only")]
        response_only: bool,
        /// 仅输出请求/响应头，不打印正文
        #[arg(long = "headers-only")]
        headers_only: bool,
        /// 将正文原样保存到文件（默认保存响应体；配合 --request-only 时保存请求体）
        #[arg(long = "save-body")]
        save_body: Option<PathBuf>,
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
struct RepeaterSourceArgs {
    /// 来源流量 UUID 或正整数序号
    #[arg(long = "flow", value_parser = parse_flow_ref, conflicts_with_all = ["target", "raw_request", "raw_file"])]
    flow: Option<CliFlowRef>,
    /// 使用拦截改写前的原始请求（仅配合 --flow）
    #[arg(long = "original", requires = "flow")]
    use_original: bool,
    /// 目标 URL/origin，例如 http://127.0.0.1:8080（省略 --raw/--raw-file 时自动生成 GET 模板）
    #[arg(long)]
    target: Option<String>,
    /// 原始 HTTP 请求文本（需配合 --target）
    #[arg(long = "raw", requires = "target", conflicts_with = "raw_file", allow_hyphen_values = true)]
    raw_request: Option<String>,
    /// 从文件读取原始 HTTP 请求（`-` 表示从 stdin 读取；需配合 --target）
    #[arg(long = "raw-file", requires = "target", conflicts_with = "raw_request")]
    raw_file: Option<PathBuf>,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum RepeaterCommand {
    /// 列出当前工作区中的所有 Repeater 标签页
    List,
    /// 在 Repeater 打开请求标签（支持来源流量、模板目标或空白标签）
    Open {
        #[command(flatten)]
        source: RepeaterSourceArgs,
        /// 新建空白 Repeater 标签页
        #[arg(long, conflicts_with_all = ["flow", "target", "raw_request", "raw_file", "use_original"])]
        blank: bool,
        /// 自定义标签标题
        #[arg(long, allow_hyphen_values = true)]
        title: Option<String>,
    },
    /// 重命名指定 Repeater 标签页
    Rename {
        /// Repeater 标签 UUID
        tab_id: TabId,
        /// 新的标签标题
        #[arg(allow_hyphen_values = true)]
        title: String,
    },
    /// 关闭指定 Repeater 标签页
    Close {
        /// Repeater 标签 UUID
        tab_id: TabId,
    },
    /// 通过 Repeater 发送单次请求
    Send {
        /// 直接向已有 Repeater 标签页发包并回写最近一次结果
        #[arg(long = "tab", conflicts_with_all = ["flow", "target", "raw_request", "raw_file", "use_original"])]
        tab_id: Option<TabId>,
        #[command(flatten)]
        source: RepeaterSourceArgs,
        /// 新增或覆盖请求头（格式 K:V，可重复）
        #[arg(long = "add-header", value_name = "K:V", value_parser = parse_header_kv)]
        add_headers: Vec<HeaderKv>,
        /// 移除指定请求头（可重复）
        #[arg(long = "remove-header", value_name = "NAME", value_parser = parse_non_empty_trimmed)]
        remove_headers: Vec<String>,
        /// 覆盖请求体文本
        #[arg(long = "body", conflicts_with = "body_file", allow_hyphen_values = true)]
        override_body: Option<String>,
        /// 从文件读取覆盖请求体（`-` 表示从 stdin 读取；支持任意二进制字节）
        #[arg(long = "body-file", conflicts_with = "override_body")]
        body_file: Option<PathBuf>,
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
    /// 列出内置 CA 预置
    Presets,
    /// 将 Proxy CA 切换为内置 CA 预置
    Use {
        /// 预置 ID（见 ca presets）
        preset_id: String,
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
    let json = cli.json;
    if cli.socket.is_none()
        && matches!(
            cli.command,
            CliCommand::Ca {
                command: CaCommand::Presets
            }
        )
    {
        let payload = mio_core::AiIpcPayload::CaPresets {
            presets: mio_core::builtin_ca_preset_infos(),
        };
        if json {
            println!("{}", serde_json::to_string(&AiIpcEnvelope::ok(payload))?);
        } else {
            render::render_human(&payload, &render::RenderOptions::default());
        }
        return Ok(());
    }
    let socket_path = cli.socket.unwrap_or_else(default_ipc_socket_path);
    let mut render_opts = render::RenderOptions::default();
    let req = match cli.command {
        CliCommand::Serve {
            listen,
            export_ca,
            ca_preset,
        } => {
            return run_headless_server(socket_path, listen, export_ca, ca_preset, json).await;
        }
        CliCommand::Status => AiIpcRequest::Status,
        CliCommand::Flows { command } => match command {
            FlowsCommand::List {
                filter,
                method,
                status,
                hide_connect,
                host,
                source,
                state,
                order,
                offset,
                limit,
            } => AiIpcRequest::FlowsList {
                offset,
                limit,
                filter,
                method,
                status,
                hide_connect,
                host,
                source,
                state,
                order,
            },
            FlowsCommand::Get {
                flow,
                original,
                request_only,
                response_only,
                headers_only,
                save_body,
            } => {
                let (flow_id, flow_number) = flow.into_pair();
                render_opts = render::RenderOptions {
                    show_original: original,
                    request_only,
                    response_only,
                    headers_only,
                    save_body,
                };
                AiIpcRequest::FlowsGet {
                    flow_id,
                    flow_number,
                }
            }
        },
        CliCommand::Ws { command } => match command {
            WsCommand::List { limit } => AiIpcRequest::WebSocketList { limit },
        },
        CliCommand::Repeater { command } => match command {
            RepeaterCommand::List => AiIpcRequest::RepeaterList,
            RepeaterCommand::Open {
                source,
                blank,
                title,
            } => {
                let (flow_id, flow_number) = source
                    .flow
                    .map(CliFlowRef::into_pair)
                    .unwrap_or((None, None));
                let raw_request = resolve_raw_request_input(source.raw_request, source.raw_file)?;
                AiIpcRequest::RepeaterOpen {
                    flow_id,
                    flow_number,
                    use_original: source.use_original,
                    target: source.target,
                    raw_request,
                    title,
                    blank,
                }
            }
            RepeaterCommand::Rename { tab_id, title } => {
                AiIpcRequest::RepeaterRename { tab_id, title }
            }
            RepeaterCommand::Close { tab_id } => AiIpcRequest::RepeaterClose { tab_id },
            RepeaterCommand::Send {
                tab_id,
                source,
                add_headers,
                remove_headers,
                override_body,
                body_file,
                insecure,
            } => {
                ensure!(
                    tab_id.is_some() || source.flow.is_some() || source.target.is_some(),
                    "repeater send 必须指定 --tab <tab-id>、--flow <uuid|number> 或 --target <url>"
                );
                let (flow_id, flow_number) = source
                    .flow
                    .map(CliFlowRef::into_pair)
                    .unwrap_or((None, None));
                let raw_request = resolve_raw_request_input(source.raw_request, source.raw_file)?;
                let override_body_base64 = match body_file {
                    Some(path) => Some(B64_STANDARD.encode(read_cli_input_bytes(&path, "--body-file")?)),
                    None => None,
                };
                AiIpcRequest::RepeaterSend {
                    tab_id,
                    flow_id,
                    flow_number,
                    use_original: source.use_original,
                    target: source.target,
                    raw_request,
                    add_headers: add_headers.into_iter().map(HeaderKv::into_pair).collect(),
                    remove_headers,
                    override_body,
                    override_body_base64,
                    insecure,
                }
            }
        },
        CliCommand::SlimSave { dest_path } => AiIpcRequest::SlimSave {
            path: dest_path,
            options: None,
        },
        CliCommand::Ca { command } => match command {
            CaCommand::Export {
                dest_path,
                pem,
                der,
            } => AiIpcRequest::ExportCa {
                path: dest_path,
                pem: if pem {
                    Some(true)
                } else if der {
                    Some(false)
                } else {
                    None
                },
            },
            CaCommand::Presets => AiIpcRequest::CaPresets,
            CaCommand::Use { preset_id } => AiIpcRequest::CaPresetUse { preset_id },
        },
        CliCommand::Proxy { command } => match command {
            ProxyCommand::Start { bind_address } => AiIpcRequest::ProxyStart { bind_address },
            ProxyCommand::Stop => AiIpcRequest::ProxyStop,
            ProxyCommand::Intercept {
                requests,
                responses,
            } => AiIpcRequest::ProxyIntercept {
                requests,
                responses,
            },
            ProxyCommand::Pending => AiIpcRequest::ProxyPending,
            ProxyCommand::Decide(args) => AiIpcRequest::ProxyDecide {
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
        },
        CliCommand::Workspace { command } => match command {
            WorkspaceCommand::Get => AiIpcRequest::WorkspaceGet,
            WorkspaceCommand::Set { json_path } => {
                let raw = std::fs::read_to_string(&json_path)
                    .with_context(|| format!("读取工作区 JSON 失败：{}", json_path.display()))?;
                let workspace_json = extract_workspace_patch_json(&raw)?;
                AiIpcRequest::WorkspacePatch { workspace_json }
            }
        },
        CliCommand::Shutdown => AiIpcRequest::Shutdown,
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
        let raw_err = env.error.unwrap_or_else(|| "IPC 请求失败".into());
        bail!("{}", rewrite_cli_error_hint(&raw_err));
    }
    let saved_body = if let Some(payload) = &env.payload {
        render::save_flow_body_if_requested(payload, &render_opts)?
    } else {
        None
    };
    if json {
        println!("{}", raw_line.trim());
        return Ok(());
    }
    let payload = env.payload.context("IPC 成功响应缺少数据载荷")?;
    render::render_human(&payload, &render_opts);
    if let Some((saved_path, byte_len)) = saved_body {
        println!(
            "\n已保存正文 ({}) -> {}",
            render::format_human_bytes(byte_len as u64),
            saved_path.display()
        );
    }
    Ok(())
}

fn read_cli_input_bytes(path: &Path, flag_name: &str) -> Result<Vec<u8>> {
    if path == Path::new("-") {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .with_context(|| format!("从标准输入读取 {flag_name} 失败"))?;
        Ok(buf)
    } else {
        std::fs::read(path)
            .with_context(|| format!("读取 {flag_name} 文件失败：{}", path.display()))
    }
}

fn resolve_raw_request_input(
    raw_request: Option<String>,
    raw_file: Option<PathBuf>,
) -> Result<Option<String>> {
    match (raw_request, raw_file) {
        (Some(raw), None) => Ok(Some(raw)),
        (None, Some(path)) => {
            let bytes = read_cli_input_bytes(&path, "--raw-file")?;
            let text = String::from_utf8(bytes)
                .context("--raw-file 内容必须为有效 UTF-8 文本（如需二进制正文请配合 --body-file）")?;
            Ok(Some(text))
        }
        (None, None) => Ok(None),
        (Some(_), Some(_)) => bail!("不能同时指定 --raw 与 --raw-file"),
    }
}

fn rewrite_cli_error_hint(msg: &str) -> String {
    if msg.contains("Proxy 设置 > Proxy listeners") {
        msg.replace(
            "请在“Proxy 设置 > Proxy listeners”中修改为其他空闲端口",
            "请通过 `--listen <IP:PORT>` 参数修改为其他空闲端口",
        )
    } else {
        msg.to_owned()
    }
}

fn format_headless_ready_json(
    socket_path: &Path,
    active_proxy: Option<&str>,
    ca_preset: Option<&str>,
) -> String {
    let mut value = serde_json::json!({
        "ready": true,
        "socket": socket_path.to_string_lossy(),
        "proxy_address": active_proxy,
    });
    if let Some(preset_id) = ca_preset {
        value["ca_preset"] = serde_json::Value::String(preset_id.to_owned());
    }
    value.to_string()
}

fn extract_workspace_patch_json(raw: &str) -> Result<String> {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(payload) = map.get("payload").and_then(|v| v.as_object())
            && let Some(ws) = payload.get("workspace").filter(|v| v.is_object())
        {
            return serde_json::to_string(ws).context("序列化内层工作区 JSON 失败");
        }
        if let Some(ws) = map.get("workspace").filter(|v| v.is_object()) {
            return serde_json::to_string(ws).context("序列化工作区字段 JSON 失败");
        }
    }
    Ok(raw.to_owned())
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
    ca_preset: Option<String>,
    json: bool,
) -> Result<()> {
    let preset = ca_preset
        .as_deref()
        .map(mio_core::resolve_ca_preset)
        .transpose()?;
    let applied_preset = preset.map(|p| p.id);
    let ca_dest = match export_ca {
        Some(raw) => {
            let trimmed = raw.trim();
            ensure!(!trimmed.is_empty(), "导出 CA 目标路径不能为空");
            Some(PathBuf::from(trimmed))
        }
        None => None,
    };
    let listen_addr: Option<SocketAddr> = match listen {
        Some(raw) => {
            let trimmed = raw.trim();
            ensure!(
                !trimmed.is_empty(),
                "--listen 不能为空，请提供有效的 SocketAddr（例如 127.0.0.1:8080）"
            );
            Some(
                trimmed
                    .parse()
                    .context("无效的监听 SocketAddr（例如 127.0.0.1:8080）")?,
            )
        }
        None => None,
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

        if let Some(preset) = preset {
            commands
                .send(Command::ImportCa {
                    cert_pem: preset.pem.to_string(),
                    key_pem: String::new(),
                })
                .await
                .context("发送内置 CA 预置命令失败")?;
            await_headless_event(&mut events, "import_ca", |ev| match ev {
                Event::CaRegenerated { .. } => Some(()),
                _ => None,
            })
            .await?;
        }

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
            let rewritten = rewrite_cli_error_hint(&format!("{err:#}"));
            bail!("{rewritten}");
        }
    };

    if json {
        println!(
            "{}",
            format_headless_ready_json(&socket_path, active_proxy.as_deref(), applied_preset)
        );
    } else if let Some(preset_id) = applied_preset {
        println!(
            "MIO headless 服务已就绪 | IPC={} | Proxy={} | CA={preset_id}",
            socket_path.display(),
            active_proxy.as_deref().unwrap_or("未启动")
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

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct IsolatedCliEnv {
        _id: EnvVarGuard,
        _prefs: EnvVarGuard,
        _plugins: EnvVarGuard,
    }

    impl IsolatedCliEnv {
        fn in_dir(root: &Path) -> Self {
            let identity = root.join("identity");
            let prefs = root.join("preferences.json");
            let plugins = root.join("plugins");
            Self {
                _id: EnvVarGuard::set("MIO_IDENTITY_DIR", &identity),
                _prefs: EnvVarGuard::set("MIO_USER_PREFERENCES_PATH", &prefs),
                _plugins: EnvVarGuard::set("MIO_PLUGINS_DIR", &plugins),
            }
        }
    }

    #[test]
    fn parses_all_cli_subcommands_and_global_flags() {
        let cli = parse_cli(&s(&["--socket", "/tmp/x.sock", "status", "--json"])).unwrap();
        assert_eq!(cli.socket, Some(PathBuf::from("/tmp/x.sock")));
        assert!(cli.json);
        assert_eq!(cli.command, CliCommand::Status);

        let cli = parse_cli(&s(&[
            "flows",
            "list",
            "--filter",
            "login",
            "--method",
            "POST",
            "--status",
            "200",
            "--hide-connect",
            "--host",
            "api.example.invalid",
            "--source",
            "proxy",
            "--state",
            "abnormal",
            "--order",
            "asc",
            "--offset",
            "5",
            "--limit",
            "20",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Flows {
                command: FlowsCommand::List {
                    filter: "login".into(),
                    method: Some("POST".into()),
                    status: Some(200),
                    hide_connect: true,
                    host: Some("api.example.invalid".into()),
                    source: Some("proxy".into()),
                    state: Some("abnormal".into()),
                    order: Some("asc".into()),
                    offset: 5,
                    limit: 20,
                }
            }
        );

        let fid = Uuid::new_v4();
        let cli = parse_cli(&s(&[
            "flows",
            "get",
            &fid.to_string(),
            "--original",
            "--headers-only",
            "--response-only",
            "--save-body",
            "/tmp/body.bin",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Flows {
                command: FlowsCommand::Get {
                    flow: CliFlowRef::Uuid(fid),
                    original: true,
                    request_only: false,
                    response_only: true,
                    headers_only: true,
                    save_body: Some(PathBuf::from("/tmp/body.bin")),
                }
            }
        );
        let cli_num = parse_cli(&s(&["flows", "get", "42", "--request-only"])).unwrap();
        assert_eq!(
            cli_num.command,
            CliCommand::Flows {
                command: FlowsCommand::Get {
                    flow: CliFlowRef::Number(42),
                    original: false,
                    request_only: true,
                    response_only: false,
                    headers_only: false,
                    save_body: None,
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
                    tab_id: None,
                    source: RepeaterSourceArgs {
                        flow: Some(CliFlowRef::Uuid(fid)),
                        use_original: true,
                        target: None,
                        raw_request: None,
                        raw_file: None,
                    },
                    add_headers: vec![HeaderKv {
                        name: "X-Test".into(),
                        value: "val".into(),
                    }],
                    remove_headers: vec!["Cookie".into()],
                    override_body: Some("{\"ok\":true}".into()),
                    body_file: None,
                    insecure: true,
                }
            }
        );

        let tid = Uuid::new_v4();
        assert_eq!(
            parse_cli(&s(&["repeater", "list"])).unwrap().command,
            CliCommand::Repeater {
                command: RepeaterCommand::List
            }
        );
        assert_eq!(
            parse_cli(&s(&["repeater", "rename", &tid.to_string(), "New Name"]))
                .unwrap()
                .command,
            CliCommand::Repeater {
                command: RepeaterCommand::Rename {
                    tab_id: tid,
                    title: "New Name".into(),
                }
            }
        );
        assert_eq!(
            parse_cli(&s(&["repeater", "close", &tid.to_string()]))
                .unwrap()
                .command,
            CliCommand::Repeater {
                command: RepeaterCommand::Close { tab_id: tid }
            }
        );
        assert!(matches!(
            parse_cli(&s(&["repeater", "open", "--blank"]))
                .unwrap()
                .command,
            CliCommand::Repeater {
                command: RepeaterCommand::Open { blank: true, .. }
            }
        ));
        assert!(matches!(
            &parse_cli(&s(&["repeater", "open", "--target", "http://127.0.0.1:8080"]))
                .unwrap()
                .command,
            CliCommand::Repeater {
                command: RepeaterCommand::Open {
                    source: RepeaterSourceArgs {
                        target: Some(t),
                        raw_request: None,
                        ..
                    },
                    ..
                }
            } if t == "http://127.0.0.1:8080"
        ));
        assert!(matches!(
            &parse_cli(&s(&["repeater", "send", "--tab", &tid.to_string(), "--body-file", "-"]))
                .unwrap()
                .command,
            CliCommand::Repeater {
                command: RepeaterCommand::Send {
                    tab_id: Some(t),
                    body_file: Some(p),
                    ..
                }
            } if *t == tid && p == Path::new("-")
        ));
        let ver_err = parse_cli(&s(&["--version"])).unwrap_err();
        assert_eq!(ver_err.kind(), ErrorKind::DisplayVersion);
        let ver_short = parse_cli(&s(&["-V"])).unwrap_err();
        assert_eq!(ver_short.kind(), ErrorKind::DisplayVersion);

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

        assert_eq!(
            parse_cli(&s(&["ca", "presets"])).unwrap().command,
            CliCommand::Ca {
                command: CaCommand::Presets
            }
        );
        assert_eq!(
            parse_cli(&s(&["ca", "use", "emudhra-emsign-dv-g3b"]))
                .unwrap()
                .command,
            CliCommand::Ca {
                command: CaCommand::Use {
                    preset_id: "emudhra-emsign-dv-g3b".into()
                }
            }
        );
        assert!(matches!(
            parse_cli(&s(&[
                "serve",
                "--ca-preset",
                "emudhra-aaa-dv-g3b"
            ]))
            .unwrap()
            .command,
            CliCommand::Serve {
                ca_preset: Some(ref id),
                ..
            } if id == "emudhra-aaa-dv-g3b"
        ));

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
                ca_preset: None,
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
            parse_cli(&s(&["flows", "get", "0"])).is_err(),
            "flow number 0 must be rejected"
        );
        assert!(
            parse_cli(&s(&[
                "flows",
                "get",
                "1",
                "--request-only",
                "--response-only",
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
        let ready_json = format_headless_ready_json(
            Path::new("/tmp/mio-test.sock"),
            Some("127.0.0.1:8080"),
            None,
        );
        let parsed: serde_json::Value = serde_json::from_str(&ready_json).unwrap();
        assert_eq!(parsed["ready"], serde_json::json!(true));
        assert_eq!(parsed["socket"], serde_json::json!("/tmp/mio-test.sock"));
        assert_eq!(parsed["proxy_address"], serde_json::json!("127.0.0.1:8080"));
        assert!(parsed.get("ca_preset").is_none());

        let no_proxy_json =
            format_headless_ready_json(Path::new("/tmp/mio-test.sock"), None, Some("emudhra-emsign-dv-g3b"));
        let parsed_no_proxy: serde_json::Value = serde_json::from_str(&no_proxy_json).unwrap();
        assert_eq!(parsed_no_proxy["ready"], serde_json::json!(true));
        assert_eq!(parsed_no_proxy["proxy_address"], serde_json::Value::Null);
        assert_eq!(
            parsed_no_proxy["ca_preset"],
            serde_json::json!("emudhra-emsign-dv-g3b")
        );

        let err_json = AiIpcEnvelope::err_json("bad argument");
        let parsed_err: serde_json::Value = serde_json::from_str(&err_json).unwrap();
        assert_eq!(parsed_err["ok"], serde_json::json!(false));
        assert_eq!(parsed_err["error"], serde_json::json!("bad argument"));
    }

    #[tokio::test]
    async fn offline_ca_presets_succeeds_without_running_server() {
        execute_cli(parse_cli(&s(&["ca", "presets"])).unwrap())
            .await
            .unwrap();
        execute_cli(parse_cli(&s(&["ca", "presets", "--json"])).unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cli_executes_against_live_runtime_ipc_socket() {
        let _lock = ENV_LOCK.lock().await;
        let dir = TempDirGuard::new("mio-cli");
        let _env = IsolatedCliEnv::in_dir(&dir.path);
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
        execute_cli(parse_cli(&s(&["--socket", &sock_str, "ca", "presets", "--json"])).unwrap())
            .await
            .unwrap();
        execute_cli(
            parse_cli(&s(&[
                "--socket",
                &sock_str,
                "ca",
                "use",
                "emudhra-aaa-dv-g3b",
                "--json",
            ]))
            .unwrap(),
        )
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
        let _lock = ENV_LOCK.lock().await;
        let dir = TempDirGuard::new("mio-srv");
        let _env = IsolatedCliEnv::in_dir(&dir.path);

        let sock = dir.path.join("headless.sock");
        let ca_pem = dir.path.join("ca.pem");

        let server_sock = sock.clone();
        let server_ca = ca_pem.to_string_lossy().into_owned();
        let server_task = tokio::spawn(async move {
            run_headless_server(
                server_sock,
                Some("127.0.0.1:0".into()),
                Some(server_ca),
                None,
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
    async fn headless_server_applies_builtin_ca_preset_and_exports_it() {
        let _lock = ENV_LOCK.lock().await;
        let dir = TempDirGuard::new("mio-preset");
        let _env = IsolatedCliEnv::in_dir(&dir.path);

        let sock = dir.path.join("preset.sock");
        let ca_pem = dir.path.join("preset-ca.pem");
        let preset = mio_core::ca_preset("emudhra-emsign-dv-g3b").unwrap();
        let expected = mio_proxy::MitmAuthority::from_pem_pair(preset.pem, "").unwrap();

        let server_sock = sock.clone();
        let server_ca = ca_pem.to_string_lossy().into_owned();
        let server_task = tokio::spawn(async move {
            run_headless_server(
                server_sock,
                None,
                Some(server_ca),
                Some("emudhra-emsign-dv-g3b".into()),
                true,
            )
            .await
        });

        let mut ready = false;
        for _ in 0..200 {
            if sock.exists() && ca_pem.exists() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(ready, "headless preset startup did not export CA");

        let exported = std::fs::read_to_string(&ca_pem).unwrap();
        assert_eq!(exported, expected.certificate_pem());

        let s_down = sock.clone();
        let (down_env, _) = tokio::task::spawn_blocking(move || {
            call_ai_ipc(&s_down, &AiIpcRequest::Shutdown, Duration::from_secs(5))
        })
        .await
        .unwrap()
        .unwrap();
        assert!(down_env.ok);
        server_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn headless_server_rejects_invalid_listen_address_cleanly() {
        let dir = TempDirGuard::new("mio-bad");
        let sock = dir.path.join("bad.sock");
        let err =
            run_headless_server(sock.clone(), Some("not-a-socket-addr".into()), None, None, true)
                .await
                .unwrap_err();
        assert!(err.to_string().contains("SocketAddr"));

        let blank_err = run_headless_server(sock.clone(), Some("   ".into()), None, None, true)
            .await
            .unwrap_err();
        assert!(blank_err.to_string().contains("--listen"));

        let blank_preset = run_headless_server(sock.clone(), None, None, Some("   ".into()), true)
            .await
            .unwrap_err();
        assert!(blank_preset.to_string().contains("不能为空"));

        let bad_preset = run_headless_server(sock, None, None, Some("missing-preset".into()), true)
            .await
            .unwrap_err();
        let bad_msg = bad_preset.to_string();
        assert!(bad_msg.contains("missing-preset"));
        assert!(bad_msg.contains("emudhra-emsign-dv-g3b"));
        assert!(bad_msg.contains("emudhra-aaa-dv-g3b"));
    }

    #[test]
    fn extract_workspace_patch_json_unwraps_workspace_get_response() {
        let raw_ws = r#"{"revision":3,"project_name":"demo"}"#;
        assert_eq!(extract_workspace_patch_json(raw_ws).unwrap(), raw_ws);

        let wrapped = r#"{"ok":true,"workspace":{"revision":3,"project_name":"demo"}}"#;
        let extracted: serde_json::Value =
            serde_json::from_str(&extract_workspace_patch_json(wrapped).unwrap()).unwrap();
        assert_eq!(extracted["revision"], 3);
        assert_eq!(extracted["project_name"], "demo");

        let nested = r#"{"ok":true,"payload":{"workspace":{"revision":4,"project_name":"n"}}}"#;
        let extracted2: serde_json::Value =
            serde_json::from_str(&extract_workspace_patch_json(nested).unwrap()).unwrap();
        assert_eq!(extracted2["revision"], 4);
    }

    #[test]
    fn rewrites_port_conflict_hint_for_cli_and_formats_human_bytes() {
        let gui_msg = "Proxy 监听地址 127.0.0.1:8080 已被占用（可能正由另一个 MIO 窗口或其他程序监听）；请在“Proxy 设置 > Proxy listeners”中修改为其他空闲端口（如 8081）后重试";
        let cli_msg = rewrite_cli_error_hint(gui_msg);
        assert!(!cli_msg.contains("Proxy 设置 > Proxy listeners"));
        assert!(cli_msg.contains("--listen <IP:PORT>"));
        assert!(cli_msg.contains("8081"));

        assert_eq!(render::format_human_bytes(512), "512 B");
        assert!(render::format_human_bytes(2048).contains("2.00 KiB"));
        assert!(render::format_human_bytes(3 * 1024 * 1024).contains("3.00 MiB"));
    }
}
