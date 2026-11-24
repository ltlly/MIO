use base64::engine::general_purpose::{STANDARD as B64_STANDARD, URL_SAFE_NO_PAD as B64_URL_NO_PAD};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::workbench::tabs::render_pane) enum ExplanationSeverity {
    Ok,
    Info,
    Warn,
}

pub(in crate::workbench::tabs::render_pane) struct ExplanationItem {
    badge: &'static str,
    severity: ExplanationSeverity,
    title: String,
    detail: String,
    mono: bool,
}

pub(in crate::workbench::tabs::render_pane) struct RepeaterExplanationReport {
    title: String,
    overview: Vec<(&'static str, String)>,
    auth_items: Vec<ExplanationItem>,
    response_summary: Option<String>,
    cookie_items: Vec<ExplanationItem>,
    security_header_items: Vec<ExplanationItem>,
    ai_context: String,
}

#[derive(PartialEq, Eq)]
pub(in crate::workbench::tabs::render_pane) enum ExplanationCacheKey {
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

pub(super) fn xml_escape_text(text: &str) -> String {
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
