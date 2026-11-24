use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result as AnyResult, ensure};
use serde::{Deserialize, Serialize};

pub(super) const PRESET_REMOVE_COOKIE: &str = r#"// 移除当前请求中的 Cookie 请求头（等价于 External Custom Actions「Remove cookies」）
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

pub(super) const STARTER_CUSTOM_ACTION: &str = r#"// 自定义 Rhai 脚本示例（修改 request 后将直接更新当前草稿，print 内容输出到下方）
// 可用辅助函数：
//   remove_all_cookies(request) / remove_cookie(request, name) / set_cookie(request, name, val)
//   set_header(request, name, val) / remove_header(request, name) / remove_headers(request, [...])
//   set_query_param(request, k, v) / remove_query_param(request, k)
//   base64url_encode(s) / base64url_decode(s) / regex_matches(s, pat)
remove_all_cookies(request);
set_header(request, "X-Custom-Test", "mio");
print("已移除 Cookie 并添加 X-Custom-Test 请求头。");"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CustomActionCategory {
    MutateRequest,
    Inspect,
}

#[derive(Clone, Copy)]
pub(super) struct CustomActionPreset {
    pub(super) id: &'static str,
    pub(super) category: CustomActionCategory,
    pub(super) label: &'static str,
    pub(super) description: &'static str,
    pub(super) script: &'static str,
}

pub(super) const CUSTOM_ACTION_PRESETS: [CustomActionPreset; 14] = [
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CustomActionScriptEntry {
    pub(super) name: String,
    pub(super) code: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomActionLibraryFile {
    version: u32,
    scripts: Vec<CustomActionScriptEntry>,
}

#[cfg(test)]
std::thread_local! {
    pub(super) static TEST_CUSTOM_ACTIONS_PATH: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn custom_actions_library_path() -> AnyResult<Option<PathBuf>> {
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

pub(super) fn read_custom_action_library_from_path(path: &Path) -> AnyResult<Vec<CustomActionScriptEntry>> {
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

pub(super) fn save_custom_action_to_path(
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

pub(super) fn delete_custom_action_from_path(
    path: Option<&Path>,
    current: &[CustomActionScriptEntry],
    name: &str,
) -> AnyResult<Vec<CustomActionScriptEntry>> {
    mutate_custom_action_library_at(path, current, |scripts| {
        scripts.retain(|entry| entry.name != name);
        Ok(())
    })
}
