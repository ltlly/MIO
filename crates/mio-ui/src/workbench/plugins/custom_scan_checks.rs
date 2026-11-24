use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::ButtonVariants,
    checkbox::Checkbox,
    input::{Textarea, TextareaState},
};
use mio_core::{
    AuditConfidence, AuditSeverity, CustomScanCheck, Header, HttpRequest, HttpResponse,
    HttpVersion, Uuid,
};

use super::render_suite::mio_neutral_button;
use super::*;

const MAX_CUSTOM_SCAN_CHECKS: usize = 256;
const DEFAULT_SAMPLE_URL: &str = "https://example.com/api/debug?env=staging";
const DEFAULT_SAMPLE_RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Powered-By: DebugServer/1.0\r\n\r\n{\"status\":\"ok\",\"debug\":\"X-Debug-Secret=demo\"}";
const DEFAULT_CHECK_SCRIPT: &str = "if response.status == 200 && response.body.contains(\"X-Debug-Secret\") {\n    \"响应正文泄露调试凭据 (X-Debug-Secret)\"\n} else {\n    false\n}";

pub(super) const CUSTOM_SCAN_CHECK_TEMPLATES: [(
    &str,
    &str,
    &str,
    AuditSeverity,
    AuditConfidence,
    &str,
); 5] = [
    (
        "调试凭据与堆栈泄露检测",
        "调试凭据/堆栈",
        "检测响应正文中是否包含调试密钥或未处理异常堆栈信息",
        AuditSeverity::Medium,
        AuditConfidence::Firm,
        DEFAULT_CHECK_SCRIPT,
    ),
    (
        "敏感环境配置或 Git 泄露",
        "环境/源码泄露",
        "检测 .env / .git / 数据库口令与 APP_KEY 等敏感配置暴露",
        AuditSeverity::High,
        AuditConfidence::Firm,
        "if response.status == 200 && (response.body.contains(\"DB_PASSWORD=\") || response.body.contains(\"APP_KEY=base64:\") || response.body.contains(\"ref: refs/heads/\")) {\n    \"响应暴露敏感环境配置或源码仓库元数据\"\n} else {\n    false\n}",
    ),
    (
        "高危 CORS 通配与凭据并存",
        "CORS 高危配置",
        "检测响应同时返回较宽松 Access-Control-Allow-Origin 与 Allow-Credentials: true",
        AuditSeverity::High,
        AuditConfidence::Certain,
        "let acao = header_value(response, \"Access-Control-Allow-Origin\");\nlet acac = header_value(response, \"Access-Control-Allow-Credentials\");\nacac.to_lower();\nif acac == \"true\" && (acao == \"*\" || acao == \"null\" || acao.contains(\"evil\")) {\n    \"CORS 响应允许跨站凭据访问 (ACA-Origin=\" + acao + \")\"\n} else {\n    false\n}",
    ),
    (
        "Set-Cookie 缺少 HttpOnly/Secure 标志",
        "Cookie 安全标志",
        "检测 Set-Cookie 响应头是否缺少 HttpOnly 或 Secure 防护属性",
        AuditSeverity::Low,
        AuditConfidence::Certain,
        "let sc = header_value(response, \"Set-Cookie\");\nlet lower = sc;\nlower.to_lower();\nif sc != \"\" && (!lower.contains(\"httponly\") || !lower.contains(\"secure\")) {\n    \"Set-Cookie 缺少 HttpOnly 或 Secure 属性：\" + sc\n} else {\n    false\n}",
    ),
    (
        "服务端详细版本指纹暴露",
        "版本指纹暴露",
        "检测 X-Powered-By / Server 响应头暴露具体组件版本号",
        AuditSeverity::Information,
        AuditConfidence::Firm,
        "let xpb = header_value(response, \"X-Powered-By\");\nlet srv = header_value(response, \"Server\");\nif xpb != \"\" {\n    \"响应头暴露 X-Powered-By: \" + xpb\n} else if regex_matches(srv, \"/[0-9]+\\\\.[0-9]+\") {\n    \"Server 响应头包含具体版本号：\" + srv\n} else {\n    false\n}",
    ),
];

pub(super) struct CustomScanChecksUi {
    pub(super) selected_id: Option<Uuid>,
    pub(super) name_input: Entity<InputState>,
    pub(super) description_input: Entity<InputState>,
    pub(super) script_editor: Entity<TextareaState>,
    pub(super) enabled: bool,
    pub(super) severity: AuditSeverity,
    pub(super) confidence: AuditConfidence,
    pub(super) feedback: Option<(bool, String)>,
    pub(super) test_output: Option<(bool, String)>,
    pub(super) sample_open: bool,
    pub(super) sample_url_input: Entity<InputState>,
    pub(super) sample_response_editor: Entity<TextareaState>,
}

impl CustomScanChecksUi {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        let name_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("检查名称（如：调试令牌泄露检测）"));
        let description_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("问题背景说明与修复建议（可选）"));
        let script_editor = cx.new(|cx| {
            let mut st = TextareaState::new(window, cx)
                .placeholder(
                    "Rhai 检查脚本（可用变量：request、response、requestResponse），返回 true / 非空证据字符串 / #{ matched: true, evidence: \"...\" } 表示命中",
                )
                .soft_wrap(true)
                .autocorrect(false);
            st.set_value(DEFAULT_CHECK_SCRIPT, window, cx);
            st
        });
        let sample_url_input = cx.new(|cx| {
            let mut st = InputState::new(window, cx).placeholder("测试用样本请求 URL");
            st.set_value(DEFAULT_SAMPLE_URL, window, cx);
            st
        });
        let sample_response_editor = cx.new(|cx| {
            let mut st = TextareaState::new(window, cx)
                .placeholder("测试用样本原始 HTTP 响应")
                .soft_wrap(true)
                .autocorrect(false);
            st.set_value(DEFAULT_SAMPLE_RESPONSE, window, cx);
            st
        });
        Self {
            selected_id: None,
            name_input,
            description_input,
            script_editor,
            enabled: true,
            severity: AuditSeverity::Medium,
            confidence: AuditConfidence::Firm,
            feedback: None,
            test_output: None,
            sample_open: false,
            sample_url_input,
            sample_response_editor,
        }
    }

    pub(super) fn reset_selection(&mut self) {
        self.selected_id = None;
        self.feedback = None;
        self.test_output = None;
    }
}

fn severity_label(sev: AuditSeverity) -> &'static str {
    match sev {
        AuditSeverity::Information => "信息 (Info)",
        AuditSeverity::Low => "低危 (Low)",
        AuditSeverity::Medium => "中危 (Medium)",
        AuditSeverity::High => "高危 (High)",
    }
}

fn severity_short_label(sev: AuditSeverity) -> &'static str {
    match sev {
        AuditSeverity::Information => "Information",
        AuditSeverity::Low => "Low",
        AuditSeverity::Medium => "Medium",
        AuditSeverity::High => "High",
    }
}

fn confidence_label(conf: AuditConfidence) -> &'static str {
    match conf {
        AuditConfidence::Tentative => "疑似 (Tentative)",
        AuditConfidence::Firm => "较确信 (Firm)",
        AuditConfidence::Certain => "确信 (Certain)",
    }
}

fn confidence_short_label(conf: AuditConfidence) -> &'static str {
    match conf {
        AuditConfidence::Tentative => "Tentative",
        AuditConfidence::Firm => "Firm",
        AuditConfidence::Certain => "Certain",
    }
}

fn build_sample_http_pair(sample_url: &str, sample_response_raw: &str) -> (HttpRequest, HttpResponse) {
    let url = if sample_url.trim().is_empty() {
        DEFAULT_SAMPLE_URL.to_owned()
    } else {
        sample_url.trim().to_owned()
    };
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "example.com".to_owned());
    let req = HttpRequest {
        method: "GET".into(),
        url,
        version: HttpVersion::Http11,
        headers: vec![
            Header {
                name: "Host".into(),
                value: host.into_bytes(),
            },
            Header {
                name: "User-Agent".into(),
                value: b"MIO-CustomScanCheck-Test/1.0".to_vec(),
            },
        ],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    let normalized = if sample_response_raw.contains("\r\n") {
        sample_response_raw.to_owned()
    } else {
        sample_response_raw.replace('\n', "\r\n")
    };
    let resp = mio_core::parse_response(normalized.as_bytes(), HttpVersion::Http11).unwrap_or_else(
        |_| HttpResponse {
            version: HttpVersion::Http11,
            status: 200,
            reason_phrase: Some("OK".into()),
            headers: vec![Header {
                name: "Content-Type".into(),
                value: b"text/plain".to_vec(),
            }],
            body: sample_response_raw.as_bytes().to_vec(),
            trailers: Vec::new(),
        },
    );
    (req, resp)
}

impl Workbench {
    pub(in crate::workbench) fn new_custom_scan_check(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extensions.custom_checks.selected_id = None;
        self.extensions.custom_checks.enabled = true;
        self.extensions.custom_checks.severity = AuditSeverity::Medium;
        self.extensions.custom_checks.confidence = AuditConfidence::Firm;
        self.extensions.custom_checks.feedback = None;
        self.extensions.custom_checks.test_output = None;
        self.extensions
            .custom_checks
            .name_input
            .update(cx, |inp, cx| {
                inp.set_value("", window, cx);
                inp.focus(window, cx);
            });
        self.extensions
            .custom_checks
            .description_input
            .update(cx, |inp, cx| inp.set_value("", window, cx));
        self.extensions
            .custom_checks
            .script_editor
            .update(cx, |ed, cx| ed.set_value(DEFAULT_CHECK_SCRIPT, window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn select_custom_scan_check(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(check) = self
            .workspace
            .custom_scan_checks
            .iter()
            .find(|c| c.id == id)
            .cloned()
        else {
            return;
        };
        self.extensions.custom_checks.selected_id = Some(check.id);
        self.extensions.custom_checks.enabled = check.enabled;
        self.extensions.custom_checks.severity = check.severity;
        self.extensions.custom_checks.confidence = check.confidence;
        self.extensions.custom_checks.feedback = None;
        self.extensions.custom_checks.test_output = None;
        self.extensions
            .custom_checks
            .name_input
            .update(cx, |inp, cx| inp.set_value(check.name, window, cx));
        self.extensions
            .custom_checks
            .description_input
            .update(cx, |inp, cx| inp.set_value(check.description, window, cx));
        self.extensions
            .custom_checks
            .script_editor
            .update(cx, |ed, cx| ed.set_value(check.script, window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_custom_scan_check_enabled(
        &mut self,
        id: Uuid,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() || self.project.is_none() {
            return;
        }
        let Some(check) = self
            .workspace
            .custom_scan_checks
            .iter_mut()
            .find(|c| c.id == id)
        else {
            return;
        };
        if check.enabled == enabled {
            return;
        }
        check.enabled = enabled;
        let name = check.name.clone();
        if self.extensions.custom_checks.selected_id == Some(id) {
            self.extensions.custom_checks.enabled = enabled;
        }
        self.save_workspace(cx);
        self.error = None;
        self.status = if enabled {
            format!("已启用自定义扫描检查「{name}」（将自动在审计引擎中执行）。")
        } else {
            format!("已禁用自定义扫描检查「{name}」。")
        };
        cx.notify();
    }

    pub(in crate::workbench) fn save_custom_scan_check(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() || self.project.is_none() {
            return;
        }
        let name = match editing::input_snapshot(
            &self.extensions.custom_checks.name_input,
            window,
            cx,
        ) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => {
                self.extensions.custom_checks.feedback = Some((false, err.to_owned()));
                cx.notify();
                return;
            }
        };
        let description = match editing::input_snapshot(
            &self.extensions.custom_checks.description_input,
            window,
            cx,
        ) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => {
                self.extensions.custom_checks.feedback = Some((false, err.to_owned()));
                cx.notify();
                return;
            }
        };
        let script = match self
            .extensions
            .custom_checks
            .script_editor
            .update(cx, |ed, cx| {
                if ed.marked_text_range(window, cx).is_some() {
                    Err(editing::COMPOSITION_PENDING.to_owned())
                } else {
                    Ok(ed.content().text().to_string())
                }
            }) {
            Ok(v) => v,
            Err(err) => {
                self.extensions.custom_checks.feedback = Some((false, err));
                cx.notify();
                return;
            }
        };
        if name.is_empty() || name.len() > 256 || name.contains('\0') {
            self.extensions.custom_checks.feedback =
                Some((false, "检查名称不能为空、包含 NUL 或超过 256 字节。".to_owned()));
            cx.notify();
            return;
        }
        if description.len() > 2048 || description.contains('\0') {
            self.extensions.custom_checks.feedback =
                Some((false, "描述说明不能包含 NUL 或超过 2048 字节。".to_owned()));
            cx.notify();
            return;
        }
        if script.trim().is_empty() || script.len() > 64 * 1024 || script.contains('\0') {
            self.extensions.custom_checks.feedback = Some((
                false,
                "检查脚本不能为空、包含 NUL 或超过 64 KiB 上限。".to_owned(),
            ));
            cx.notify();
            return;
        }
        // Compile-check the Rhai script against sample traffic before persisting.
        let (sample_req, sample_resp) =
            build_sample_http_pair(DEFAULT_SAMPLE_URL, DEFAULT_SAMPLE_RESPONSE);
        if let Err(err) = mio_core::eval_custom_scan_check_script(&script, &sample_req, &sample_resp)
        {
            self.extensions.custom_checks.feedback =
                Some((false, format!("未保存，脚本编译/执行校验失败：{err}")));
            cx.notify();
            return;
        }

        let enabled = self.extensions.custom_checks.enabled;
        let severity = self.extensions.custom_checks.severity;
        let confidence = self.extensions.custom_checks.confidence;
        let editing_id = self.extensions.custom_checks.selected_id;

        if let Some(id) = editing_id
            && let Some(pos) = self
                .workspace
                .custom_scan_checks
                .iter()
                .position(|c| c.id == id)
        {
            let prev = self.workspace.custom_scan_checks[pos].clone();
            self.workspace.custom_scan_checks[pos] = CustomScanCheck {
                id,
                enabled,
                name: name.clone(),
                severity,
                confidence,
                script,
                description,
            };
            if let Err(err) = self.workspace.validate_structure() {
                self.workspace.custom_scan_checks[pos] = prev;
                self.extensions.custom_checks.feedback =
                    Some((false, format!("未保存检查：{err}")));
                cx.notify();
                return;
            }
            self.extensions.custom_checks.selected_id = Some(id);
        } else {
            if self.workspace.custom_scan_checks.len() >= MAX_CUSTOM_SCAN_CHECKS {
                self.extensions.custom_checks.feedback = Some((
                    false,
                    format!("自定义扫描检查数量已达 {MAX_CUSTOM_SCAN_CHECKS} 条上限。"),
                ));
                cx.notify();
                return;
            }
            let id = Uuid::new_v4();
            self.workspace.custom_scan_checks.push(CustomScanCheck {
                id,
                enabled,
                name: name.clone(),
                severity,
                confidence,
                script,
                description,
            });
            if let Err(err) = self.workspace.validate_structure() {
                self.workspace.custom_scan_checks.pop();
                self.extensions.custom_checks.feedback =
                    Some((false, format!("未保存检查：{err}")));
                cx.notify();
                return;
            }
            self.extensions.custom_checks.selected_id = Some(id);
        }
        self.save_workspace(cx);
        self.extensions.custom_checks.feedback = Some((
            true,
            format!("已保存自定义扫描检查「{name}」到项目工作区。"),
        ));
        self.error = None;
        self.status = format!("已保存自定义扫描检查「{name}」。");
        cx.notify();
    }

    pub(in crate::workbench) fn delete_custom_scan_check(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() || self.project.is_none() {
            return;
        }
        let Some(pos) = self
            .workspace
            .custom_scan_checks
            .iter()
            .position(|c| c.id == id)
        else {
            return;
        };
        let removed = self.workspace.custom_scan_checks.remove(pos);
        if self.extensions.custom_checks.selected_id == Some(id) {
            self.new_custom_scan_check(window, cx);
        }
        self.save_workspace(cx);
        self.extensions.custom_checks.feedback =
            Some((true, format!("已删除自定义扫描检查「{}」。", removed.name)));
        self.error = None;
        self.status = format!("已删除自定义扫描检查「{}」。", removed.name);
        cx.notify();
    }

    pub(in crate::workbench) fn test_custom_scan_check(&mut self, cx: &mut Context<Self>) {
        let script = self
            .extensions
            .custom_checks
            .script_editor
            .read(cx)
            .content()
            .text()
            .to_string();
        let sample_url = self
            .extensions
            .custom_checks
            .sample_url_input
            .read(cx)
            .value()
            .to_string();
        let sample_resp_raw = self
            .extensions
            .custom_checks
            .sample_response_editor
            .read(cx)
            .content()
            .text()
            .to_string();
        let (req, resp) = build_sample_http_pair(&sample_url, &sample_resp_raw);
        let outcome = match mio_core::eval_custom_scan_check_script(&script, &req, &resp) {
            Ok(Some(evidence)) => {
                self.status = format!("自定义检查测试命中：{evidence}");
                (true, format!("命中告警 (Hit)：{evidence}"))
            }
            Ok(None) => {
                self.status = "自定义检查测试完成：未命中告警。".into();
                (true, "执行完成：未命中告警 (No issue matched)".to_owned())
            }
            Err(err) => (false, format!("测试失败：{err}")),
        };
        self.extensions.custom_checks.test_output = Some(outcome);
        cx.notify();
    }

    pub(super) fn render_extensions_custom_scan_checks(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let table_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let header_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let header_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let zebra_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();
        let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
        let sel_border: gpui_kit::Hsla = rgb(if dark { 0x5b8ad6 } else { 0x4876b5 }).into();

        let editable = self.project.is_some() && !busy;
        let checks = &self.workspace.custom_scan_checks;
        let selected_id = self.extensions.custom_checks.selected_id;
        let current_enabled = self.extensions.custom_checks.enabled;
        let current_severity = self.extensions.custom_checks.severity;
        let current_confidence = self.extensions.custom_checks.confidence;
        let feedback = self.extensions.custom_checks.feedback.clone();
        let test_output = self.extensions.custom_checks.test_output.clone();
        let sample_open = self.extensions.custom_checks.sample_open;

        let mut table = div()
            .id("extensions-custom-checks-table")
            .test_support()
            .w_full()
            .border_1()
            .border_color(table_border)
            .bg(theme::surface())
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .h(px(24.))
                    .w_full()
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(table_border)
                    .flex()
                    .items_center()
                    .text_size(theme::ui_font_size())
                    .text_color(header_fg)
                    .child(
                        div()
                            .w(px(68.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("Enabled"),
                    )
                    .child(
                        div()
                            .w(px(210.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("Name"),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("Severity"),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("Confidence"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .child("Description / Script"),
                    )
                    .child(
                        div()
                            .w(px(70.))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .child("操作"),
                    ),
            );

        if checks.is_empty() {
            table = table.child(
                div()
                    .id("extensions-custom-checks-empty")
                    .test_support()
                    .min_h(px(72.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(12.))
                    .text_size(px(12.))
                    .text_color(theme::muted_foreground())
                    .child("当前项目尚未配置自定义扫描检查。使用下方编辑器创建 Rhai 检查规则，启用后将自动参与被动与主动扫描。"),
            );
        } else {
            for (index, check) in checks.iter().enumerate() {
                let id = check.id;
                let is_selected = selected_id == Some(id);
                let row_bg = if is_selected {
                    theme::selected()
                } else if index % 2 == 1 {
                    zebra_bg
                } else {
                    theme::surface()
                };
                let row_hover = if is_selected {
                    theme::selected()
                } else {
                    hover_bg
                };
                let summary_preview = if !check.description.trim().is_empty() {
                    check.description.clone()
                } else {
                    check
                        .script
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .trim()
                        .to_owned()
                };

                table = table.child(
                    div()
                        .id(SharedString::from(format!("custom-scan-check-row-{id}")))
                        .test_support()
                        .min_h(px(26.))
                        .w_full()
                        .bg(row_bg)
                        .when(is_selected, |row| {
                            row.border_t_1().border_b_1().border_color(sel_border)
                        })
                        .hover(move |s| s.bg(row_hover))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .text_size(theme::ui_font_size())
                        .text_color(theme::foreground())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_custom_scan_check(id, window, cx);
                        }))
                        .child(
                            div()
                                .w(px(68.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Checkbox::new(SharedString::from(format!(
                                        "custom-scan-check-toggle-{id}"
                                    )))
                                    .small()
                                    .checked(check.enabled)
                                    .disabled(!editable)
                                    .on_click(cx.listener(move |this, checked, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_custom_scan_check_enabled(id, *checked, cx);
                                    })),
                                ),
                        )
                        .child(
                            div()
                                .w(px(210.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .py(px(3.))
                                .truncate()
                                .child(check.name.clone()),
                        )
                        .child(
                            div()
                                .w(px(100.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .py(px(3.))
                                .truncate()
                                .child(severity_short_label(check.severity)),
                        )
                        .child(
                            div()
                                .w(px(100.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .py(px(3.))
                                .truncate()
                                .child(confidence_short_label(check.confidence)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(8.))
                                .py(px(3.))
                                .truncate()
                                .child(summary_preview),
                        )
                        .child(
                            div()
                                .w(px(70.))
                                .flex_shrink_0()
                                .px(px(6.))
                                .child(
                                    button(
                                        SharedString::from(format!("custom-scan-check-del-{id}")),
                                        "删除",
                                    )
                                    .ghost()
                                    .small()
                                    .disabled(!editable)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.delete_custom_scan_check(id, window, cx);
                                    })),
                                ),
                        ),
                );
            }
        }

        let severity_buttons = [
            (AuditSeverity::Information, "extensions-custom-check-sev-info"),
            (AuditSeverity::Low, "extensions-custom-check-sev-low"),
            (AuditSeverity::Medium, "extensions-custom-check-sev-medium"),
            (AuditSeverity::High, "extensions-custom-check-sev-high"),
        ]
        .into_iter()
        .map(|(sev, btn_id)| {
            button(btn_id, severity_label(sev))
                .small()
                .selected(current_severity == sev)
                .disabled(!editable)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.extensions.custom_checks.severity = sev;
                    cx.notify();
                }))
        });

        let confidence_buttons = [
            (
                AuditConfidence::Tentative,
                "extensions-custom-check-conf-tentative",
            ),
            (AuditConfidence::Firm, "extensions-custom-check-conf-firm"),
            (
                AuditConfidence::Certain,
                "extensions-custom-check-conf-certain",
            ),
        ]
        .into_iter()
        .map(|(conf, btn_id)| {
            button(btn_id, confidence_label(conf))
                .small()
                .selected(current_confidence == conf)
                .disabled(!editable)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.extensions.custom_checks.confidence = conf;
                    cx.notify();
                }))
        });

        let editor_card = div()
            .id("extensions-custom-check-editor-card")
            .test_support()
            .w_full()
            .p(px(12.))
            .border_1()
            .border_color(table_border)
            .rounded(px(3.))
            .bg(theme::toolbar_surface())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child(if selected_id.is_some() {
                                "编辑自定义扫描检查 (Rhai)"
                            } else {
                                "新建自定义扫描检查 (Rhai)"
                            }),
                    )
                    .child(
                        Checkbox::new("extensions-custom-check-enabled")
                            .label("启用检查（在审计引擎中自动执行）")
                            .checked(current_enabled)
                            .disabled(!editable)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.extensions.custom_checks.enabled = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme::muted_foreground())
                            .child("示例模板："),
                    )
                    .children(CUSTOM_SCAN_CHECK_TEMPLATES.into_iter().enumerate().map(
                        |(idx, (full_name, chip_label, desc, sev, conf, tpl_script))| {
                            button(
                                SharedString::from(format!("extensions-custom-check-tpl-{idx}")),
                                chip_label,
                            )
                            .ghost()
                            .small()
                            .tooltip(format!("填入示例检查模板：{full_name}"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.extensions.custom_checks.selected_id = None;
                                this.extensions.custom_checks.severity = sev;
                                this.extensions.custom_checks.confidence = conf;
                                this.extensions
                                    .custom_checks
                                    .name_input
                                    .update(cx, |inp, cx| inp.set_value(full_name, window, cx));
                                this.extensions
                                    .custom_checks
                                    .description_input
                                    .update(cx, |inp, cx| inp.set_value(desc, window, cx));
                                this.extensions
                                    .custom_checks
                                    .script_editor
                                    .update(cx, |ed, cx| ed.set_value(tpl_script, window, cx));
                                this.extensions.custom_checks.feedback = None;
                                this.extensions.custom_checks.test_output = None;
                                cx.notify();
                            }))
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(72.))
                            .flex_shrink_0()
                            .text_size(theme::ui_font_size())
                            .child("检查名称"),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.extensions.custom_checks.name_input)
                                .id("extensions-custom-check-name")
                                .aria_label("自定义扫描检查名称")
                                .small()
                                .h(px(26.)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(72.))
                            .flex_shrink_0()
                            .text_size(theme::ui_font_size())
                            .child("描述说明"),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.extensions.custom_checks.description_input)
                                .id("extensions-custom-check-desc")
                                .aria_label("自定义扫描检查描述")
                                .small()
                                .h(px(26.)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(theme::ui_font_size())
                                    .text_color(theme::muted_foreground())
                                    .child("危害级别："),
                            )
                            .children(severity_buttons),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(theme::ui_font_size())
                                    .text_color(theme::muted_foreground())
                                    .child("置信度："),
                            )
                            .children(confidence_buttons),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme::muted_foreground())
                    .child("检查脚本（变量：request / response / requestResponse，支持返回 bool、证据字符串或 #{ matched, evidence }）"),
            )
            .child(
                div()
                    .id("extensions-custom-check-script")
                    .test_support()
                    .w_full()
                    .h(px(110.))
                    .font_family(theme::mono_font_family())
                    .text_size(theme::editor_font_size())
                    .child(
                        Textarea::new(&self.extensions.custom_checks.script_editor)
                            .size_full()
                            .bordered(true)
                            .aria_label("自定义扫描检查 Rhai 脚本"),
                    ),
            )
            .when(sample_open, |card| {
                card.child(
                    div()
                        .p(px(8.))
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(3.))
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child("测试样本 HTTP 请求 / 响应"),
                        )
                        .child(
                            Input::new(&self.extensions.custom_checks.sample_url_input)
                                .id("extensions-custom-check-sample-url")
                                .aria_label("测试样本 URL")
                                .small()
                                .h(px(24.)),
                        )
                        .child(
                            div()
                                .id("extensions-custom-check-sample-response")
                                .test_support()
                                .w_full()
                                .h(px(76.))
                                .font_family(theme::mono_font_family())
                                .text_size(theme::editor_font_size())
                                .child(
                                    Textarea::new(
                                        &self.extensions.custom_checks.sample_response_editor,
                                    )
                                    .size_full()
                                    .bordered(true)
                                    .aria_label("测试样本 HTTP 响应"),
                                ),
                        ),
                )
            })
            .when_some(feedback, |card, (ok, msg)| {
                card.child(
                    div()
                        .id("extensions-custom-check-feedback")
                        .test_support()
                        .text_size(px(12.))
                        .text_color(if ok {
                            rgb(0x2e8b57).into()
                        } else {
                            theme::danger_fg()
                        })
                        .child(msg),
                )
            })
            .when_some(test_output, |card, (ok, msg)| {
                card.child(
                    div()
                        .id("extensions-custom-check-test-output")
                        .test_support()
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(if ok {
                            rgb(0x2e8b57).into()
                        } else {
                            theme::danger_border()
                        })
                        .bg(if ok {
                            theme::surface()
                        } else {
                            theme::danger_bg()
                        })
                        .text_size(px(12.))
                        .text_color(if ok {
                            theme::foreground()
                        } else {
                            theme::danger_fg()
                        })
                        .child(msg),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        button("extensions-custom-check-save", "保存检查")
                            .primary()
                            .icon(IconName::Save)
                            .tooltip("编译验证并保存到项目工作区（启用后立即生效于审计引擎）")
                            .disabled(!editable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_custom_scan_check(window, cx);
                            })),
                    )
                    .child(
                        button("extensions-custom-check-test", "实时测试 (Test)")
                            .icon(IconName::Play)
                            .tooltip("针对样本 HTTP 请求与响应实时执行当前 Rhai 扫描检查脚本")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.test_custom_scan_check(cx);
                            })),
                    )
                    .child(
                        button(
                            "extensions-custom-check-toggle-sample",
                            if sample_open {
                                "收起测试样本"
                            } else {
                                "编辑测试样本"
                            },
                        )
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.extensions.custom_checks.sample_open =
                                !this.extensions.custom_checks.sample_open;
                            cx.notify();
                        })),
                    )
                    .when_some(selected_id, |row, id| {
                        row.child(
                            button("extensions-custom-check-delete", "删除当前")
                                .ghost()
                                .icon(IconName::Trash)
                                .disabled(!editable)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.delete_custom_scan_check(id, window, cx);
                                })),
                        )
                    }),
            );

        div()
            .id("extensions-custom-scan-checks-view")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.))
            .bg(theme::surface())
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child("自定义扫描检查 (Custom scan checks)"),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child("编写并持久化 Rhai 自定义安全检查脚本至项目工作区，已启用的规则将在 MIO 审计引擎的被动与主动扫描中自动执行。"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div().w(px(110.)).child(
                            mio_neutral_button(
                                "extensions-custom-check-new",
                                "新建检查",
                                "创建新的 Rhai 自定义安全扫描检查",
                                editable,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_custom_scan_check(window, cx);
                            })),
                        ),
                    )
                    .child(
                        div().w(px(150.)).child(
                            mio_neutral_button(
                                "extensions-scan-open-dashboard",
                                "打开 Dashboard 扫描",
                                "切换到 Dashboard 查看与配置被动/主动扫描任务",
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_dashboard(cx);
                            })),
                        ),
                    )
                    .child(
                        div().w(px(140.)).child(
                            mio_neutral_button(
                                "extensions-scan-add-local",
                                "添加本地扩展…",
                                "返回已安装页并选择本地扫描扩展包目录",
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.extensions.page = ExtensionsPage::Installed;
                                this.extensions.add_panel_open = true;
                                this.choose_plugin(window, cx);
                            })),
                        ),
                    ),
            )
            .child(table)
            .child(editor_card)
            .into_any_element()
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;

    #[test]
    fn all_custom_scan_check_templates_compile_and_trigger_on_matching_traffic() {
        let req = HttpRequest {
            method: "GET".into(),
            url: "https://target.example/debug".into(),
            version: HttpVersion::Http11,
            headers: vec![Header {
                name: "Host".into(),
                value: b"target.example".to_vec(),
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        };
        let matching_resp = HttpResponse {
            version: HttpVersion::Http11,
            status: 200,
            reason_phrase: Some(b"OK".to_vec()),
            headers: vec![
                Header {
                    name: "Access-Control-Allow-Origin".into(),
                    value: b"*".to_vec(),
                },
                Header {
                    name: "Access-Control-Allow-Credentials".into(),
                    value: b"true".to_vec(),
                },
                Header {
                    name: "Set-Cookie".into(),
                    value: b"sid=1; Path=/".to_vec(),
                },
                Header {
                    name: "X-Powered-By".into(),
                    value: b"Express/4.18".to_vec(),
                },
                Header {
                    name: "Server".into(),
                    value: b"nginx/1.25.0".to_vec(),
                },
            ],
            body: b"X-Debug-Secret=1 DB_PASSWORD=demo ref: refs/heads/main".to_vec(),
            trailers: Vec::new(),
        };

        for (title, _, _, _, _, script) in CUSTOM_SCAN_CHECK_TEMPLATES {
            mio_core::validate_filter_script(script)
                .unwrap_or_else(|e| panic!("scan check template {title} syntax error: {e}"));
            let hit = mio_core::eval_custom_scan_check_script(script, &req, &matching_resp)
                .unwrap_or_else(|e| panic!("scan check template {title} runtime error: {e}"));
            assert!(
                hit.is_some(),
                "scan check template {title} should trigger on matching sample traffic"
            );
        }
    }
}
