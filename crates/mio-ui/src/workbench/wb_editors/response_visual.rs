use super::*;
use gpui_kit::assets::IconName;

pub(super) struct CachedRenderedResponse {
    status_line: String,
    status_code: Option<u16>,
    content_type: Option<String>,
    location: Option<String>,
    body_len: usize,
    summary: editing::RenderedBodySummary,
    body_for_browser: std::sync::Arc<Vec<u8>>,
    inline_image: Option<(std::sync::Arc<gpui_kit::RenderImage>, u32, u32)>,
}

fn decode_static_inline_image(
    body: &[u8],
    format_name: &str,
    width: Option<u32>,
    height: Option<u32>,
    cx: &App,
) -> Option<(std::sync::Arc<gpui_kit::RenderImage>, u32, u32)> {
    if format_name == "SVG" {
        if body.len() > 512 * 1024 {
            return None;
        }
        let img = cx.svg_renderer().render_single_frame(body, 1.0).ok()?;
        let w = width.unwrap_or(320).clamp(32, 4096);
        let h = height.unwrap_or(180).clamp(32, 4096);
        return Some((img, w, h));
    }
    if body.len() > 4 * 1024 * 1024 {
        return None;
    }
    if let (Some(w), Some(h)) = (width, height)
        && (w == 0 || h == 0 || w > 4096 || h > 4096)
    {
        return None;
    }
    let decoded = image::load_from_memory(body).ok()?;
    let w = decoded.width();
    let h = decoded.height();
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    let mut rgba = decoded.into_rgba8();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some((
        std::sync::Arc::new(gpui_kit::RenderImage::new(vec![image::Frame::new(rgba)])),
        w,
        h,
    ))
}

fn fit_preview_box(w: u32, h: u32, max_w: f32, max_h: f32) -> (f32, f32) {
    let wf = w.max(1) as f32;
    let hf = h.max(1) as f32;
    let scale = (max_w / wf).min(max_h / hf).min(1.0);
    ((wf * scale).max(16.0), (hf * scale).max(16.0))
}

impl Workbench {
    pub(in crate::workbench) fn render_response_editor_body(
        &self,
        slot: EditorSlot,
        response: &MessageEditor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if response.render_active {
            return self.render_response_visual_card(slot.id(), response, cx);
        }
        editing::message_editor(response)
            .readonly(true)
            .size_full()
            .context_menu(self.request_context_menu(response.state(), true))
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
        let cached = self
            .editor_ui_caches
            .rendered_response_cache
            .borrow()
            .iter()
            .find(|(k, _)| *k == rev_key)
            .map(|(_, val)| val.clone());
        let entry = if let Some(hit) = cached {
            hit
        } else {
            let raw = response.bytes_lossless(cx);
            let computed = if raw.is_empty() {
                None
            } else {
                let parsed = editing::parse_rendered_http_response(&raw);
                let inline_image = match &parsed.summary {
                    editing::RenderedBodySummary::Image {
                        format_name,
                        width,
                        height,
                        ..
                    } => decode_static_inline_image(&parsed.body, format_name, *width, *height, cx),
                    _ => None,
                };
                let body_for_browser = std::sync::Arc::new(parsed.body);
                Some(std::rc::Rc::new(CachedRenderedResponse {
                    status_line: parsed.status_line,
                    status_code: parsed.status_code,
                    content_type: parsed.content_type,
                    location: parsed.location,
                    body_len: parsed.body_len,
                    summary: parsed.summary,
                    body_for_browser,
                    inline_image,
                }))
            };
            {
                let mut entries = self.editor_ui_caches.rendered_response_cache.borrow_mut();
                if let Some(pos) = entries.iter().position(|(k, _)| *k == rev_key) {
                    entries.remove(pos);
                } else if entries.len() >= 4 {
                    entries.remove(0);
                }
                entries.push((rev_key, computed.clone()));
            }
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
            editing::RenderedBodySummary::Html(html_summary) => {
                let editing::RenderedHtmlSummary {
                    title,
                    meta_description,
                    headings,
                    paragraphs,
                    links,
                    forms,
                    scripts,
                    stylesheets,
                    images,
                    tables,
                    code_blocks,
                    inline_theme,
                } = html_summary.as_ref();
                if title.is_some() || meta_description.is_some() {
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
                            .when_some(title.clone(), |box_el, page_title| {
                                box_el
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
                                            .child(page_title),
                                    )
                            })
                            .when_some(meta_description.clone(), |box_el, desc| {
                                box_el.child(
                                    div()
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::muted_foreground())
                                        .child(desc),
                                )
                            }),
                    );
                }
                if inline_theme.background_hex.is_some()
                    || inline_theme.text_hex.is_some()
                    || inline_theme.accent_hex.is_some()
                {
                    let mut swatches = div()
                        .id(SharedString::from(format!("{id}-render-css-theme")))
                        .test_support()
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
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child("静态 CSS 配色提取"),
                        );
                    for (label, maybe_hex) in [
                        ("背景", inline_theme.background_hex),
                        ("正文", inline_theme.text_hex),
                        ("强调", inline_theme.accent_hex),
                    ] {
                        if let Some(hex_val) = maybe_hex {
                            swatches = swatches.child(
                                div()
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .child(
                                        div()
                                            .w(px(12.))
                                            .h(px(12.))
                                            .rounded(px(2.))
                                            .border_1()
                                            .border_color(theme::border())
                                            .bg(rgb(hex_val)),
                                    )
                                    .child(
                                        div()
                                            .font_family(theme::mono_font_family())
                                            .text_size(theme::scaled_px(11.))
                                            .child(format!("{label}: #{hex_val:06x}")),
                                    ),
                            );
                        }
                    }
                    content = content.child(swatches);
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
                if !tables.is_empty() {
                    let mut tables_box = div()
                        .id(SharedString::from(format!("{id}-render-tables")))
                        .test_support()
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
                                .child(format!("静态表格内容 ({})", tables.len())),
                        );
                    for table in tables {
                        let mut grid = div()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::surface())
                            .overflow_hidden()
                            .flex()
                            .flex_col();
                        if !table.headers.is_empty() {
                            let mut hdr_row = div()
                                .w_full()
                                .px(px(8.))
                                .py(px(4.))
                                .bg(theme::toolbar_surface())
                                .border_b_1()
                                .border_color(theme::border())
                                .flex()
                                .gap(px(8.));
                            for col in &table.headers {
                                hdr_row = hdr_row.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .text_size(theme::scaled_px(11.5))
                                        .child(col.clone()),
                                );
                            }
                            grid = grid.child(hdr_row);
                        }
                        for (r_idx, row) in table.rows.iter().enumerate() {
                            let mut row_el = div()
                                .w_full()
                                .px(px(8.))
                                .py(px(3.))
                                .when(r_idx % 2 == 1, |r| r.bg(card_bg))
                                .flex()
                                .gap(px(8.));
                            for cell in row {
                                row_el = row_el.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(theme::scaled_px(11.5))
                                        .child(cell.clone()),
                                );
                            }
                            grid = grid.child(row_el);
                        }
                        tables_box = tables_box.child(grid);
                    }
                    content = content.child(tables_box);
                }
                if !code_blocks.is_empty() {
                    let mut code_box = div()
                        .id(SharedString::from(format!("{id}-render-code")))
                        .test_support()
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
                                .child(format!("代码块片段 ({})", code_blocks.len())),
                        );
                    for block in code_blocks {
                        code_box = code_box.child(
                            div()
                                .p(px(8.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme::border())
                                .bg(theme::surface())
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .child(block.clone()),
                        );
                    }
                    content = content.child(code_box);
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
                if !images.is_empty() {
                    let mut imgs_box = div()
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
                                .child(format!("静态图片引用 ({})", images.len())),
                        );
                    for image_ref in images {
                        let line = if image_ref.alt.is_empty() {
                            image_ref.src.clone()
                        } else {
                            format!("{} [alt: {}]", image_ref.src, image_ref.alt)
                        };
                        imgs_box = imgs_box.child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .child(line),
                        );
                    }
                    content = content.child(imgs_box);
                }
                if !scripts.is_empty() || !stylesheets.is_empty() {
                    let mut assets_box = div()
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
                                .child(format!(
                                    "外部脚本与样式引用 ({} JS · {} CSS)",
                                    scripts.len(),
                                    stylesheets.len()
                                )),
                        );
                    for src in scripts {
                        assets_box = assets_box.child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .child(format!("[JS] {src}")),
                        );
                    }
                    for href in stylesheets {
                        assets_box = assets_box.child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .child(format!("[CSS] {href}")),
                        );
                    }
                    content = content.child(assets_box);
                }
            }
            editing::RenderedBodySummary::Xml {
                root_tag,
                node_count,
                pretty,
                byte_len,
            } => {
                let root_label = root_tag.as_deref().unwrap_or("root");
                content = content.child(
                    div()
                        .id(SharedString::from(format!("{id}-render-xml")))
                        .test_support()
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
                                    "XML 结构化响应卡片 (<{root_label}> · {node_count} 节点 · {byte_len} 字节)"
                                )),
                        )
                        .child(
                            div()
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(12.))
                                .child(pretty.clone()),
                        ),
                );
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
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(format!("图像离线静态预览 · {format_name}")),
                        )
                        .when_some(parsed.inline_image.clone(), |card, (img_handle, iw, ih)| {
                            let (disp_w, disp_h) = fit_preview_box(iw, ih, 420.0, 280.0);
                            card.child(
                                div()
                                    .id(SharedString::from(format!("{id}-render-image-preview")))
                                    .test_support()
                                    .p(px(8.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .bg(theme::surface())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(gpui_kit::img(img_handle).w(px(disp_w)).h(px(disp_h))),
                            )
                        })
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
        let project = self.project;
        let ext = ext.to_owned();
        let body = body_bytes.to_vec();
        cx.spawn(async move |this, cx| {
            let write_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::write_response_preview_file(&preview_dir, &ext, &body)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.project != project {
                    return;
                }
                match write_result {
                    Ok(abs_path) => {
                        let path_display = abs_path.to_string_lossy().replace('\\', "/");
                        let file_url = if path_display.starts_with('/') {
                            format!("file://{path_display}")
                        } else {
                            format!("file:///{path_display}")
                        };
                        cx.open_url(&file_url);
                        view.error = None;
                        view.status =
                            format!("已在系统浏览器中打开响应预览：{}", abs_path.display());
                        cx.notify();
                    }
                    Err(error) => {
                        view.fail(format!("写入响应预览文件失败：{error}"), cx);
                    }
                }
            });
        })
        .detach();
    }

}
