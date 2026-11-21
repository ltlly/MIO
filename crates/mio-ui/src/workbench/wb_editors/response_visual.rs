use super::*;
use gpui_kit::assets::IconName;

struct CachedRenderedResponse {
    status_line: String,
    status_code: Option<u16>,
    content_type: Option<String>,
    location: Option<String>,
    body_len: usize,
    summary: editing::RenderedBodySummary,
    body_for_browser: std::sync::Arc<Vec<u8>>,
}

std::thread_local! {
    static RENDERED_RESPONSE_CACHE: std::cell::RefCell<
        Vec<(
            crate::workbench::inspector::MessageEditorRevKey,
            Option<std::rc::Rc<CachedRenderedResponse>>,
        )>,
    > = const { std::cell::RefCell::new(Vec::new()) };
}

impl Workbench {
    pub(in crate::workbench) fn render_response_editor_body(
        &self,
        id: &'static str,
        response: &MessageEditor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if response.render_active {
            return self.render_response_visual_card(id, response, cx);
        }
        editing::message_editor(response.state())
            .readonly(true)
            .size_full()
            .context_menu(self.request_context_menu(response.state().clone(), true))
            .aria_label("响应消息编辑器")
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_response_visual_card(
        &self,
        id: &'static str,
        response: &MessageEditor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rev_key = crate::workbench::inspector::message_editor_rev_key(response, cx);
        let cached = RENDERED_RESPONSE_CACHE.with(|cache| {
            cache
                .borrow()
                .iter()
                .find(|(k, _)| *k == rev_key)
                .map(|(_, val)| val.clone())
        });
        let entry = if let Some(hit) = cached {
            hit
        } else {
            let raw = response.bytes_lossless(cx);
            let computed = if raw.is_empty() {
                None
            } else {
                let parsed = editing::parse_rendered_http_response(&raw);
                let body_for_browser =
                    std::sync::Arc::new(editing::extract_http_response_body(&raw));
                Some(std::rc::Rc::new(CachedRenderedResponse {
                    status_line: parsed.status_line,
                    status_code: parsed.status_code,
                    content_type: parsed.content_type,
                    location: parsed.location,
                    body_len: parsed.body_len,
                    summary: parsed.summary,
                    body_for_browser,
                }))
            };
            RENDERED_RESPONSE_CACHE.with(|cache| {
                let mut entries = cache.borrow_mut();
                if let Some(pos) = entries.iter().position(|(k, _)| *k == rev_key) {
                    entries.remove(pos);
                } else if entries.len() >= 4 {
                    entries.remove(0);
                }
                entries.push((rev_key, computed.clone()));
            });
            computed
        };
        let Some(parsed) = entry else {
            return div()
                .id(SharedString::from(format!("{id}-render-card")))
                .test_support()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::surface())
                .text_color(theme::muted_foreground())
                .child("暂无响应内容可供渲染预览")
                .into_any_element();
        };
        let dark = theme::is_dark();
        let card_bg = rgb(if dark { 0x25282e } else { 0xf8f9fb });
        let badge_color = parsed
            .status_code
            .map(|code| editing::status_code_color(code, dark, false))
            .unwrap_or_else(theme::foreground);
        let ct_label = parsed
            .content_type
            .clone()
            .unwrap_or_else(|| "未声明 Content-Type".into());
        let body_for_browser = std::sync::Arc::clone(&parsed.body_for_browser);
        let ct_for_browser = parsed
            .content_type
            .clone()
            .unwrap_or_else(|| "text/html; charset=utf-8".into());

        let mut header_bar = div()
            .w_full()
            .p(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme::border())
            .bg(card_bg)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .font_family(theme::mono_font_family())
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(badge_color)
                    .child(parsed.status_line.clone()),
            )
            .child(
                div()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(theme::toolbar_surface())
                    .text_size(theme::scaled_px(11.5))
                    .text_color(theme::muted_foreground())
                    .child(format!("{ct_label} · {} 字节", parsed.body_len)),
            )
            .child(div().flex_1())
            .child(
                button(
                    SharedString::from(format!("{id}-render-open-browser")),
                    "在系统浏览器中预览响应",
                )
                .icon(IconName::ExternalLink)
                .tooltip("将当前响应正文写入本地工作区预览文件并在系统默认浏览器中打开")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_rendered_response_in_browser(&body_for_browser, &ct_for_browser, cx);
                })),
            );

        if let Some(loc) = &parsed.location {
            let loc_copy = loc.clone();
            header_bar = header_bar.child(
                button(
                    SharedString::from(format!("{id}-render-copy-location")),
                    "复制重定向链接",
                )
                .icon(IconName::Link)
                .tooltip(format!("复制 Location 标头：{loc}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(loc_copy.clone()));
                    this.status = "已复制重定向 Location 链接到剪贴板。".into();
                    cx.notify();
                })),
            );
        }

        let mut content = div()
            .id(SharedString::from(format!("{id}-render-card")))
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .bg(theme::surface())
            .child(header_bar);

        match &parsed.summary {
            editing::RenderedBodySummary::Html {
                title,
                headings,
                paragraphs,
                links,
                forms,
                scripts,
            } => {
                if let Some(page_title) = title {
                    content = content.child(
                        div()
                            .p(px(10.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(card_bg)
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(11.))
                                    .text_color(theme::muted_foreground())
                                    .child("页面标题 (<title>)"),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(14.5))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(page_title.clone()),
                            ),
                    );
                }
                if !headings.is_empty() || !paragraphs.is_empty() {
                    let mut body_box = div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child("可见页面文本摘要"),
                        );
                    for (lvl, heading) in headings {
                        body_box = body_box.child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_size(theme::scaled_px((16.0 - f32::from(*lvl)).max(12.0)))
                                .child(format!("H{lvl} · {heading}")),
                        );
                    }
                    for para in paragraphs {
                        body_box = body_box.child(div().child(para.clone()));
                    }
                    content = content.child(body_box);
                }
                if !links.is_empty() {
                    let mut links_box = div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("超链接列表 ({})", links.len())),
                        );
                    for (idx, link) in links.iter().enumerate() {
                        let href_copy = link.href.clone();
                        let display_text = if link.text.is_empty() {
                            link.href.clone()
                        } else {
                            format!("{} — {}", link.text, link.href)
                        };
                        links_box = links_box.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(theme::mono_font_family())
                                        .text_size(theme::scaled_px(11.5))
                                        .child(display_text),
                                )
                                .child(
                                    button(
                                        (SharedString::from(format!("{id}-copy-href")), idx),
                                        "复制链接",
                                    )
                                    .icon(IconName::Copy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(
                                            gpui_kit::ClipboardItem::new_string(href_copy.clone()),
                                        );
                                        this.status = "已复制超链接地址到剪贴板。".into();
                                        cx.notify();
                                    })),
                                ),
                        );
                    }
                    content = content.child(links_box);
                }
                if !forms.is_empty() {
                    let mut forms_box = div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("表单与输入控件 ({})", forms.len())),
                        );
                    for form in forms {
                        let mut form_card = div()
                            .p(px(8.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface())
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .font_family(theme::mono_font_family())
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(format!("{} {}", form.method, form.action)),
                            );
                        for inp in &form.inputs {
                            form_card = form_card.child(
                                div()
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::scaled_px(11.5))
                                    .text_color(theme::muted_foreground())
                                    .child(format!(
                                        "[{}] name=\"{}\" value=\"{}\"",
                                        inp.input_type, inp.name, inp.value
                                    )),
                            );
                        }
                        forms_box = forms_box.child(form_card);
                    }
                    content = content.child(forms_box);
                }
                if !scripts.is_empty() {
                    let mut scripts_box = div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("外部脚本引用 ({})", scripts.len())),
                        );
                    for src in scripts {
                        scripts_box = scripts_box.child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .child(src.clone()),
                        );
                    }
                    content = content.child(scripts_box);
                }
            }
            editing::RenderedBodySummary::Json { pretty, byte_len } => {
                content = content.child(
                    div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("JSON 结构化响应卡片 ({byte_len} 字节)")),
                        )
                        .child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(12.))
                                .child(pretty.clone()),
                        ),
                );
            }
            editing::RenderedBodySummary::Image {
                format_name,
                mime_type,
                width,
                height,
                byte_len,
            } => {
                let dims = match (*width, *height) {
                    (Some(w), Some(h)) => format!("{w} × {h} px"),
                    _ => "尺寸未声明".into(),
                };
                content = content.child(
                    div()
                        .p(px(12.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("图像资源元数据卡片 · {format_name}")),
                        )
                        .child(div().child(format!("MIME 类型：{mime_type}")))
                        .child(div().child(format!("图像尺寸：{dims}")))
                        .child(div().child(format!("载荷大小：{byte_len} 字节"))),
                );
            }
            editing::RenderedBodySummary::PlainText { text, byte_len } => {
                content = content.child(
                    div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("纯文本响应预览 ({byte_len} 字节)")),
                        )
                        .child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(12.))
                                .child(text.clone()),
                        ),
                );
            }
            editing::RenderedBodySummary::Binary {
                byte_len,
                hex_preview,
            } => {
                content = content.child(
                    div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(card_bg)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!(
                                    "二进制响应载荷 ({byte_len} 字节，可通过右键菜单保存响应体到文件)"
                                )),
                        )
                        .child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .child(hex_preview.clone()),
                        ),
                );
            }
        }
        content.into_any_element()
    }

    pub(super) fn open_rendered_response_in_browser(
        &mut self,
        body_bytes: &[u8],
        content_type: &str,
        cx: &mut Context<Self>,
    ) {
        let lower_ct = content_type.to_ascii_lowercase();
        let ext = if lower_ct.contains("json") {
            "json"
        } else if lower_ct.contains("svg") {
            "svg"
        } else if lower_ct.contains("png") {
            "png"
        } else if lower_ct.contains("jpeg") || lower_ct.contains("jpg") {
            "jpg"
        } else if lower_ct.contains("gif") {
            "gif"
        } else if lower_ct.contains("webp") {
            "webp"
        } else if lower_ct.contains("plain") {
            "txt"
        } else {
            "html"
        };
        let preview_dir = self
            .project_path
            .as_ref()
            .and_then(|p| p.parent().map(PathBuf::from))
            .or_else(|| {
                mio_core::default_user_preferences_path()
                    .ok()
                    .and_then(|p| p.parent().map(|dir| dir.join("render-previews")))
            })
            .unwrap_or_else(|| PathBuf::from(".local/render-previews"));
        if let Err(error) = std::fs::create_dir_all(&preview_dir) {
            self.fail(format!("无法创建预览目录：{error}"), cx);
            return;
        }
        let file_path = preview_dir.join(format!(
            "mio-response-preview-{}.{ext}",
            mio_core::Uuid::new_v4()
        ));
        if let Err(error) = std::fs::write(&file_path, body_bytes) {
            self.fail(format!("写入响应预览文件失败：{error}"), cx);
            return;
        }
        let abs_path = std::fs::canonicalize(&file_path).unwrap_or(file_path);
        let path_display = abs_path.to_string_lossy().replace('\\', "/");
        let file_url = if path_display.starts_with('/') {
            format!("file://{path_display}")
        } else {
            format!("file:///{path_display}")
        };
        cx.open_url(&file_url);
        self.error = None;
        self.status = format!("已在系统浏览器中打开响应预览：{}", abs_path.display());
        cx.notify();
    }

}
