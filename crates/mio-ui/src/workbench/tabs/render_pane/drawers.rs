use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result as AnyResult, ensure};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as B64_STANDARD, URL_SAFE_NO_PAD as B64_URL_NO_PAD},
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    input::{Editor, EditorState, Input, InputState},
};
use serde::{Deserialize, Serialize};

use super::*;

const PRESET_REMOVE_COOKIE: &str = r#"// 移除当前请求中的 Cookie 请求头（等价于 External Custom Actions「Remove cookies」）
let prev = header_value(request, "Cookie");
if prev == "" {
    print("当前请求中不包含 Cookie 请求头。");
} else {
    remove_all_cookies(request);
    print("已移除 Cookie：" + prev);
}"#;

const PRESET_STRIP_AUTH: &str = r#"// 移除常见身份认证与 CSRF 请求头，用于未授权 / 越权访问对照测试
let targets = [
    "Cookie",
    "Authorization",
    "Proxy-Authorization",
    "X-Auth-Token",
    "X-API-Key",
    "X-CSRF-Token",
    "X-XSRF-Token"
];
let removed = [];
for name in targets {
    if has_header(request, name) {
        removed.push(name);
    }
}
remove_headers(request, targets);
if removed.is_empty() {
    print("当前请求中未发现常见认证请求头。");
} else {
    print("已移除认证请求头（" + removed.len() + " 项）：");
    for h in removed {
        print("  - " + h);
    }
}"#;

const PRESET_STRIP_CACHE: &str = r#"// 移除条件缓存请求头，强制服务端返回完整响应正文
let cache_headers = [
    "If-None-Match",
    "If-Modified-Since",
    "If-Match",
    "If-Unmodified-Since",
    "Cache-Control",
    "Pragma"
];
remove_headers(request, cache_headers);
print("已移除条件请求与缓存控制标头。");"#;

const PRESET_BYPASS_HEADERS: &str = r#"// 添加常见反向代理源 IP 伪造头，测试 IP 白名单或限流绕过
let spoof = "127.0.0.1";
for h in [
    "X-Forwarded-For",
    "X-Real-IP",
    "X-Originating-IP",
    "Client-IP",
    "X-Remote-Addr"
] {
    set_header(request, h, spoof);
}
print("已注入反代源 IP 伪造头（" + spoof + "）。");"#;

const PRESET_CORS_ORIGIN: &str = r#"// 设置第三方恶意 Origin 请求头，测试服务端 CORS 校验策略
let origin = "https://evil.example";
set_header(request, "Origin", origin);
print("已设置请求头 Origin: " + origin);"#;

const PRESET_CACHE_BUSTER: &str = r#"// 在 URL 查询参数中添加防缓存时间戳 _cb，测试缓存投毒或绕过 CDN 缓存
let ts = "" + unix_timestamp_ms();
set_query_param(request, "_cb", ts);
print("已附加防缓存参数：_cb=" + ts);
print("当前 URL：" + request.url);"#;

const PRESET_MINIFY_HEADERS: &str = r#"// 清理 Sec-Fetch-* / Sec-Ch-Ua* 等浏览器冗余请求头，保留最小化重放报文
remove_headers(request, [
    "Sec-Ch-Ua",
    "Sec-Ch-Ua-Mobile",
    "Sec-Ch-Ua-Platform",
    "Sec-Fetch-Site",
    "Sec-Fetch-Mode",
    "Sec-Fetch-User",
    "Sec-Fetch-Dest",
    "Upgrade-Insecure-Requests",
    "Priority",
    "DNT",
    "Referer",
    "Accept-Language"
]);
print("已精简浏览器指纹与冗余请求头。");"#;

const PRESET_JWT: &str = r#"// 从 Authorization 或 Cookie 中提取并解码 JWT Header / Payload
let auth = request.headerValue("Authorization");
let token = "";
if auth.len() > 7 {
    let prefix = auth.sub_string(0, 7);
    prefix.to_lower();
    if prefix == "bearer " {
        let rest = auth.sub_string(7, auth.len() - 7);
        rest.trim();
        token = rest;
    }
}
if token == "" {
    let cookie = request.headerValue("Cookie");
    if cookie != "" {
        for part in cookie.split(";") {
            let p = part;
            p.trim();
            let eq = p.index_of("=");
            if eq > 0 {
                let v = p.sub_string(eq + 1, p.len() - eq - 1);
                v.trim();
                if v.starts_with("eyJ") && v.split(".").len() == 3 {
                    token = v;
                    break;
                }
            }
        }
    }
}
if token == "" {
    print("未在 Authorization 或 Cookie 中检测到 JWT 令牌。");
} else {
    let segs = token.split(".");
    if segs.len() < 2 {
        print("检测到令牌，但不符合三段式 JWT 结构。");
    } else {
        print("=== JWT Header ===");
        print(base64url_decode(segs[0]));
        print("=== JWT Payload ===");
        print(base64url_decode(segs[1]));
        let sig_len = if segs.len() > 2 { segs[2].len() } else { 0 };
        print("签名段长度：" + sig_len + " 字符");
    }
}"#;

const PRESET_SECURITY_HEADERS: &str = r#"// 检查常见安全响应头、CORS 与服务端指纹
if response.status == 0 {
    print("当前标签尚未收到响应，请先发送请求。");
} else {
    print("响应状态码：" + response.status + " (" + response.version + ")");
    let required = [
        "Strict-Transport-Security",
        "Content-Security-Policy",
        "X-Frame-Options",
        "X-Content-Type-Options",
        "Referrer-Policy",
        "Permissions-Policy"
    ];
    let missing = 0;
    for h in required {
        let v = header_value(response, h);
        if v == "" {
            print("[缺失] " + h);
            missing += 1;
        } else {
            print("[存在] " + h + ": " + v);
        }
    }
    let acao = header_value(response, "Access-Control-Allow-Origin");
    let acac = header_value(response, "Access-Control-Allow-Credentials");
    if acao != "" {
        print("[CORS] Access-Control-Allow-Origin: " + acao);
        if acao == "*" && acac != "" {
            print("[警告] 通配符 CORS 与 Credentials 并存！");
        }
    }
    let srv = header_value(response, "Server");
    if srv != "" {
        print("[指纹] Server: " + srv);
    }
    print("安全头检查完毕，共缺失 " + missing + " / " + required.len() + " 项建议响应头。");
}"#;

const PRESET_EXTRACT_LINKS: &str = r#"// 从响应头 Location 与响应正文中提取链接与路径
if response.status == 0 {
    print("当前标签尚未收到响应，请先发送请求。");
} else {
    let body = response.body;
    if body.len() > 32768 {
        body = body.sub_string(0, 32768);
    }
    let found = [];
    for token in body.split("\"") {
        let t = token;
        t.trim();
        if t.starts_with("http://") || t.starts_with("https://")
            || (t.starts_with("/") && t.len() > 1 && !t.contains(" ") && !t.contains("<"))
        {
            if !found.contains(t) && found.len() < 100 {
                found.push(t);
            }
        }
    }
    for token in body.split("'") {
        let t = token;
        t.trim();
        if t.starts_with("http://") || t.starts_with("https://") {
            if !found.contains(t) && found.len() < 100 {
                found.push(t);
            }
        }
    }
    let loc = header_value(response, "Location");
    if loc != "" && !found.contains(loc) {
        found.push(loc);
    }
    if found.is_empty() {
        print("未在响应头或正文中提取到链接。");
    } else {
        print("共提取 " + found.len() + " 条链接/路径：");
        for u in found {
            print("  " + u);
        }
    }
}"#;

const PRESET_INSPECT_COOKIES: &str = r#"// 解析请求 Cookie 键值对与响应 Set-Cookie 安全属性
let req_cookie = header_value(request, "Cookie");
print("=== 请求 Cookie ===");
if req_cookie == "" {
    print("  (无请求 Cookie)");
} else {
    for item in req_cookie.split(";") {
        let pair = item;
        pair.trim();
        if pair != "" {
            print("  " + pair);
        }
    }
}
print("=== 响应 Set-Cookie ===");
if response.status == 0 {
    print("  (尚未收到响应)");
} else {
    let sc = header_value(response, "Set-Cookie");
    if sc == "" {
        print("  (无 Set-Cookie 响应头)");
    } else {
        print("  " + sc);
        let lower = sc;
        lower.to_lower();
        print("  HttpOnly=" + lower.contains("httponly") + ", Secure=" + lower.contains("secure") + ", SameSite=" + lower.contains("samesite"));
    }
}"#;

const PRESET_FIND_SECRETS: &str = r#"// 扫描响应头与正文中的敏感凭据、密钥与令牌模式
if response.status == 0 {
    print("当前标签尚未收到响应，请先发送请求。");
} else {
    let body = response.body;
    if body.len() > 65536 {
        body = body.sub_string(0, 65536);
    }
    let rules = [
        ["JWT", "eyJ[A-Za-z0-9_-]{8,}\\.eyJ[A-Za-z0-9_-]{8,}"],
        ["Bearer", "(?i)Bearer\\s+[A-Za-z0-9._-]{12,}"],
        ["AWS AKIA", "AKIA[0-9A-Z]{16}"],
        ["PrivateKey", "-----BEGIN [A-Z ]*PRIVATE KEY-----"],
        ["Credential", "(?i)(password|passwd|secret|api_key|access_token)\\s*[:=]\\s*[\"'][^\"']{4,}"]
    ];
    let hits = 0;
    for item in rules {
        if regex_matches(body, item[1]) || regex_matches(response.raw, item[1]) {
            print("[命中] " + item[0] + " (" + item[1] + ")");
            hits += 1;
        }
    }
    if hits == 0 {
        print("未在响应中检测到常见密钥或硬编码凭据模式。");
    } else {
        print("共命中 " + hits + " 类敏感模式。");
    }
}"#;

const PRESET_TO_CURL: &str = r#"// 生成可复现当前请求的 curl 命令
fn sh_quote(text) {
    let s = "" + text;
    s.replace("'", "'\"'\"'");
    "'" + s + "'"
}

let cmd = "curl -i -s -k -X " + request.method + " " + sh_quote(request.url);
for k in request.headers.keys() {
    let kl = "" + k;
    kl.to_lower();
    if kl == "content-length" {
        continue;
    }
    cmd += " \\\n  -H " + sh_quote(k + ": " + request.headers[k]);
}
if request.body != "" {
    cmd += " \\\n  --data-binary " + sh_quote(request.body);
}
print(cmd);"#;

const PRESET_STATS: &str = r#"// 统计请求与响应的参数数量、头部数量、长度及摘要
print("=== 请求概览 ===");
print("方法与版本：" + request.method + " (" + request.version + ")");
print("目标 URL：" + request.url);
print("请求头数量：" + request.headers.keys().len());
print("请求体字节长度：" + request.body.len());
print("原始请求总长度：" + request.raw.len());
let q = request.query;
let q_count = if q == "" { 0 } else { q.split("&").len() };
print("Query 参数数量：" + q_count);
if q != "" {
    for pair in q.split("&") {
        print("  [Query] " + pair);
    }
}
print("=== 响应概览 ===");
if response.status == 0 {
    print("尚未收到响应。");
} else {
    print("状态码：" + response.status + " (" + response.version + ")");
    print("响应头数量：" + response.headers.keys().len());
    print("响应体字节长度：" + response.body.len());
    print("原始响应总长度：" + response.raw.len());
    print("请求体 SHA-256：" + sha256_hex(request.body));
}"#;

const STARTER_CUSTOM_ACTION: &str = r#"// 自定义 Rhai 脚本示例（修改 request 后将直接更新当前草稿，print 内容输出到下方）
// 可用辅助函数：
//   remove_all_cookies(request) / remove_cookie(request, name) / set_cookie(request, name, val)
//   set_header(request, name, val) / remove_header(request, name) / remove_headers(request, [...])
//   set_query_param(request, k, v) / remove_query_param(request, k)
//   base64url_encode(s) / base64url_decode(s) / regex_matches(s, pat)
remove_all_cookies(request);
set_header(request, "X-Custom-Test", "mio");
print("已移除 Cookie 并添加 X-Custom-Test 请求头。");"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CustomActionCategory {
    MutateRequest,
    Inspect,
}

#[derive(Clone, Copy)]
struct CustomActionPreset {
    id: &'static str,
    category: CustomActionCategory,
    label: &'static str,
    description: &'static str,
    script: &'static str,
}

const CUSTOM_ACTION_PRESETS: [CustomActionPreset; 14] = [
    CustomActionPreset {
        id: "repeater-custom-action-preset-remove-cookie",
        category: CustomActionCategory::MutateRequest,
        label: "去除 Cookie",
        description: "移除当前请求中的 Cookie 标头",
        script: PRESET_REMOVE_COOKIE,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-strip-auth",
        category: CustomActionCategory::MutateRequest,
        label: "去除认证头",
        description: "移除 Cookie / Authorization / API Key / CSRF 等认证凭据头",
        script: PRESET_STRIP_AUTH,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-strip-cache",
        category: CustomActionCategory::MutateRequest,
        label: "去除缓存头",
        description: "移除 If-None-Match / If-Modified-Since / Cache-Control 等条件头",
        script: PRESET_STRIP_CACHE,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-bypass-headers",
        category: CustomActionCategory::MutateRequest,
        label: "反代绕过头",
        description: "注入 X-Forwarded-For / X-Real-IP 等本地源 IP 伪造头",
        script: PRESET_BYPASS_HEADERS,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-cors-origin",
        category: CustomActionCategory::MutateRequest,
        label: "CORS Origin 测试",
        description: "设置第三方 Origin 请求头用于检测 CORS 策略",
        script: PRESET_CORS_ORIGIN,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-cache-buster",
        category: CustomActionCategory::MutateRequest,
        label: "防缓存时间戳",
        description: "在 URL 中附加时间戳参数 _cb 绕过缓存",
        script: PRESET_CACHE_BUSTER,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-minify-headers",
        category: CustomActionCategory::MutateRequest,
        label: "精简冗余头",
        description: "清理 Sec-Fetch-* / Sec-Ch-Ua* / Referer 等浏览器噪声头",
        script: PRESET_MINIFY_HEADERS,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-jwt",
        category: CustomActionCategory::Inspect,
        label: "提取 JWT 载荷",
        description: "从 Authorization / Cookie 提取并解码 JWT Header 与 Payload",
        script: PRESET_JWT,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-headers",
        category: CustomActionCategory::Inspect,
        label: "检查安全响应头",
        description: "审计 HSTS / CSP / XFO / CORS 与 Server 响应头",
        script: PRESET_SECURITY_HEADERS,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-links",
        category: CustomActionCategory::Inspect,
        label: "提取响应链接",
        description: "提取 Location 标头及响应正文中的 URL 与路径",
        script: PRESET_EXTRACT_LINKS,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-inspect-cookies",
        category: CustomActionCategory::Inspect,
        label: "解析 Cookie 明细",
        description: "查看请求 Cookie 键值对与响应 Set-Cookie 安全属性",
        script: PRESET_INSPECT_COOKIES,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-find-secrets",
        category: CustomActionCategory::Inspect,
        label: "敏感信息扫描",
        description: "扫描响应中的 JWT / Bearer / AKIA / 私钥与硬编码口令特征",
        script: PRESET_FIND_SECRETS,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-to-curl",
        category: CustomActionCategory::Inspect,
        label: "生成 cURL 命令",
        description: "将当前请求导出为可直接在终端运行的 curl 命令",
        script: PRESET_TO_CURL,
    },
    CustomActionPreset {
        id: "repeater-custom-action-preset-stats",
        category: CustomActionCategory::Inspect,
        label: "统计参数与长度",
        description: "统计请求头、Query 参数、正文长度与 SHA-256 摘要",
        script: PRESET_STATS,
    },
];

struct CustomActionsUiState {
    script_editor: Entity<EditorState>,
    script_name: Entity<InputState>,
    active_preset: Option<usize>,
    selected_saved: Option<String>,
    saved_scripts: Vec<CustomActionScriptEntry>,
    output: String,
    error: Option<String>,
    saved_notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomActionScriptEntry {
    name: String,
    code: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomActionLibraryFile {
    version: u32,
    scripts: Vec<CustomActionScriptEntry>,
}

#[cfg(test)]
std::thread_local! {
    static TEST_CUSTOM_ACTIONS_PATH: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

fn custom_actions_library_path() -> AnyResult<Option<PathBuf>> {
    #[cfg(test)]
    if let Some(path) = TEST_CUSTOM_ACTIONS_PATH.with(|cell| cell.borrow().clone()) {
        Ok(Some(path))
    } else {
        Ok(None)
    }
    #[cfg(not(test))]
    Ok(Some(
        mio_core::default_user_preferences_path()?.with_file_name("repeater-custom-actions.json"),
    ))
}

fn read_custom_action_library_from_path(path: &Path) -> AnyResult<Vec<CustomActionScriptEntry>> {
    let Some(bytes) =
        mio_core::ui_files::read_owned_file_bounded(path, 4 * 1024 * 1024, "自定义动作脚本库")?
    else {
        return Ok(Vec::new());
    };
    parse_custom_action_library_bytes(&bytes)
}

fn parse_custom_action_library_bytes(bytes: &[u8]) -> AnyResult<Vec<CustomActionScriptEntry>> {
    let parsed: CustomActionLibraryFile =
        serde_json::from_slice(bytes).context("解析 Custom actions 脚本库失败")?;
    ensure!(parsed.version == 1, "Custom actions 脚本库版本不受支持");
    ensure!(
        parsed.scripts.len() <= 128,
        "Custom actions 脚本库数量超过 128 上限"
    );
    Ok(parsed.scripts)
}

fn mutate_custom_action_library_at(
    path: Option<&Path>,
    current: &[CustomActionScriptEntry],
    mutate: impl FnOnce(&mut Vec<CustomActionScriptEntry>) -> AnyResult<()>,
) -> AnyResult<Vec<CustomActionScriptEntry>> {
    if let Some(p) = path {
        let mut final_scripts = Vec::new();
        mio_core::ui_files::locked_cas_update_owned_file(
            p,
            "repeater-custom-actions.lock",
            4 * 1024 * 1024,
            "Custom actions 脚本库",
            |before| {
                let mut scripts = match before {
                    Some(bytes) => parse_custom_action_library_bytes(&bytes)?,
                    None => Vec::new(),
                };
                mutate(&mut scripts)?;
                let payload = serde_json::to_vec_pretty(&CustomActionLibraryFile {
                    version: 1,
                    scripts: scripts.clone(),
                })
                .context("序列化 Custom actions 脚本库失败")?;
                final_scripts = scripts;
                Ok(payload)
            },
        )?;
        Ok(final_scripts)
    } else {
        let mut scripts = current.to_vec();
        mutate(&mut scripts)?;
        Ok(scripts)
    }
}

fn save_custom_action_to_path(
    path: Option<&Path>,
    current: &[CustomActionScriptEntry],
    name: &str,
    code: &str,
) -> AnyResult<Vec<CustomActionScriptEntry>> {
    let trimmed_name = name.trim();
    ensure!(
        !trimmed_name.is_empty()
            && trimmed_name.len() <= 256
            && !trimmed_name.chars().any(char::is_control),
        "脚本名称须为 1–256 字节的非空文本且不含控制字符"
    );
    mio_core::validate_filter_script(code)?;
    mutate_custom_action_library_at(path, current, |scripts| {
        if let Some(existing) = scripts
            .iter_mut()
            .find(|entry| entry.name == trimmed_name)
        {
            existing.code = code.to_owned();
        } else {
            ensure!(scripts.len() < 128, "自定义动作脚本库最多保存 128 个脚本");
            scripts.push(CustomActionScriptEntry {
                name: trimmed_name.to_owned(),
                code: code.to_owned(),
            });
        }
        Ok(())
    })
}

fn delete_custom_action_from_path(
    path: Option<&Path>,
    current: &[CustomActionScriptEntry],
    name: &str,
) -> AnyResult<Vec<CustomActionScriptEntry>> {
    mutate_custom_action_library_at(path, current, |scripts| {
        scripts.retain(|entry| entry.name != name);
        Ok(())
    })
}

fn fallback_target_from_raw(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let head = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .map_or(text.as_ref(), |(h, _)| h);
    for line in head.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("host")
            && !v.trim().is_empty()
        {
            return format!("http://{}", v.trim());
        }
    }
    "http://localhost".into()
}

fn detect_response_line_version(raw: &[u8], fallback: HttpVersion) -> HttpVersion {
    let end = raw
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(raw.len());
    let first = std::str::from_utf8(&raw[..end]).unwrap_or_default();
    match first.split_ascii_whitespace().next() {
        Some("HTTP/1.0") => HttpVersion::Http10,
        Some("HTTP/1.1") => HttpVersion::Http11,
        Some("HTTP/2" | "HTTP/2.0") => HttpVersion::Http2,
        _ => fallback,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ExplanationSeverity {
    Ok,
    Info,
    Warn,
}

pub(super) struct ExplanationItem {
    badge: &'static str,
    severity: ExplanationSeverity,
    title: String,
    detail: String,
    mono: bool,
}

pub(super) struct RepeaterExplanationReport {
    title: String,
    overview: Vec<(&'static str, String)>,
    auth_items: Vec<ExplanationItem>,
    response_summary: Option<String>,
    cookie_items: Vec<ExplanationItem>,
    security_header_items: Vec<ExplanationItem>,
    ai_context: String,
}

#[derive(PartialEq, Eq)]
pub(super) enum ExplanationCacheKey {
    Http {
        tab_id: TabId,
        title: String,
        target_id: gpui_kit::EntityId,
        target_rev: u64,
        req_rev: crate::workbench::inspector::MessageEditorRevKey,
        resp_rev: crate::workbench::inspector::MessageEditorRevKey,
        fallback_ver: HttpVersion,
        loaded_flow: Option<(FlowId, u64, bool)>,
    },
    WebSocket {
        ws_id: TabId,
        title: String,
        url: String,
        direction: mio_core::WebSocketDirection,
        draft_kind: mio_core::WebSocketKind,
        headers_len: usize,
        history_len: usize,
        draft_revision: u64,
        draft_rev: Option<crate::workbench::inspector::MessageEditorRevKey>,
    },
    Empty,
}

fn decode_b64url_json(segment: &str) -> Option<String> {
    let trimmed = segment.trim().trim_end_matches('=');
    let bytes = B64_URL_NO_PAD
        .decode(trimmed)
        .or_else(|_| {
            let mut std_b64 = trimmed.replace('-', "+").replace('_', "/");
            while !std_b64.len().is_multiple_of(4) {
                std_b64.push('=');
            }
            B64_STANDARD.decode(&std_b64)
        })
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(val) => serde_json::to_string_pretty(&val).ok().or(Some(text)),
        Err(_) => Some(text),
    }
}

fn inspect_jwt_token(source: &str, token: &str, out: &mut Vec<ExplanationItem>) {
    let segs: Vec<&str> = token.split('.').collect();
    if segs.len() != 3 {
        return;
    }
    let Some(header_json) = decode_b64url_json(segs[0]) else {
        return;
    };
    let Some(payload_json) = decode_b64url_json(segs[1]) else {
        return;
    };
    let header_val = serde_json::from_str::<serde_json::Value>(&header_json).ok();
    let payload_val = serde_json::from_str::<serde_json::Value>(&payload_json).ok();
    let alg = header_val
        .as_ref()
        .and_then(|v| v.get("alg"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let alg_warn = alg.eq_ignore_ascii_case("none");
    out.push(ExplanationItem {
        badge: if alg_warn { "高危" } else { "JWT" },
        severity: if alg_warn {
            ExplanationSeverity::Warn
        } else {
            ExplanationSeverity::Info
        },
        title: format!("{source} · JWT Header (alg={alg})"),
        detail: header_json,
        mono: true,
    });
    let mut notes = Vec::new();
    let mut payload_sev = ExplanationSeverity::Info;
    if alg_warn {
        notes.push("警告：JWT 算法为 none，可能存在签名绕过风险！".to_owned());
        payload_sev = ExplanationSeverity::Warn;
    } else if alg.starts_with("HS") {
        notes.push(format!("对称签名算法 {alg}：可评估弱密钥爆破或算法混淆。"));
    }
    if let Some(obj) = payload_val.as_ref().and_then(|v| v.as_object()) {
        if let Some(sub) = obj.get("sub").and_then(|v| v.as_str()) {
            notes.push(format!("主体 sub={sub}"));
        }
        if let Some(role) = obj
            .get("role")
            .or_else(|| obj.get("roles"))
            .or_else(|| obj.get("scope"))
        {
            notes.push(format!("权限声明：{role}"));
        }
        if let Some(exp) = obj.get("exp").and_then(|v| v.as_i64()) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if exp < now {
                notes.push(format!("令牌已过期 {} 秒 (exp={exp})", now - exp));
                payload_sev = ExplanationSeverity::Warn;
            } else {
                notes.push(format!("令牌剩余有效 {} 秒 (exp={exp})", exp - now));
            }
        }
    }
    let detail = if notes.is_empty() {
        payload_json
    } else {
        format!("{payload_json}\n\n— {}", notes.join(" · "))
    };
    out.push(ExplanationItem {
        badge: if payload_sev == ExplanationSeverity::Warn {
            "注意"
        } else {
            "载荷"
        },
        severity: payload_sev,
        title: format!("{source} · JWT Payload"),
        detail,
        mono: true,
    });
}

fn summarize_request_body(content_type: &str, body: &[u8]) -> (String, bool) {
    if body.is_empty() {
        return ("空正文 (0 B)".into(), false);
    }
    let ct_lower = content_type.to_ascii_lowercase();
    if (ct_lower.contains("application/json") || body.first().is_some_and(|&b| b == b'{' || b == b'['))
        && let Ok(val) = serde_json::from_slice::<serde_json::Value>(body) {
            match &val {
                serde_json::Value::Object(map) => {
                    let keys: Vec<&str> = map.keys().map(String::as_str).collect();
                    let has_csrf_param = keys.iter().any(|k| {
                        let low = k.to_ascii_lowercase();
                        low.contains("csrf") || low.contains("xsrf") || low == "_token"
                    });
                    let preview = keys.iter().take(6).copied().collect::<Vec<_>>().join(", ");
                    let suffix = if keys.len() > 6 { " …" } else { "" };
                    return (
                        format!(
                            "JSON 对象 · {} 个字段 ({preview}{suffix}) · {} B",
                            keys.len(),
                            body.len()
                        ),
                        has_csrf_param,
                    );
                }
                serde_json::Value::Array(arr) => {
                    return (
                        format!("JSON 数组 · {} 项 · {} B", arr.len(), body.len()),
                        false,
                    );
                }
                _ => return (format!("JSON 标量 · {} B", body.len()), false),
            }
        }
    if ct_lower.contains("application/x-www-form-urlencoded") {
        let pairs: Vec<(String, String)> =
            form_urlencoded::parse(body).into_owned().collect();
        let has_csrf_param = pairs.iter().any(|(k, _)| {
            let low = k.to_ascii_lowercase();
            low.contains("csrf")
                || low.contains("xsrf")
                || low == "_token"
                || low.contains("authenticity_token")
        });
        let names = pairs
            .iter()
            .take(6)
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if pairs.len() > 6 { " …" } else { "" };
        return (
            format!(
                "Form URL-encoded · {} 个参数 ({names}{suffix}) · {} B",
                pairs.len(),
                body.len()
            ),
            has_csrf_param,
        );
    }
    if ct_lower.contains("multipart/form-data") {
        let boundary = content_type
            .split(';')
            .find_map(|part| {
                let p = part.trim();
                p.strip_prefix("boundary=")
                    .or_else(|| p.strip_prefix("BOUNDARY="))
                    .map(|b| b.trim_matches('"'))
            })
            .unwrap_or("");
        let part_count = if !boundary.is_empty() {
            let marker = format!("--{boundary}");
            String::from_utf8_lossy(body)
                .matches(&marker)
                .count()
                .saturating_sub(1)
        } else {
            0
        };
        let body_str = String::from_utf8_lossy(body).to_ascii_lowercase();
        let has_csrf_param = body_str.contains("csrf") || body_str.contains("xsrf");
        return (
            format!("Multipart 表单 · 约 {part_count} 段 · {} B", body.len()),
            has_csrf_param,
        );
    }
    if std::str::from_utf8(body).is_ok() {
        let kind = if ct_lower.contains("xml") {
            "XML"
        } else if ct_lower.contains("graphql") {
            "GraphQL"
        } else {
            "文本"
        };
        (format!("{kind} 正文 · {} B", body.len()), false)
    } else {
        (format!("二进制正文 · {} B", body.len()), false)
    }
}

fn truncate_for_ai_context(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.chars().count() <= max_chars {
        text.into_owned()
    } else {
        let head: String = text.chars().take(max_chars).collect();
        format!("{head}\n… [已截断，原始总计 {} 字节]", bytes.len())
    }
}

fn analyze_http_explanation(
    tab_title: &str,
    target: &str,
    raw_req: &[u8],
    req: Option<&mio_core::HttpRequest>,
    raw_resp: &[u8],
    resp: Option<&mio_core::HttpResponse>,
) -> RepeaterExplanationReport {
    let mut overview = Vec::new();
    let mut auth_items = Vec::new();
    let mut cookie_items = Vec::new();
    let mut security_header_items = Vec::new();
    let mut response_summary = None;

    if let Some(req) = req {
        let parsed_url = url::Url::parse(&req.url).ok();
        let path = parsed_url
            .as_ref()
            .map_or("/", |u| u.path())
            .to_owned();
        let query_pairs: Vec<(String, String)> = parsed_url
            .as_ref()
            .map(|u| u.query_pairs().into_owned().collect())
            .unwrap_or_default();
        let query_summary = if query_pairs.is_empty() {
            "无 (0)".to_owned()
        } else {
            let names = query_pairs
                .iter()
                .take(6)
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let more = if query_pairs.len() > 6 { " …" } else { "" };
            format!("{} 个 ({names}{more})", query_pairs.len())
        };
        let content_type = req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned())
            .unwrap_or_default();
        let (body_summary, body_has_csrf) = summarize_request_body(&content_type, &req.body);
        overview.push(("请求方法", format!("{} ({})", req.method, req.version.as_str())));
        overview.push(("目标源", if target.is_empty() { req.url.clone() } else { target.to_owned() }));
        overview.push(("路径", path));
        overview.push(("Query 参数", query_summary));
        overview.push(("请求头", format!("{} 项", req.headers.len())));
        overview.push(("请求正文", body_summary));

        // Auth & Session analysis
        let mut has_cookie = false;
        let mut has_csrf = body_has_csrf
            || query_pairs.iter().any(|(k, _)| {
                let low = k.to_ascii_lowercase();
                low.contains("csrf") || low.contains("xsrf") || low == "_token"
            });
        for header in &req.headers {
            let name_low = header.name.to_ascii_lowercase();
            let val = String::from_utf8_lossy(&header.value).trim().to_owned();
            if name_low == "authorization" {
                let val_low = val.to_ascii_lowercase();
                if let Some(rest) = val.get(7..).filter(|_| val_low.starts_with("bearer ")) {
                    let tok = rest.trim();
                    if tok.split('.').count() == 3 {
                        inspect_jwt_token("Authorization", tok, &mut auth_items);
                    } else {
                        auth_items.push(ExplanationItem {
                            badge: "Bearer",
                            severity: ExplanationSeverity::Info,
                            title: "Authorization: Bearer 令牌".into(),
                            detail: format!("非三段式 JWT 令牌，长度 {} 字符。", tok.len()),
                            mono: false,
                        });
                    }
                } else if let Some(rest) = val.get(6..).filter(|_| val_low.starts_with("basic ")) {
                    let decoded = B64_STANDARD
                        .decode(rest.trim())
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok());
                    let detail = match decoded {
                        Some(creds) => {
                            let user = creds.split(':').next().unwrap_or("");
                            format!(
                                "Basic 认证用户：{user}（密码已脱敏）{}",
                                if req.url.starts_with("http://") {
                                    "；警告：明文 HTTP 传输！"
                                } else {
                                    ""
                                }
                            )
                        }
                        None => "Basic 认证（Base64 解码失败）".into(),
                    };
                    auth_items.push(ExplanationItem {
                        badge: if req.url.starts_with("http://") {
                            "风险"
                        } else {
                            "Basic"
                        },
                        severity: if req.url.starts_with("http://") {
                            ExplanationSeverity::Warn
                        } else {
                            ExplanationSeverity::Info
                        },
                        title: "HTTP Basic 认证".into(),
                        detail,
                        mono: false,
                    });
                } else {
                    auth_items.push(ExplanationItem {
                        badge: "Auth",
                        severity: ExplanationSeverity::Info,
                        title: "自定义 Authorization 标头".into(),
                        detail: format!("长度 {} 字符", val.len()),
                        mono: false,
                    });
                }
            } else if matches!(
                name_low.as_str(),
                "x-api-key"
                    | "api-key"
                    | "x-auth-token"
                    | "x-access-token"
                    | "x-amz-security-token"
            ) {
                auth_items.push(ExplanationItem {
                    badge: "API Key",
                    severity: ExplanationSeverity::Info,
                    title: format!("检测到密钥标头：{}", header.name),
                    detail: format!("值长度 {} 字符，建议测试未授权访问与越权替换。", val.len()),
                    mono: false,
                });
            } else if matches!(
                name_low.as_str(),
                "x-csrf-token" | "x-xsrf-token" | "csrf-token" | "x-requested-with"
            ) {
                has_csrf = true;
                auth_items.push(ExplanationItem {
                    badge: "CSRF",
                    severity: ExplanationSeverity::Ok,
                    title: format!("存在防 CSRF 标头：{}", header.name),
                    detail: format!("值长度 {} 字符，可尝试删除或复用令牌验证服务端校验强度。", val.len()),
                    mono: false,
                });
            } else if name_low == "cookie" {
                has_cookie = true;
                let parts: Vec<&str> = val
                    .split(';')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .collect();
                let cookie_names: Vec<&str> = parts
                    .iter()
                    .filter_map(|p| p.split_once('=').map(|(k, _)| k.trim()))
                    .collect();
                auth_items.push(ExplanationItem {
                    badge: "Cookie",
                    severity: ExplanationSeverity::Info,
                    title: format!("请求携带 {} 个 Cookie", parts.len()),
                    detail: if cookie_names.is_empty() {
                        "已携带 Cookie 标头".into()
                    } else {
                        cookie_names.join(", ")
                    },
                    mono: false,
                });
                for part in parts {
                    if let Some((k, v)) = part.split_once('=') {
                        let v = v.trim();
                        if v.starts_with("eyJ") && v.split('.').count() == 3 {
                            inspect_jwt_token(&format!("Cookie({})", k.trim()), v, &mut auth_items);
                        }
                    }
                }
            }
        }
        let is_state_changing = matches!(
            req.method.as_str(),
            "POST" | "PUT" | "PATCH" | "DELETE"
        );
        if is_state_changing && has_cookie && !has_csrf {
            auth_items.push(ExplanationItem {
                badge: "CSRF 提醒",
                severity: ExplanationSeverity::Warn,
                title: "状态变更请求未检测到明显 CSRF 令牌".into(),
                detail: "该请求使用 Cookie 会话且方法为状态变更类型，建议检查 Origin/Referer 校验并生成 CSRF PoC 验证。".into(),
                mono: false,
            });
        }
        if auth_items.is_empty() {
            auth_items.push(ExplanationItem {
                badge: "匿名",
                severity: ExplanationSeverity::Info,
                title: "未检测到常见认证凭证".into(),
                detail: "请求中未发现 Authorization、Cookie 或 API Key 标头。".into(),
                mono: false,
            });
        }
    } else {
        overview.push(("状态", "当前请求草稿尚未构成完整 HTTP 报文".into()));
    }

    if let Some(resp) = resp {
        let resp_ct = resp
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| String::from_utf8_lossy(&h.value).into_owned())
            .unwrap_or_else(|| "未声明".into());
        response_summary = Some(format!(
            "HTTP {} ({}) · {} · 正文 {} B",
            resp.status,
            resp.version.as_str(),
            resp_ct,
            resp.body.len()
        ));

        // Set-Cookie audit
        for header in resp
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("set-cookie"))
        {
            let raw_val = String::from_utf8_lossy(&header.value).into_owned();
            let mut parts = raw_val.split(';').map(str::trim);
            let name_pair = parts.next().unwrap_or("");
            let cookie_name = name_pair
                .split_once('=')
                .map_or(name_pair, |(k, _)| k.trim());
            let attrs: Vec<String> = parts.map(|p| p.to_ascii_lowercase()).collect();
            let has_httponly = attrs.iter().any(|a| a == "httponly");
            let has_secure = attrs.iter().any(|a| a == "secure");
            let samesite = attrs.iter().find_map(|a| a.strip_prefix("samesite="));
            let mut issues = Vec::new();
            if !has_httponly {
                issues.push("缺少 HttpOnly");
            }
            if !has_secure {
                issues.push("缺少 Secure");
            }
            match samesite {
                None => issues.push("缺少 SameSite"),
                Some("none") if !has_secure => issues.push("SameSite=None 但缺少 Secure"),
                _ => {}
            }
            let sev = if issues.is_empty() {
                ExplanationSeverity::Ok
            } else {
                ExplanationSeverity::Warn
            };
            cookie_items.push(ExplanationItem {
                badge: if issues.is_empty() { "安全" } else { "警告" },
                severity: sev,
                title: format!("Set-Cookie: {cookie_name}"),
                detail: if issues.is_empty() {
                    format!(
                        "HttpOnly ✓ · Secure ✓ · SameSite={}",
                        samesite.unwrap_or("未指定")
                    )
                } else {
                    issues.join(" · ")
                },
                mono: false,
            });
        }

        let find_header = |name: &str| -> Option<String> {
            resp.headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .map(|h| String::from_utf8_lossy(&h.value).trim().to_owned())
        };

        // CORS
        if let Some(acao) = find_header("access-control-allow-origin") {
            let acac = find_header("access-control-allow-credentials").unwrap_or_default();
            let risky = (acao == "*" && acac.eq_ignore_ascii_case("true")) || acao == "null";
            security_header_items.push(ExplanationItem {
                badge: if risky { "风险" } else { "CORS" },
                severity: if risky {
                    ExplanationSeverity::Warn
                } else {
                    ExplanationSeverity::Info
                },
                title: format!("CORS: Allow-Origin = {acao}"),
                detail: if acac.is_empty() {
                    "未设置 Access-Control-Allow-Credentials".into()
                } else {
                    format!("Access-Control-Allow-Credentials: {acac}")
                },
                mono: false,
            });
        }

        // Core security headers
        for (hdr, label) in [
            ("content-security-policy", "CSP (Content-Security-Policy)"),
            ("strict-transport-security", "HSTS (Strict-Transport-Security)"),
            ("x-frame-options", "X-Frame-Options"),
            ("x-content-type-options", "X-Content-Type-Options"),
            ("cache-control", "Cache-Control"),
        ] {
            match find_header(hdr) {
                Some(val) => {
                    let warn = hdr == "x-content-type-options"
                        && !val.eq_ignore_ascii_case("nosniff");
                    security_header_items.push(ExplanationItem {
                        badge: if warn { "异常" } else { "存在" },
                        severity: if warn {
                            ExplanationSeverity::Warn
                        } else {
                            ExplanationSeverity::Ok
                        },
                        title: label.into(),
                        detail: val,
                        mono: false,
                    });
                }
                None => {
                    security_header_items.push(ExplanationItem {
                        badge: "缺失",
                        severity: ExplanationSeverity::Warn,
                        title: label.into(),
                        detail: "响应未配置此安全标头".into(),
                        mono: false,
                    });
                }
            }
        }

        // Server fingerprint
        let mut fingerprints = Vec::new();
        for fp in ["server", "x-powered-by", "x-aspnet-version", "via"] {
            if let Some(val) = find_header(fp) {
                fingerprints.push(format!("{fp}: {val}"));
            }
        }
        if !fingerprints.is_empty() {
            security_header_items.push(ExplanationItem {
                badge: "指纹",
                severity: ExplanationSeverity::Info,
                title: "服务端指纹信息暴露".into(),
                detail: fingerprints.join(" · "),
                mono: false,
            });
        }
    }

    let mut ai_lines = Vec::new();
    ai_lines.push(format!("[Repeater 标签] {tab_title}"));
    if !target.is_empty() {
        ai_lines.push(format!("[目标] {target}"));
    }
    ai_lines.push("\n=== 离线安全预分析摘要 ===".into());
    for (k, v) in &overview {
        ai_lines.push(format!("- {k}: {v}"));
    }
    for item in &auth_items {
        ai_lines.push(format!("- [{}] {}: {}", item.badge, item.title, item.detail));
    }
    for item in &cookie_items {
        ai_lines.push(format!("- [{}] {}: {}", item.badge, item.title, item.detail));
    }
    for item in &security_header_items {
        if item.severity == ExplanationSeverity::Warn {
            ai_lines.push(format!("- [{}] {}: {}", item.badge, item.title, item.detail));
        }
    }
    ai_lines.push("\n=== HTTP Request ===".into());
    ai_lines.push(truncate_for_ai_context(raw_req, 8192));
    if !raw_resp.is_empty() {
        ai_lines.push("\n=== HTTP Response ===".into());
        ai_lines.push(truncate_for_ai_context(raw_resp, 8192));
    }

    RepeaterExplanationReport {
        title: tab_title.to_owned(),
        overview,
        auth_items,
        response_summary,
        cookie_items,
        security_header_items,
        ai_context: ai_lines.join("\n"),
    }
}

fn xml_escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn render_explanation_badge(badge: &'static str, sev: ExplanationSeverity) -> AnyElement {
    let dark = theme::is_dark();
    let (bg, fg) = match sev {
        ExplanationSeverity::Ok => (
            rgb(if dark { 0x1e3a2b } else { 0xe6f4ea }),
            rgb(if dark { 0x7ee787 } else { 0x137333 }),
        ),
        ExplanationSeverity::Warn => (
            rgb(if dark { 0x452b18 } else { 0xfef3e6 }),
            rgb(if dark { 0xffb454 } else { 0xb54708 }),
        ),
        ExplanationSeverity::Info => (
            rgb(if dark { 0x26354d } else { 0xe8f0fe }),
            rgb(if dark { 0x79c0ff } else { 0x1967d2 }),
        ),
    };
    div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(3.))
        .flex_shrink_0()
        .bg(bg)
        .text_color(fg)
        .text_size(theme::scaled_px(10.5))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .child(badge)
        .into_any_element()
}

fn render_explanation_card(
    title: &'static str,
    items: &[ExplanationItem],
) -> AnyElement {
    let card_bg: gpui_kit::Hsla =
        rgb(if theme::is_dark() { 0x222428 } else { 0xf8f9fa }).into();
    let mut card = div()
        .w_full()
        .p(px(8.))
        .rounded(px(4.))
        .bg(card_bg)
        .border_1()
        .border_color(theme::border())
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(theme::scaled_px(12.))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(theme::foreground())
                .child(title),
        );
    for item in items {
        let detail_view = if item.mono {
            div()
                .w_full()
                .p(px(6.))
                .rounded(px(3.))
                .bg(theme::background())
                .border_1()
                .border_color(theme::border())
                .font_family(theme::mono_font_family())
                .text_size(theme::scaled_px(11.))
                .text_color(theme::foreground())
                .child(item.detail.clone())
        } else {
            div()
                .text_size(theme::scaled_px(11.5))
                .text_color(theme::muted_foreground())
                .child(item.detail.clone())
        };
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(render_explanation_badge(item.badge, item.severity))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(theme::scaled_px(12.))
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(theme::foreground())
                                .child(item.title.clone()),
                        ),
                )
                .child(detail_view),
        );
    }
    card.into_any_element()
}

impl Workbench {
    pub(in crate::workbench) fn open_active_repeater_in_mio_ai(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let http_payload = if let Some(tab_id) = self.workspace.active
            && let Some(editors) = self.tab_editors.get(&tab_id)
        {
            let tab_title = self
                .workspace
                .tabs
                .iter()
                .find(|t| t.id == tab_id)
                .map_or_else(|| "Repeater".to_owned(), |t| t.title.clone());
            let target = editing::input_snapshot(&editors.target, window, cx)
                .map(|s| s.trim().to_owned())
                .unwrap_or_else(|_| {
                    self.workspace
                        .tabs
                        .iter()
                        .find(|t| t.id == tab_id)
                        .map_or_else(String::new, |t| t.draft.target.clone())
                });
            let req_bytes = editors
                .request
                .bytes(window, cx)
                .unwrap_or_else(|_| editors.request.bytes_lossless(cx));
            let resp_bytes = editors.response.bytes_lossless(cx);
            let mut ctx = format!(
                "[Repeater 标签] {tab_title}\n[目标] {target}\n\n=== HTTP Request ===\n{}",
                truncate_for_ai_context(&req_bytes, 12_288)
            );
            if !resp_bytes.is_empty() {
                ctx.push_str("\n\n=== HTTP Response ===\n");
                ctx.push_str(&truncate_for_ai_context(&resp_bytes, 12_288));
            }
            Some((tab_title, ctx))
        } else {
            None
        };
        if let Some((tab_title, ctx)) = http_payload {
            self.open_mio_ai_with_context(tab_title, ctx, cx);
            return;
        }
        let ws_payload = if let Some(ws_id) = self.workspace.active_websocket_tab
            && let Some(ws_tab) = self
                .workspace
                .websocket_tabs
                .iter()
                .find(|t| !t.closed && t.id == ws_id)
        {
            let draft_bytes = self
                .websocket_repeater
                .editors
                .get(&ws_id)
                .map(|ed| ed.draft.bytes_lossless(cx))
                .unwrap_or_else(|| ws_tab.draft_payload.clone());
            let ctx = format!(
                "[WebSocket Repeater 标签] {}\n[URL] {}\n[方向] {:?}\n[历史帧数] {}\n\n=== Draft Payload ===\n{}",
                ws_tab.title,
                ws_tab.url,
                ws_tab.direction,
                ws_tab.history.len(),
                truncate_for_ai_context(&draft_bytes, 12_288)
            );
            Some((ws_tab.title.clone(), ctx))
        } else {
            None
        };
        if let Some((title, ctx)) = ws_payload {
            self.open_mio_ai_with_context(title, ctx, cx);
            return;
        }
        self.mio_ai_panel_open = true;
        cx.notify();
    }

    pub(in crate::workbench) fn save_repeater_entire_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_picker_pending()
            || !self.backend_available()
            || self.transition.is_some()
            || self.project.is_none()
        {
            return;
        }
        let (suggested_name, xml_payload, json_payload, count_label) =
            if let Some(tab_id) = self.workspace.active
                && let Some(editors) = self.tab_editors.get(&tab_id)
            {
                let tab_meta = self.workspace.tabs.iter().find(|t| t.id == tab_id);
                let title = tab_meta.map_or("repeater", |t| t.title.as_str()).to_owned();
                let target = editing::input_snapshot(&editors.target, window, cx)
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_else(|_| {
                        tab_meta.map_or_else(String::new, |t| t.draft.target.clone())
                    });
                let version = tab_meta.map_or(HttpVersion::Http11, |t| t.draft.version);
                let raw_req = editors
                    .request
                    .bytes(window, cx)
                    .unwrap_or_else(|_| editors.request.bytes_lossless(cx));
                let raw_resp = editors.response.bytes_lossless(cx);
                let effective_target = if target.is_empty() {
                    fallback_target_from_raw(&raw_req)
                } else {
                    target.clone()
                };
                let parsed_req = mio_core::parse_request(&raw_req, &effective_target, version).ok();
                let parsed_resp = if raw_resp.is_empty() {
                    editors
                        .loaded_flow
                        .as_ref()
                        .and_then(|f| f.response.clone())
                } else {
                    mio_core::parse_response(
                        &raw_resp,
                        detect_response_line_version(&raw_resp, version),
                    )
                    .ok()
                };
                let history_rows: Vec<mio_core::RepeaterHistoryEntry> = if self
                    .repeater_history
                    .owner
                    .is_some_and(|(_, id)| id == tab_id) { {
                        self.repeater_history
                            .page
                            .as_ref()
                            .map(|p| p.rows.clone())
                            .unwrap_or_default()
                    } } else { Default::default() };

                let url_str = parsed_req
                    .as_ref()
                    .map_or_else(|| effective_target.clone(), |r| r.url.clone());
                let parsed_url = url::Url::parse(&url_str).ok();
                let host = parsed_url
                    .as_ref()
                    .and_then(|u| u.host_str())
                    .unwrap_or("")
                    .to_owned();
                let port = parsed_url
                    .as_ref()
                    .and_then(|u| u.port_or_known_default())
                    .unwrap_or(80);
                let protocol = parsed_url
                    .as_ref()
                    .map_or("http", |u| u.scheme())
                    .to_owned();
                let method = parsed_req
                    .as_ref()
                    .map_or_else(|| "GET".to_owned(), |r| r.method.clone());
                let path = parsed_url
                    .as_ref()
                    .map_or_else(|| "/".to_owned(), |u| u.path().to_owned());
                let status = parsed_resp.as_ref().map_or(0u16, |r| r.status);
                let req_b64 = B64_STANDARD.encode(&raw_req);
                let resp_b64 = B64_STANDARD.encode(&raw_resp);

                let mut xml = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<items mioVersion=\"{}\">\n",
                    env!("CARGO_PKG_VERSION")
                );
                xml.push_str(&format!(
                    "  <item>\n    <title>{}</title>\n    <url>{}</url>\n    <host>{}</host>\n    <port>{}</port>\n    <protocol>{}</protocol>\n    <method>{}</method>\n    <path>{}</path>\n    <request base64=\"true\">{}</request>\n    <status>{}</status>\n    <responselength>{}</responselength>\n    <response base64=\"true\">{}</response>\n  </item>\n",
                    xml_escape_text(&title),
                    xml_escape_text(&url_str),
                    xml_escape_text(&host),
                    port,
                    xml_escape_text(&protocol),
                    xml_escape_text(&method),
                    xml_escape_text(&path),
                    req_b64,
                    status,
                    raw_resp.len(),
                    resp_b64,
                ));
                for row in &history_rows {
                    xml.push_str(&format!(
                        "  <historyEntry position=\"{}\">\n    <method>{}</method>\n    <url>{}</url>\n    <status>{}</status>\n    <diagnostic>{}</diagnostic>\n  </historyEntry>\n",
                        row.position + 1,
                        xml_escape_text(row.method.as_deref().unwrap_or("")),
                        xml_escape_text(row.target.as_deref().unwrap_or("")),
                        row.status.map_or_else(String::new, |s| s.to_string()),
                        xml_escape_text(row.diagnostic.as_deref().unwrap_or("")),
                    ));
                }
                xml.push_str("</items>\n");

                let json_val = serde_json::json!({
                    "tab_title": title,
                    "target": effective_target,
                    "url": url_str,
                    "method": method,
                    "status": status,
                    "request_raw": String::from_utf8_lossy(&raw_req),
                    "response_raw": String::from_utf8_lossy(&raw_resp),
                    "history_entries": history_rows.iter().map(|r| serde_json::json!({
                        "position": r.position + 1,
                        "method": r.method,
                        "target": r.target,
                        "status": r.status,
                        "diagnostic": r.diagnostic,
                    })).collect::<Vec<_>>(),
                });
                let json = serde_json::to_string_pretty(&json_val).unwrap_or_default();
                let count = 1 + history_rows.len();
                (
                    "repeater-history.xml",
                    xml.into_bytes(),
                    json.into_bytes(),
                    format!("Repeater 完整历史（共 {count} 条）"),
                )
            } else if let Some(ws_id) = self.workspace.active_websocket_tab
                && let Some(ws_tab) = self
                    .workspace
                    .websocket_tabs
                    .iter()
                    .find(|t| !t.closed && t.id == ws_id)
            {
                let mut xml = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<websocketHistory mioVersion=\"{}\">\n",
                    env!("CARGO_PKG_VERSION")
                );
                for item in &ws_tab.history {
                    xml.push_str(&format!(
                        "  <frame sequence=\"{}\" direction=\"{:?}\" kind=\"{:?}\" timestamp=\"{}\">\n    <url>{}</url>\n    <payload base64=\"true\">{}</payload>\n  </frame>\n",
                        item.sequence,
                        item.direction,
                        item.kind,
                        item.observed_unix_ms,
                        xml_escape_text(&ws_tab.url),
                        B64_STANDARD.encode(&item.payload),
                    ));
                }
                xml.push_str("</websocketHistory>\n");
                let json_val = serde_json::json!({
                    "tab_title": ws_tab.title,
                    "url": ws_tab.url,
                    "frames": ws_tab.history.iter().map(|item| serde_json::json!({
                        "sequence": item.sequence,
                        "direction": format!("{:?}", item.direction),
                        "kind": format!("{:?}", item.kind),
                        "observed_unix_ms": item.observed_unix_ms,
                        "payload": String::from_utf8_lossy(&item.payload),
                    })).collect::<Vec<_>>(),
                });
                let json = serde_json::to_string_pretty(&json_val).unwrap_or_default();
                let count = ws_tab.history.len();
                (
                    "ws-repeater-history.xml",
                    xml.into_bytes(),
                    json.into_bytes(),
                    format!("WebSocket Repeater 完整历史（共 {count} 帧）"),
                )
            } else {
                self.fail("当前没有可保存历史的 Repeater 标签页。", cx);
                return;
            };

        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(suggested_name),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，本次历史保存已取消。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut dest))) => {
                            if dest.extension().is_none() {
                                dest.set_extension("xml");
                            }
                            let is_json = dest
                                .extension()
                                .and_then(|e| e.to_str())
                                .is_some_and(|e| e.eq_ignore_ascii_case("json"));
                            let bytes = if is_json { &json_payload } else { &xml_payload };
                            match mio_core::ui_files::write_export_file(&dest, bytes) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已保存{count_label}至：{}",
                                        dest.display()
                                    );
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("保存 Repeater 历史失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => view.fail(format!("无法选择保存路径：{err}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    #[inline(never)]
    fn render_custom_actions_header(
        &self,
        ui_state: &Entity<CustomActionsUiState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state_for_new = ui_state.clone();
        toolbar()
            .flex_nowrap()
            .gap(px(5.))
            .px(px(8.))
            .child(
                gpui_kit::component::Icon::new(IconName::Zap)
                    .size(theme::scaled_px(13.))
                    .text_color(theme::accent()),
            )
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Custom actions"),
            )
            .child(div().flex_1())
            .child(
                Button::new("repeater-custom-action-new")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(22.))
                    .px(px(6.))
                    .label("新建")
                    .tooltip("清空为自定义请求修改 + 日志输出示例模板")
                    .on_click(cx.listener(move |_, _, window, cx| {
                        state_for_new.update(cx, |st, cx| {
                            st.active_preset = None;
                            st.selected_saved = None;
                            st.error = None;
                            st.saved_notice = None;
                            st.script_name.update(cx, |inp, cx| {
                                inp.set_value("", window, cx);
                                inp.focus(window, cx);
                            });
                            st.script_editor.update(cx, |ed, cx| {
                                ed.set_value(STARTER_CUSTOM_ACTION, window, cx);
                            });
                        });
                        cx.notify();
                    })),
            )
            .child(
                Button::new("repeater-custom-actions-close")
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(24.))
                    .h(theme::scaled_px(24.))
                    .icon(IconName::Close)
                    .accessibility_label("关闭 Custom actions")
                    .tooltip("收起 Repeater Custom actions 侧栏")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.repeater_custom_actions_open = false;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_custom_actions_preset_group(
        &self,
        title: &'static str,
        category: CustomActionCategory,
        ui_state: &Entity<CustomActionsUiState>,
        active_preset: Option<usize>,
        has_http_tab: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut chips = div().flex().flex_wrap().items_center().gap(px(4.));
        for (idx, preset) in CUSTOM_ACTION_PRESETS.into_iter().enumerate() {
            if preset.category != category {
                continue;
            }
            let state_load = ui_state.clone();
            let state_exec = ui_state.clone();
            let is_active = active_preset == Some(idx);
            chips = chips.child(
                div()
                    .flex()
                    .items_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if is_active {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(if is_active {
                        theme::selected()
                    } else {
                        theme::toolbar_surface()
                    })
                    .overflow_hidden()
                    .child(
                        Button::new(preset.id)
                            .ghost()
                            .small()
                            .compact()
                            .rounded(ButtonRounded::None)
                            .h(theme::scaled_px(21.))
                            .px(px(6.))
                            .label(preset.label)
                            .tooltip(format!("载入脚本：{}", preset.description))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                state_load.update(cx, |st, cx| {
                                    st.active_preset = Some(idx);
                                    st.selected_saved = None;
                                    st.error = None;
                                    st.saved_notice = None;
                                    st.script_editor.update(cx, |ed, cx| {
                                        ed.set_value(preset.script, window, cx);
                                    });
                                    st.script_name.update(cx, |inp, cx| {
                                        inp.set_value(preset.label, window, cx);
                                    });
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{}-run", preset.id)))
                            .ghost()
                            .small()
                            .compact()
                            .rounded(ButtonRounded::None)
                            .w(theme::scaled_px(20.))
                            .h(theme::scaled_px(21.))
                            .border_l_1()
                            .border_color(theme::border())
                            .icon(IconName::Play)
                            .disabled(!has_http_tab)
                            .accessibility_label(format!("立即执行：{}", preset.label))
                            .tooltip(format!("载入并立即执行「{}」", preset.label))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                state_exec.update(cx, |st, cx| {
                                    st.active_preset = Some(idx);
                                    st.selected_saved = None;
                                    st.script_editor.update(cx, |ed, cx| {
                                        ed.set_value(preset.script, window, cx);
                                    });
                                    st.script_name.update(cx, |inp, cx| {
                                        inp.set_value(preset.label, window, cx);
                                    });
                                });
                                this.execute_custom_action_into_ui(
                                    preset.script,
                                    &state_exec,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .text_size(theme::scaled_px(11.))
                    .text_color(theme::muted_foreground())
                    .child(title),
            )
            .child(chips)
            .into_any_element()
    }

    #[inline(never)]
    fn render_custom_actions_saved_section(
        &self,
        ui_state: &Entity<CustomActionsUiState>,
        saved_scripts: &[CustomActionScriptEntry],
        selected_saved: Option<&str>,
        has_http_tab: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut saved_section = div()
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(theme::scaled_px(11.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(theme::muted_foreground())
                            .child(format!("我的自定义脚本 ({})", saved_scripts.len())),
                    ),
            );
        if saved_scripts.is_empty() {
            saved_section = saved_section.child(
                div()
                    .id("repeater-custom-actions-saved-empty")
                    .test_support()
                    .text_size(theme::scaled_px(11.))
                    .text_color(theme::muted_foreground())
                    .child("暂无已保存的自定义脚本，在下方编辑名称与代码后点击「保存脚本」。"),
            );
        } else {
            let mut list = div()
                .id("repeater-custom-actions-saved-list")
                .test_support()
                .max_h(px(86.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(2.));
            for (idx, entry) in saved_scripts.iter().enumerate() {
                let entry_name_load = entry.name.clone();
                let entry_code_load = entry.code.clone();
                let entry_name_run = entry.name.clone();
                let entry_code_run = entry.code.clone();
                let entry_name_del = entry.name.clone();
                let is_sel = selected_saved == Some(entry.name.as_str());
                let st_load = ui_state.clone();
                let st_run = ui_state.clone();
                let st_del = ui_state.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!(
                            "repeater-custom-action-saved-row-{idx}"
                        )))
                        .test_support()
                        .h(theme::scaled_px(22.))
                        .px(px(6.))
                        .rounded(px(3.))
                        .bg(if is_sel {
                            theme::selected()
                        } else {
                            theme::toolbar_surface()
                        })
                        .border_1()
                        .border_color(if is_sel {
                            theme::accent()
                        } else {
                            theme::border()
                        })
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "repeater-custom-action-saved-select-{idx}"
                                )))
                                .flex_1()
                                .min_w_0()
                                .cursor_pointer()
                                .truncate()
                                .text_size(theme::scaled_px(11.5))
                                .text_color(theme::foreground())
                                .child(entry.name.clone())
                                .on_click(cx.listener(move |_, _, window, cx| {
                                    let name = entry_name_load.clone();
                                    let code = entry_code_load.clone();
                                    st_load.update(cx, |st, cx| {
                                        st.active_preset = None;
                                        st.selected_saved = Some(name.clone());
                                        st.error = None;
                                        st.saved_notice = None;
                                        st.script_name.update(cx, |inp, cx| {
                                            inp.set_value(name, window, cx);
                                        });
                                        st.script_editor.update(cx, |ed, cx| {
                                            ed.set_value(code, window, cx);
                                        });
                                    });
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "repeater-custom-action-saved-run-{idx}"
                            )))
                            .ghost()
                            .small()
                            .compact()
                            .w(theme::scaled_px(20.))
                            .h(theme::scaled_px(18.))
                            .icon(IconName::Play)
                            .disabled(!has_http_tab)
                            .accessibility_label("运行已保存脚本")
                            .tooltip("立即在当前请求上执行此自定义脚本")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let name = entry_name_run.clone();
                                let code = entry_code_run.clone();
                                st_run.update(cx, |st, cx| {
                                    st.active_preset = None;
                                    st.selected_saved = Some(name.clone());
                                    st.script_name.update(cx, |inp, cx| {
                                        inp.set_value(name, window, cx);
                                    });
                                    st.script_editor.update(cx, |ed, cx| {
                                        ed.set_value(code.clone(), window, cx);
                                    });
                                });
                                this.execute_custom_action_into_ui(&code, &st_run, window, cx);
                            })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "repeater-custom-action-saved-del-{idx}"
                            )))
                            .ghost()
                            .small()
                            .compact()
                            .w(theme::scaled_px(20.))
                            .h(theme::scaled_px(18.))
                            .icon(IconName::Close)
                            .accessibility_label("删除已保存脚本")
                            .tooltip("从我的自定义脚本中删除")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let name = entry_name_del.clone();
                                let lib_path = custom_actions_library_path().ok().flatten();
                                let current = st_del.read(cx).saved_scripts.clone();
                                match delete_custom_action_from_path(
                                    lib_path.as_deref(),
                                    &current,
                                    &name,
                                ) {
                                    Ok(next) => {
                                        st_del.update(cx, |st, _| {
                                            st.saved_scripts = next;
                                            if st.selected_saved.as_deref() == Some(&name) {
                                                st.selected_saved = None;
                                            }
                                            st.error = None;
                                            st.saved_notice =
                                                Some(format!("已删除自定义脚本：{name}"));
                                        });
                                        this.status = format!("已删除自定义脚本：{name}");
                                    }
                                    Err(err) => {
                                        st_del.update(cx, |st, _| {
                                            st.saved_notice = None;
                                            st.error = Some(format!("{err:#}"));
                                        });
                                    }
                                }
                                cx.notify();
                            })),
                        ),
                );
            }
            saved_section = saved_section.child(list);
        }
        saved_section.into_any_element()
    }

    #[inline(never)]
    fn render_custom_actions_name_bar(
        &self,
        ui_state: &Entity<CustomActionsUiState>,
        script_name: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state_for_save = ui_state.clone();
        div()
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(script_name)
                        .id("repeater-custom-action-name")
                        .aria_label("自定义脚本名称")
                        .small(),
                ),
            )
            .child(
                button("repeater-custom-action-save", "保存脚本")
                    .icon(IconName::Save)
                    .tooltip("校验并保存到本地「我的自定义脚本」(repeater-custom-actions.json)")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let (name_entity, editor_entity, current_scripts) = {
                            let st = state_for_save.read(cx);
                            (
                                st.script_name.clone(),
                                st.script_editor.clone(),
                                st.saved_scripts.clone(),
                            )
                        };
                        let name_res =
                            editing::input_snapshot(&name_entity, window, cx).map(|s| s.to_string());
                        let code_res =
                            editing::snapshot(&editor_entity, window, cx).map(|s| s.to_string());
                        let lib_path = custom_actions_library_path().ok().flatten();
                        let result = name_res
                            .map_err(|e| anyhow::anyhow!(e))
                            .and_then(|name| {
                                let code = code_res.map_err(|e| anyhow::anyhow!(e))?;
                                save_custom_action_to_path(
                                    lib_path.as_deref(),
                                    &current_scripts,
                                    &name,
                                    &code,
                                )
                                .map(|next| (name.trim().to_owned(), next))
                            });
                        match result {
                            Ok((name, next)) => {
                                let notice = format!("已保存自定义脚本：{name}");
                                state_for_save.update(cx, |st, _| {
                                    st.active_preset = None;
                                    st.selected_saved = Some(name);
                                    st.saved_scripts = next;
                                    st.error = None;
                                    st.saved_notice = Some(notice.clone());
                                });
                                this.error = None;
                                this.status = notice;
                            }
                            Err(err) => {
                                state_for_save.update(cx, |st, _| {
                                    st.saved_notice = None;
                                    st.error = Some(format!("{err:#}"));
                                });
                            }
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_custom_actions_run_bar(
        &self,
        ui_state: &Entity<CustomActionsUiState>,
        has_http_tab: bool,
        has_output: bool,
        has_error: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state_for_run = ui_state.clone();
        let state_for_copy = ui_state.clone();
        let state_for_clear = ui_state.clone();
        div()
            .px(px(8.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::toolbar_surface())
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                button("repeater-custom-action-run", "运行 (Run)")
                    .primary()
                    .icon(IconName::Play)
                    .disabled(!has_http_tab)
                    .tooltip("针对当前激活的 Repeater 请求与响应执行上方脚本（如修改 request 将直接更新当前请求）")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let editor_entity = state_for_run.read(cx).script_editor.clone();
                        let script = match editing::snapshot(&editor_entity, window, cx) {
                            Ok(s) => s.to_string(),
                            Err(err) => {
                                state_for_run.update(cx, |st, _| {
                                    st.error = Some(err.to_owned());
                                });
                                cx.notify();
                                return;
                            }
                        };
                        this.execute_custom_action_into_ui(&script, &state_for_run, window, cx);
                    })),
            )
            .child(
                button("repeater-custom-action-copy-output", "复制输出")
                    .icon(IconName::Copy)
                    .disabled(!has_output)
                    .tooltip("将自定义动作脚本的输出结果复制到剪贴板")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let text = state_for_copy.read(cx).output.clone();
                        if !text.is_empty() {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                            this.status = "已复制 Custom actions 输出到剪贴板".into();
                            cx.notify();
                        }
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("repeater-custom-action-clear")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(22.))
                    .px(px(6.))
                    .label("清空")
                    .disabled(!has_output && !has_error)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        state_for_clear.update(cx, |st, _| {
                            st.output.clear();
                            st.error = None;
                            st.saved_notice = None;
                        });
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_repeater_custom_actions_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui_state = window.use_keyed_state("repeater-custom-actions-ui", cx, |window, cx| {
            let script_editor = cx.new(|ecx| {
                let mut ed = EditorState::new(window, ecx)
                    .language("javascript")
                    .line_number(true)
                    .soft_wrap(true)
                    .auto_close(false)
                    .autocorrect(false);
                ed.set_value(PRESET_REMOVE_COOKIE, window, ecx);
                ed
            });
            let script_name = cx.new(|icx| {
                InputState::new(window, icx)
                    .default_value("去除 Cookie")
                    .placeholder("自定义脚本名称（保存到本地脚本库）")
            });
            let saved_scripts = custom_actions_library_path()
                .ok()
                .flatten()
                .and_then(|path| read_custom_action_library_from_path(&path).ok())
                .unwrap_or_default();
            CustomActionsUiState {
                script_editor,
                script_name,
                active_preset: Some(0),
                selected_saved: None,
                saved_scripts,
                output: String::new(),
                error: None,
                saved_notice: None,
            }
        });

        let (
            script_editor,
            script_name,
            active_preset,
            selected_saved,
            saved_scripts,
            output,
            error,
            saved_notice,
        ) = {
            let st = ui_state.read(cx);
            (
                st.script_editor.clone(),
                st.script_name.clone(),
                st.active_preset,
                st.selected_saved.clone(),
                st.saved_scripts.clone(),
                st.output.clone(),
                st.error.clone(),
                st.saved_notice.clone(),
            )
        };
        let has_http_tab = self
            .workspace
            .active
            .is_some_and(|id| self.tab_editors.contains_key(&id));
        let dock_left = self.inspector.dock_left;
        let header = self.render_custom_actions_header(&ui_state, cx);
        let presets = div()
            .id("repeater-custom-actions-presets-pane")
            .max_h(px(164.))
            .flex_shrink_0()
            .overflow_y_scroll()
            .p(px(8.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.render_custom_actions_preset_group(
                "修改当前请求 (Rhai 原地修改草稿 · 点 ▶ 立即执行)：",
                CustomActionCategory::MutateRequest,
                &ui_state,
                active_preset,
                has_http_tab,
                cx,
            ))
            .child(self.render_custom_actions_preset_group(
                "分析与提取 (解析请求/响应并输出到下方)：",
                CustomActionCategory::Inspect,
                &ui_state,
                active_preset,
                has_http_tab,
                cx,
            ));
        let saved_section = self.render_custom_actions_saved_section(
            &ui_state,
            &saved_scripts,
            selected_saved.as_deref(),
            has_http_tab,
            cx,
        );
        let name_bar = self.render_custom_actions_name_bar(&ui_state, &script_name, cx);
        let run_bar = self.render_custom_actions_run_bar(
            &ui_state,
            has_http_tab,
            !output.is_empty(),
            error.is_some(),
            cx,
        );

        div()
            .id("repeater-custom-actions-sidebar")
            .test_support()
            .w(px(352.))
            .flex_shrink_0()
            .h_full()
            .when(dock_left, |d| d.border_r_1())
            .when(!dock_left, |d| d.border_l_1())
            .border_color(theme::border())
            .bg(theme::surface())
            .flex()
            .flex_col()
            .child(header)
            .child(presets)
            .child(saved_section)
            .child(name_bar)
            .child(
                div()
                    .h(px(155.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        Editor::new(&script_editor)
                            .size_full()
                            .bordered(false)
                            .aria_label("Custom actions Rhai 脚本编辑器"),
                    ),
            )
            .child(run_bar)
            .when_some(error, |col, err| {
                col.child(
                    div()
                        .id("repeater-custom-action-error")
                        .test_support()
                        .px(px(8.))
                        .py(px(5.))
                        .bg(theme::danger_bg())
                        .border_b_1()
                        .border_color(theme::border())
                        .text_size(theme::scaled_px(11.5))
                        .text_color(theme::danger_fg())
                        .child(err),
                )
            })
            .when_some(saved_notice, |col, msg| {
                col.child(
                    div()
                        .id("repeater-custom-action-saved")
                        .test_support()
                        .px(px(8.))
                        .py(px(4.))
                        .border_b_1()
                        .border_color(theme::border())
                        .text_size(theme::scaled_px(11.5))
                        .text_color(rgb(if theme::is_dark() { 0x7ee787 } else { 0x137333 }))
                        .child(msg),
                )
            })
            .child(
                div()
                    .id("repeater-custom-action-output")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(8.))
                    .bg(theme::background())
                    .font_family(theme::mono_font_family())
                    .text_size(theme::scaled_px(11.5))
                    .text_color(if output.is_empty() {
                        theme::muted_foreground()
                    } else {
                        theme::foreground()
                    })
                    .child(if output.is_empty() {
                        if has_http_tab {
                            "点击模板「▶」或下方「运行 (Run)」执行 Rhai 脚本；若修改 request（如去除 Cookie）将同步更新当前请求草稿。"
                                .to_owned()
                        } else {
                            "请先激活一个 HTTP Repeater 标签页以运行 Custom actions 脚本。"
                                .to_owned()
                        }
                    } else {
                        output
                    }),
            )
            .into_any_element()
    }

    fn execute_custom_action_into_ui(
        &mut self,
        script: &str,
        ui_state: &Entity<CustomActionsUiState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.run_custom_action_on_active_tab(script, window, cx) {
            Ok((out, mutated)) => {
                ui_state.update(cx, |st, _| {
                    st.output = out;
                    st.error = None;
                    st.saved_notice = None;
                });
                self.status = if mutated {
                    "已应用 Custom action 并更新当前 Repeater 请求".into()
                } else {
                    "Repeater Custom actions 脚本执行完成".into()
                };
            }
            Err(err) => {
                ui_state.update(cx, |st, _| {
                    st.error = Some(err);
                    st.saved_notice = None;
                });
            }
        }
        cx.notify();
    }

    fn run_custom_action_on_active_tab(
        &mut self,
        script: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(String, bool), String> {
        let tab_id = self
            .workspace
            .active
            .ok_or_else(|| "当前没有激活的 HTTP Repeater 标签页。".to_owned())?;
        let editors = self
            .tab_editors
            .get(&tab_id)
            .ok_or_else(|| "当前 Repeater 编辑器尚未就绪。".to_owned())?;
        let tab_meta = self.workspace.tabs.iter().find(|t| t.id == tab_id);
        let raw_req = editors
            .request
            .bytes(window, cx)
            .map_err(|e| format!("读取请求草稿失败：{e}"))?;
        let target = editing::input_snapshot(&editors.target, window, cx)
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|_| tab_meta.map_or_else(String::new, |t| t.draft.target.clone()));
        let effective_target = if target.is_empty() {
            fallback_target_from_raw(&raw_req)
        } else {
            target.clone()
        };
        let fallback_ver = tab_meta.map_or(HttpVersion::Http11, |t| t.draft.version);
        let req_version =
            mio_core::detect_request_line_version(&raw_req).unwrap_or(fallback_ver);
        let parsed_req = mio_core::parse_request(&raw_req, &effective_target, req_version)
            .map_err(|err| format!("解析当前 HTTP 请求失败：{err:#}"))?;

        let raw_resp = editors.response.bytes_lossless(cx);
        let parsed_resp = if !raw_resp.is_empty() {
            let resp_ver = detect_response_line_version(&raw_resp, req_version);
            mio_core::parse_response(&raw_resp, resp_ver).ok()
        } else {
            editors
                .loaded_flow
                .as_ref()
                .and_then(|flow| flow.response.clone())
        };
        let outcome = mio_core::eval_custom_action_with_mutation(
            script,
            &parsed_req,
            parsed_resp.as_ref(),
        )
        .map_err(|err| format!("{err:#}"))?;
        let mutated = outcome.updated_request.is_some();
        if let Some(updated) = outcome.updated_request {
            let next_raw = mio_core::request_to_raw(&updated);
            if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                let was_pretty = editors.request.pretty_active();
                let prev_view = editors.request.view();
                editors.request.load(&next_raw, window, cx);
                if was_pretty {
                    let _ = editors.request.switch_pretty(window, cx);
                } else if prev_view == crate::editing::MessageView::Hex
                    && editors.request.view() != crate::editing::MessageView::Hex
                {
                    let _ = editors
                        .request
                        .switch(crate::editing::MessageView::Hex, window, cx);
                }
                editors.dirty = true;
            }
            if target.is_empty()
                && let Ok(parsed_url) = url::Url::parse(&updated.url)
                && let Some(host) = parsed_url.host_str()
            {
                let mut next_origin = format!("{}://{host}", parsed_url.scheme());
                if let Some(port) = parsed_url.port() {
                    next_origin.push_str(&format!(":{port}"));
                }
                if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                    editors
                        .target
                        .update(cx, |inp, cx| inp.set_value(next_origin, window, cx));
                }
            }
        }
        Ok((outcome.output, mutated))
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_repeater_explanations_sidebar(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dock_left = self.inspector.dock_left;
        let cache_key = if let Some(tab_id) = self.workspace.active
            && let Some(editors) = self.tab_editors.get(&tab_id)
        {
            let tab_meta = self.workspace.tabs.iter().find(|t| t.id == tab_id);
            ExplanationCacheKey::Http {
                tab_id,
                title: tab_meta.map_or("Repeater", |t| t.title.as_str()).to_owned(),
                target_id: editors.target.entity_id(),
                target_rev: editors.target.read(cx).document_revision(),
                req_rev: crate::workbench::inspector::message_editor_rev_key(&editors.request, cx),
                resp_rev: crate::workbench::inspector::message_editor_rev_key(
                    &editors.response,
                    cx,
                ),
                fallback_ver: tab_meta.map_or(HttpVersion::Http11, |t| t.draft.version),
                loaded_flow: editors
                    .loaded_flow
                    .as_ref()
                    .map(|f| (f.summary.id, f.summary.revision, f.response.is_some())),
            }
        } else if let Some(ws_id) = self.workspace.active_websocket_tab
            && let Some(ws_tab) = self
                .workspace
                .websocket_tabs
                .iter()
                .find(|t| !t.closed && t.id == ws_id)
        {
            ExplanationCacheKey::WebSocket {
                ws_id,
                title: ws_tab.title.clone(),
                url: ws_tab.url.clone(),
                direction: ws_tab.direction,
                draft_kind: ws_tab.draft_kind,
                headers_len: ws_tab.handshake_headers.len(),
                history_len: ws_tab.history.len(),
                draft_revision: ws_tab.draft_revision,
                draft_rev: self
                    .websocket_repeater
                    .editors
                    .get(&ws_id)
                    .map(|ed| crate::workbench::inspector::message_editor_rev_key(&ed.draft, cx)),
            }
        } else {
            ExplanationCacheKey::Empty
        };

        let cached_report = self
            .repeater_ui_caches
            .explanation_cache
            .borrow()
            .as_ref()
            .filter(|(k, _)| *k == cache_key)
            .map(|(_, report)| std::rc::Rc::clone(report));

        let report: std::rc::Rc<RepeaterExplanationReport> = if let Some(hit) = cached_report {
            hit
        } else {
            let computed = if let Some(tab_id) = self.workspace.active
                && let Some(editors) = self.tab_editors.get(&tab_id)
            {
                let tab_meta = self.workspace.tabs.iter().find(|t| t.id == tab_id);
                let title = tab_meta.map_or("Repeater", |t| t.title.as_str());
                let target = editors.target.read(cx).value().trim().to_owned();
                let raw_req = editors.request.bytes_lossless(cx);
                let raw_resp = editors.response.bytes_lossless(cx);
                let effective_target = if target.is_empty() {
                    fallback_target_from_raw(&raw_req)
                } else {
                    target.clone()
                };
                let fallback_ver = tab_meta.map_or(HttpVersion::Http11, |t| t.draft.version);
                let req_ver =
                    mio_core::detect_request_line_version(&raw_req).unwrap_or(fallback_ver);
                let parsed_req = mio_core::parse_request(&raw_req, &effective_target, req_ver).ok();
                let parsed_resp = if !raw_resp.is_empty() {
                    let resp_ver = detect_response_line_version(&raw_resp, req_ver);
                    mio_core::parse_response(&raw_resp, resp_ver).ok()
                } else {
                    editors
                        .loaded_flow
                        .as_ref()
                        .and_then(|f| f.response.clone())
                };
                analyze_http_explanation(
                    title,
                    &target,
                    &raw_req,
                    parsed_req.as_ref(),
                    &raw_resp,
                    parsed_resp.as_ref(),
                )
            } else if let Some(ws_id) = self.workspace.active_websocket_tab
                && let Some(ws_tab) = self
                    .workspace
                    .websocket_tabs
                    .iter()
                    .find(|t| !t.closed && t.id == ws_id)
            {
                let draft_bytes = self
                    .websocket_repeater
                    .editors
                    .get(&ws_id)
                    .map(|ed| ed.draft.bytes_lossless(cx))
                    .unwrap_or_else(|| ws_tab.draft_payload.clone());
                let ai_context = format!(
                    "[WebSocket Repeater 标签] {}\n[URL] {}\n[方向] {:?}\n[握手标头数] {}\n[历史帧数] {}\n\n=== Draft Payload ===\n{}",
                    ws_tab.title,
                    ws_tab.url,
                    ws_tab.direction,
                    ws_tab.handshake_headers.len(),
                    ws_tab.history.len(),
                    truncate_for_ai_context(&draft_bytes, 8192)
                );
                RepeaterExplanationReport {
                    title: ws_tab.title.clone(),
                    overview: vec![
                        ("协议", "WebSocket".into()),
                        ("目标 URL", ws_tab.url.clone()),
                        ("方向", format!("{:?}", ws_tab.direction)),
                        ("帧类型", format!("{:?}", ws_tab.draft_kind)),
                        ("握手标头", format!("{} 项", ws_tab.handshake_headers.len())),
                        ("草稿大小", format!("{} B", draft_bytes.len())),
                    ],
                    auth_items: vec![ExplanationItem {
                        badge: if ws_tab.url.starts_with("wss://") {
                            "TLS"
                        } else {
                            "明文"
                        },
                        severity: if ws_tab.url.starts_with("wss://") {
                            ExplanationSeverity::Ok
                        } else {
                            ExplanationSeverity::Warn
                        },
                        title: if ws_tab.url.starts_with("wss://") {
                            "使用加密 wss:// 传输".into()
                        } else {
                            "使用明文 ws:// 传输".into()
                        },
                        detail: format!("会话历史已记录 {} 帧消息。", ws_tab.history.len()),
                        mono: false,
                    }],
                    response_summary: None,
                    cookie_items: Vec::new(),
                    security_header_items: Vec::new(),
                    ai_context,
                }
            } else {
                RepeaterExplanationReport {
                    title: "Repeater".into(),
                    overview: vec![("状态", "当前无激活的 Repeater 标签页".into())],
                    auth_items: Vec::new(),
                    response_summary: None,
                    cookie_items: Vec::new(),
                    security_header_items: Vec::new(),
                    ai_context: String::new(),
                }
            };
            let rc = std::rc::Rc::new(computed);
            *self.repeater_ui_caches.explanation_cache.borrow_mut() =
                Some((cache_key, std::rc::Rc::clone(&rc)));
            rc
        };

        let card_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x222428 } else { 0xf8f9fa }).into();
        let mut overview_card = div()
            .w_full()
            .p(px(8.))
            .rounded(px(4.))
            .bg(card_bg)
            .border_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(theme::foreground())
                    .child("请求概览 (Request Overview)"),
            );
        for (k, v) in &report.overview {
            overview_card = overview_card.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .text_size(theme::scaled_px(11.5))
                    .child(
                        div()
                            .w(px(68.))
                            .flex_shrink_0()
                            .text_color(theme::muted_foreground())
                            .child(*k),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme::foreground())
                            .child(v.clone()),
                    ),
            );
        }

        let mut body = div()
            .id("repeater-explanations-body")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(overview_card);

        if !report.auth_items.is_empty() {
            body = body.child(render_explanation_card(
                "认证与会话分析 (Auth & Session)",
                &report.auth_items,
            ));
        }
        if let Some(resp_sum) = report.response_summary.as_ref() {
            body = body.child(
                div()
                    .w_full()
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(4.))
                    .bg(card_bg)
                    .border_1()
                    .border_color(theme::border())
                    .text_size(theme::scaled_px(11.5))
                    .text_color(theme::foreground())
                    .child(resp_sum.clone()),
            );
            if !report.cookie_items.is_empty() {
                body = body.child(render_explanation_card(
                    "Set-Cookie 安全属性审计",
                    &report.cookie_items,
                ));
            }
            if !report.security_header_items.is_empty() {
                body = body.child(render_explanation_card(
                    "CORS 与安全响应头审计",
                    &report.security_header_items,
                ));
            }
        } else {
            body = body.child(
                div()
                    .w_full()
                    .p(px(8.))
                    .rounded(px(4.))
                    .bg(card_bg)
                    .border_1()
                    .border_color(theme::border())
                    .text_size(theme::scaled_px(11.5))
                    .text_color(theme::muted_foreground())
                    .child("尚未收到 HTTP 响应；发送请求后将自动审计 Set-Cookie、CORS、CSP、HSTS 与安全响应头。"),
            );
        }

        let send_title = report.title.clone();
        let send_ctx = report.ai_context.clone();
        let can_send_ai = !send_ctx.is_empty();

        div()
            .id("repeater-explanations-sidebar")
            .test_support()
            .w(px(340.))
            .flex_shrink_0()
            .h_full()
            .when(dock_left, |d| d.border_r_1())
            .when(!dock_left, |d| d.border_l_1())
            .border_color(theme::border())
            .bg(theme::surface())
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .flex_nowrap()
                    .gap(px(5.))
                    .px(px(8.))
                    .child(
                        gpui_kit::component::Icon::new(IconName::Info)
                            .size(theme::scaled_px(13.))
                            .text_color(theme::accent()),
                    )
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Explanations"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("repeater-explanations-close")
                            .ghost()
                            .small()
                            .compact()
                            .w(theme::scaled_px(24.))
                            .h(theme::scaled_px(24.))
                            .icon(IconName::Close)
                            .accessibility_label("关闭 Explanations")
                            .tooltip("收起 Repeater Explanations 离线安全分析侧栏")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.repeater_explanations_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(body)
            .child(
                div()
                    .p(px(8.))
                    .border_t_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .child(
                        Button::new("repeater-explanations-send-mio-ai")
                            .primary()
                            .small()
                            .rounded(ButtonRounded::Size(px(3.)))
                            .w_full()
                            .h(px(26.))
                            .bg(rgb(0x4946ae))
                            .text_color(rgb(0xffffff))
                            .disabled(!can_send_ai)
                            .icon(IconName::Sparkles)
                            .label("发送到 MioAT 深入分析")
                            .tooltip("携带当前报文与离线审计摘要打开 MioAT / Agent IPC 控制中心")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_mio_ai_with_context(
                                    send_title.clone(),
                                    send_ctx.clone(),
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod custom_actions_tests {
    use super::*;
    use crate::workbench::tests::{drain, fixture, tab};
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;
    use mio_core::{Header, HttpRequest, HttpResponse, HttpVersion};

    #[test]
    fn all_custom_action_presets_execute_cleanly_against_sample_http_exchange() {
        let req = HttpRequest {
            method: "POST".into(),
            url: "https://app.example/api/v1/account?id=7".into(),
            version: HttpVersion::Http11,
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"app.example".to_vec(),
                },
                Header {
                    name: "Cookie".into(),
                    value: b"sid=abc; jwt=eyJhbGciOiJIUzI1NiJ9.eyJ1c2VyIjoiYWxpY2UifQ.sig".to_vec(),
                },
                Header {
                    name: "Authorization".into(),
                    value: b"Bearer eyJhbGciOiJIUzI1NiJ9.eyJyb2xlIjoiYWRtaW4ifQ.sig".to_vec(),
                },
                Header {
                    name: "If-None-Match".into(),
                    value: b"\"v1\"".to_vec(),
                },
                Header {
                    name: "Sec-Fetch-Site".into(),
                    value: b"same-origin".to_vec(),
                },
                Header {
                    name: "Content-Type".into(),
                    value: b"application/json".to_vec(),
                },
            ],
            body: br#"{"ok":true}"#.to_vec(),
            trailers: Vec::new(),
        };
        let resp = HttpResponse {
            version: HttpVersion::Http11,
            status: 200,
            reason_phrase: Some(b"OK".to_vec()),
            headers: vec![
                Header {
                    name: "Content-Type".into(),
                    value: b"text/html".to_vec(),
                },
                Header {
                    name: "Location".into(),
                    value: b"https://app.example/next".to_vec(),
                },
                Header {
                    name: "Set-Cookie".into(),
                    value: b"prefs=1; Path=/; HttpOnly".to_vec(),
                },
                Header {
                    name: "Access-Control-Allow-Origin".into(),
                    value: b"*".to_vec(),
                },
            ],
            body: br#"<a href="/admin">Admin</a><script src="https://cdn.example/app.js"></script>{"api_key":"demo_secret_token"}"#.to_vec(),
            trailers: Vec::new(),
        };

        for preset in CUSTOM_ACTION_PRESETS {
            mio_core::validate_filter_script(preset.script)
                .unwrap_or_else(|e| panic!("preset {} invalid syntax: {e}", preset.label));
            let outcome =
                mio_core::eval_custom_action_with_mutation(preset.script, &req, Some(&resp))
                    .unwrap_or_else(|e| panic!("preset {} failed: {e}", preset.label));
            assert!(
                !outcome.output.trim().is_empty(),
                "preset {} should produce output",
                preset.label
            );
            match preset.category {
                CustomActionCategory::MutateRequest => {
                    assert!(
                        outcome.updated_request.is_some(),
                        "mutating preset {} should modify request",
                        preset.label
                    );
                }
                CustomActionCategory::Inspect => {
                    assert!(
                        outcome.updated_request.is_none(),
                        "inspect preset {} must not mutate request",
                        preset.label
                    );
                }
            }
        }
    }

    #[gpui_kit::test]
    fn repeater_custom_actions_sidebar_strips_cookie_and_persists_saved_scripts(
        cx: &mut TestAppContext,
    ) {
        struct ResetPath;
        impl Drop for ResetPath {
            fn drop(&mut self) {
                TEST_CUSTOM_ACTIONS_PATH.with(|cell| *cell.borrow_mut() = None);
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let lib_path = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("repeater-custom-actions.json");
        TEST_CUSTOM_ACTIONS_PATH.with(|cell| *cell.borrow_mut() = Some(lib_path.clone()));
        let _reset = ResetPath;

        let mut wt = tab("CustomActions");
        wt.draft.target = "https://target.example".into();
        wt.draft.raw = b"GET /profile HTTP/1.1\r\nHost: target.example\r\nCookie: sid=xyz; role=user\r\nUser-Agent: Test\r\n\r\n".to_vec();
        let tab_id = wt.id;

        let mut f = fixture(cx, vec![wt], 64);
        drain(&mut f.commands);

        f.window
            .update(cx, |_, window, cx| {
                f.view.update(cx, |view, cx| {
                    view.tool = Tool::Repeater;
                    view.repeater_custom_actions_open = true;
                    view.sync_tab_editors(window, cx);
                });
                window.render_frame(cx);
                assert!(window.find("repeater-custom-actions-sidebar").visible());
                window.click("repeater-custom-action-preset-remove-cookie-run", cx);
            })
            .unwrap();
        cx.run_until_parked();

        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let view = f.view.read(cx);
                let editors = &view.tab_editors[&tab_id];
                let raw = String::from_utf8(editors.request.bytes_lossless(cx)).unwrap();
                assert!(
                    !raw.to_ascii_lowercase().contains("cookie:"),
                    "Cookie header should be removed from active Repeater draft, got:\n{raw}"
                );
                assert!(raw.contains("User-Agent: Test"));
                assert!(editors.dirty);

                window.click("repeater-custom-action-new", cx);
            })
            .unwrap();
        cx.run_until_parked();

        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.click("repeater-custom-action-name", cx);
                window.input("添加调试头", cx);
                window.click("repeater-custom-action-save", cx);
            })
            .unwrap();
        cx.run_until_parked();

        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let saved = read_custom_action_library_from_path(&lib_path).unwrap();
                assert_eq!(saved.len(), 1);
                assert_eq!(saved[0].name, "添加调试头");
                assert!(window.find("repeater-custom-actions-saved-list").visible());
                window.click("repeater-custom-action-saved-run-0", cx);
            })
            .unwrap();
        cx.run_until_parked();

        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let view = f.view.read(cx);
                let raw = String::from_utf8(view.tab_editors[&tab_id].request.bytes_lossless(cx))
                    .unwrap();
                assert!(
                    raw.contains("X-Custom-Test: mio"),
                    "custom saved script should mutate active request, got:\n{raw}"
                );
                window.click("repeater-custom-action-saved-del-0", cx);
            })
            .unwrap();
        cx.run_until_parked();

        assert!(
            read_custom_action_library_from_path(&lib_path)
                .unwrap()
                .is_empty()
        );
    }
}
