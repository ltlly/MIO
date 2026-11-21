use gpui_kit::ClipboardItem;
use gpui_kit::assets::IconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator, Radio};
use gpui_kit::component::Icon;
use gpui_kit::uniform_list;
use std::rc::Rc;

use super::*;

pub(in crate::workbench) fn format_sequencer_analysis_report(report: &SequencerAnalysis) -> String {
    use std::fmt::Write as _;

    let (rating_text, _, _) = quality_badge(report.quality);
    let length_range = if report.min_length == report.max_length {
        format!("{} B", report.min_length)
    } else {
        format!("{}–{} B", report.min_length, report.max_length)
    };
    let mut out = String::new();
    let _ = writeln!(out, "=== Sequencer 随机性统计分析报告 ===");
    let _ = writeln!(out, "质量评级：{rating_text}");
    let _ = writeln!(out, "有效熵：{:.2} bits", report.effective_entropy_bits);
    let _ = writeln!(out, "样本总数：{}", report.sample_count);
    let _ = writeln!(out, "长度范围：{length_range}");
    let _ = writeln!(out, "字符集大小：{}", report.charset_size);
    let _ = writeln!(out, "识别编码：{}", encoding_label(report.encoding));
    let _ = writeln!(out, "诊断结论：{}", report.diagnosis);
    let _ = writeln!(out);
    let _ = writeln!(out, "--- FIPS 140-2 四项随机性检验 ---");
    match &report.bit_analysis.fips {
        FipsStatus::Pass => {
            let _ = writeln!(
                out,
                "状态：Pass (4/4 全部通过，已检验前 {MIN_FIPS_TEST_BITS} bits)"
            );
        }
        FipsStatus::InsufficientBits { available_bits } => {
            let _ = writeln!(
                out,
                "状态：样本比特不足（当前 {available_bits} bits < {MIN_FIPS_TEST_BITS} bits）"
            );
        }
        FipsStatus::Failed { failures } => {
            let _ = writeln!(out, "状态：Failed（{} / 4 项未通过）", failures.len());
            for failure in failures {
                let _ = writeln!(
                    out,
                    "  - {} | 实测：{} | 标准区间：{}",
                    fips_test_label(failure.test),
                    failure.observed,
                    failure.requirement
                );
            }
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "--- 比特级统计 ---");
    let _ = writeln!(
        out,
        "可用比特：{} | 已分析比特：{} | 1-bit 计数：{} ({:.2}%)",
        report.bit_analysis.available_bits,
        report.bit_analysis.analyzed_bits,
        report.bit_analysis.ones_count,
        report.bit_analysis.ones_ratio * 100.0
    );
    let _ = writeln!(
        out,
        "比特香农熵：{:.4} bits | 相邻比特自相关：{:.4}",
        report.bit_analysis.shannon_entropy_bits, report.bit_analysis.adjacent_autocorrelation
    );
    if !report.character_analysis.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "--- 字符级逐位置熵表 ---");
        let _ = writeln!(out, "位置\t样本数\t字符种类\tShannon(b)\tχ²\tp-value\t1%有效熵");
        for item in &report.character_analysis {
            let _ = writeln!(
                out,
                "#{}\t{}\t{}\t{:.3}\t{:.2}\t{:.4}\t{:.3}",
                item.position,
                item.samples_with_position,
                item.observed_characters,
                item.shannon_entropy_bits,
                item.chi_square,
                item.p_value,
                item.effective_entropy_bits
            );
        }
    }
    out
}

pub(in crate::workbench) fn prepare_samples_for_analysis(
    samples: Vec<String>,
    base64_predecode: bool,
) -> Result<Vec<String>, String> {
    if !base64_predecode {
        return Ok(samples);
    }
    let mut decoded_raw = Vec::with_capacity(samples.len());
    for (idx, token) in samples.iter().enumerate() {
        let bytes = crate::editing::base64_decode_bytes(token.trim()).map_err(|_| {
            format!(
                "第 {} 条样本不是有效的 Base64 编码，未执行预解码分析。",
                idx + 1
            )
        })?;
        if bytes.is_empty() {
            return Err(format!("第 {} 条样本 Base64 解码后为空。", idx + 1));
        }
        decoded_raw.push(bytes);
    }
    let all_ascii = decoded_raw.iter().all(|b| {
        (MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&b.len()) && b.iter().all(u8::is_ascii_graphic)
    });
    let mut total_bytes = 0usize;
    let mut out = Vec::with_capacity(decoded_raw.len());
    for (idx, bytes) in decoded_raw.into_iter().enumerate() {
        let transformed = if all_ascii {
            String::from_utf8(bytes).map_err(|_| format!("第 {} 条样本解码失败。", idx + 1))?
        } else {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
            }
            hex
        };
        if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&transformed.len()) {
            return Err(format!(
                "第 {} 条样本解码后长度超出 {MIN_TOKEN_BYTES}–{MAX_TOKEN_BYTES} 字节上限。",
                idx + 1
            ));
        }
        total_bytes = total_bytes.saturating_add(transformed.len());
        if total_bytes > MAX_TOTAL_SAMPLE_BYTES {
            return Err("Base64 预解码后样本总字节数超过 8 MiB 上限。".into());
        }
        out.push(transformed);
    }
    Ok(out)
}

impl Workbench {
    pub(in crate::workbench) fn fill_sequencer_delimiters_from_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let editor = self.sequencer_pane.request.state();
        let range = editor.read(cx).selected_range();
        let full = editor.read(cx).value();
        if range.is_empty() || range.end > full.len() {
            self.fail("请先在采样源请求编辑器中选中目标位置两侧上下文中间的文本。", cx);
            return;
        }
        let before = full[..range.start].rsplit(['\r', '\n']).next().unwrap_or("");
        let after = full[range.end..].split(['\r', '\n']).next().unwrap_or("");
        let start: String = before.chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
        let end: String = after.chars().take(24).collect();
        if start.is_empty() || end.is_empty() {
            self.fail("当前选区前后缺少非空同行的起止分隔上下文。", cx);
            return;
        }
        self.sequencer_pane.location_kind = LocationKind::BodyDelimiter;
        self.sequencer_pane.custom_config_open = true;
        self.sequencer_pane.delim_start.update(cx, |s, cx| s.set_value(start, window, cx));
        self.sequencer_pane.delim_end.update(cx, |s, cx| s.set_value(end, window, cx));
        self.error = None;
        self.sequencer_pane.status_text = Some("已根据请求选区自动生成起止分隔符。".into());
        cx.notify();
    }

    pub(in crate::workbench) fn import_sequencer_samples_from_file(
        &mut self,
        to_manual: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate()
            || self.sequencer_pane.running.is_some()
            || self.sequencer_pane.pending_analysis.is_some()
            || self.file_picker_pending()
        {
            return;
        }
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("导入 Sequencer 样本文件".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let paths = paths.await;
            let _ = this.update_in(cx, |view, window, cx| {
                if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                    return;
                }
                let Some(path) = (match paths {
                    Ok(Ok(Some(paths))) => paths.into_iter().next(),
                    Ok(Ok(None)) => None,
                    Ok(Err(err)) => { view.fail(format!("选择样本文件失败：{err}"), cx); return; }
                    Err(_) => { view.fail("文件选择器已关闭。", cx); return; }
                }) else {
                    return;
                };
                let content = match mio_core::ui_files::read_file_to_string(&path) {
                    Ok(text) => text,
                    Err(err) => { view.fail(format!("读取样本文件失败：{err}"), cx); return; }
                };
                let lines: Vec<String> = parse_manual_lines(&content)
                    .into_iter()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                if lines.is_empty() {
                    view.fail("所选样本文件不包含有效样本行。", cx);
                    return;
                }
                for (idx, line) in lines.iter().enumerate() {
                    if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&line.len()) {
                        view.fail(format!("第 {} 行长度超出 {MIN_TOKEN_BYTES}–{MAX_TOKEN_BYTES} 字节范围。", idx + 1), cx);
                        return;
                    }
                }
                if to_manual {
                    let existing = view.sequencer_pane.manual_editor.read(cx).value().to_string();
                    let joined = lines.join("\n");
                    let combined = if existing.trim_matches(['\r', '\n']).is_empty() {
                        joined
                    } else if existing.ends_with('\n') {
                        format!("{existing}{joined}")
                    } else {
                        format!("{existing}\n{joined}")
                    };
                    if combined.len() > MAX_TOTAL_SAMPLE_BYTES + MAX_SAMPLE_COUNT * 2 {
                        view.fail("导入后样本总大小超过 8 MiB 上限。", cx);
                        return;
                    }
                    view.sequencer_pane.manual_editor.update(cx, |s, cx| s.set_value(combined, window, cx));
                    view.sequencer_pane.status_text = Some(format!("已从 {} 导入 {} 条样本。", path.display(), lines.len()));
                } else {
                    if view.sequencer_pane.captured_samples.len() + lines.len() > MAX_SAMPLE_COUNT {
                        view.fail(format!("导入后样本总数超过 {MAX_SAMPLE_COUNT} 条上限。"), cx);
                        return;
                    }
                    let added = lines.len();
                    view.sequencer_pane.captured_samples.extend(lines);
                    let total = view.sequencer_pane.captured_samples.len();
                    view.sequencer_pane.capture.tokens_collected = total;
                    view.sequencer_pane.capture.latest_samples = view.sequencer_pane.captured_samples
                        .iter()
                        .skip(total.saturating_sub(MAX_PREVIEW_SAMPLES))
                        .cloned()
                        .collect();
                    view.sequencer_pane.sync_preview(window, cx);
                    view.sequencer_pane.status_text = Some(format!("已从文件追加 {added} 条样本（共 {total} 条）。"));
                }
                view.error = None;
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn export_sequencer_samples_to_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.file_picker_pending() {
            return;
        }
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let tokens: Vec<String> = if self.sequencer_pane.page == SequencerPage::Manual {
            let raw = self.sequencer_pane.manual_editor.read(cx).value();
            parse_manual_lines(raw.as_ref()).into_iter().filter(|l| !l.is_empty()).map(str::to_owned).collect()
        } else if !self.sequencer_pane.captured_samples.is_empty() {
            self.sequencer_pane.captured_samples.clone()
        } else {
            self.sequencer_pane.capture.latest_samples.clone()
        };
        if tokens.is_empty() {
            self.fail("当前没有可导出的 Token 样本。", cx);
            return;
        }
        let payload = format!("{}\n", tokens.join("\n"));
        let count = tokens.len();
        self.save_sequencer_file("sequencer-samples.txt", payload.into_bytes(), format!("已导出 {count} 条样本"), window, cx);
    }

    pub(in crate::workbench) fn copy_sequencer_analysis_report(&mut self, cx: &mut Context<Self>) {
        let Some(report) = self.sequencer_pane.analysis.as_deref() else {
            self.fail("当前尚无统计分析报告可复制。", cx);
            return;
        };
        let text = format_sequencer_analysis_report(report);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.error = None;
        let msg = "已复制完整 Sequencer 统计分析报告到剪贴板。".to_owned();
        self.sequencer_pane.status_text = Some(msg.clone());
        self.status = msg;
        cx.notify();
    }

    pub(in crate::workbench) fn export_sequencer_analysis_to_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sequencer_can_operate() || self.file_picker_pending() {
            return;
        }
        let Some(report) = self.sequencer_pane.analysis.as_deref() else {
            self.fail("当前尚无统计分析报告可导出。", cx);
            return;
        };
        let json = match serde_json::to_vec_pretty(report) {
            Ok(bytes) => bytes,
            Err(err) => { self.fail(format!("序列化分析报告失败：{err}"), cx); return; }
        };
        self.save_sequencer_file("sequencer-analysis.json", json, "已导出 Sequencer 分析报告".into(), window, cx);
    }

    fn save_sequencer_file(
        &mut self,
        default_name: &'static str,
        bytes: Vec<u8>,
        done_prefix: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let directory = self.default_directory();
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(window, cx, &directory, Some(default_name));
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                    return;
                }
                match result {
                    Ok(Ok(Some(path))) => match mio_core::ui_files::write_export_file(&path, &bytes) {
                        Ok(()) => {
                            view.error = None;
                            let msg = format!("{done_prefix}到 {}。", path.display());
                            view.sequencer_pane.status_text = Some(msg.clone());
                            view.status = msg;
                        }
                        Err(err) => view.fail(format!("写入文件失败：{err}"), cx),
                    },
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => view.fail(format!("选择导出路径失败：{err}"), cx),
                    Err(_) => view.fail("文件选择器已关闭。", cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn copy_all_sequencer_samples(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let tokens: Vec<String> = if self.sequencer_pane.page == SequencerPage::Manual {
            let raw = self.sequencer_pane.manual_editor.read(cx).value();
            parse_manual_lines(raw.as_ref())
                .into_iter()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        } else if !self.sequencer_pane.captured_samples.is_empty() {
            self.sequencer_pane.captured_samples.clone()
        } else {
            self.sequencer_pane.capture.latest_samples.clone()
        };
        if tokens.is_empty() {
            self.fail("当前没有可复制的 Token 样本。", cx);
            return;
        }
        let count = tokens.len();
        cx.write_to_clipboard(ClipboardItem::new_string(tokens.join("\n")));
        self.error = None;
        self.sequencer_pane.status_text = Some(format!("已复制全部 {count} 条样本到剪贴板。"));
        cx.notify();
    }

    pub(in crate::workbench) fn append_sequencer_captured_from_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sequencer_pane.running.is_some() {
            return;
        }
        if let Err(err) = self.sequencer_pane.validate_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.fail("剪贴板中没有可追加的文本样本。", cx);
            return;
        };
        let incoming: Vec<String> = parse_manual_lines(&pasted)
            .into_iter()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if incoming.is_empty() {
            self.fail("剪贴板文本为空，未追加样本。", cx);
            return;
        }
        if self.sequencer_pane.captured_samples.len() + incoming.len() > MAX_SAMPLE_COUNT {
            self.fail(
                format!("追加后样本数量超过 {MAX_SAMPLE_COUNT} 条上限，未修改当前样本。"),
                cx,
            );
            return;
        }
        for (idx, line) in incoming.iter().enumerate() {
            if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&line.len()) {
                self.fail(
                    format!(
                        "剪贴板第 {} 行长度须在 {MIN_TOKEN_BYTES}–{MAX_TOKEN_BYTES} 字节之间。",
                        idx + 1
                    ),
                    cx,
                );
                return;
            }
        }
        let added = incoming.len();
        self.sequencer_pane.captured_samples.extend(incoming);
        let total = self.sequencer_pane.captured_samples.len();
        self.sequencer_pane.capture.tokens_collected = total;
        self.sequencer_pane.capture.latest_samples = self
            .sequencer_pane
            .captured_samples
            .iter()
            .skip(total.saturating_sub(MAX_PREVIEW_SAMPLES))
            .cloned()
            .collect();
        self.sequencer_pane.sync_preview(window, cx);
        self.error = None;
        self.sequencer_pane.status_text = Some(format!(
            "已从剪贴板追加 {added} 条样本（当前共 {total} 条）。"
        ));
        cx.notify();
    }

    pub(in crate::workbench) fn analyze_manual_sequencer_samples(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match editing::snapshot(&self.sequencer_pane.manual_editor, window, cx) {
            Ok(text) => text,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let samples = match validate_manual_samples(text.as_ref()) {
            Ok(samples) => samples,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        self.enqueue_sequencer_analysis(samples, cx);
    }

    pub(in crate::workbench) fn switch_sequencer_request_pretty(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.sequencer_pane.request.switch_pretty(window, cx) {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn switch_sequencer_request_view(
        &mut self,
        view: MessageView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.sequencer_pane.request.switch(view, window, cx) {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn handle_sequencer_search_action(
        &mut self,
        action: SearchBarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let readonly = self.project.is_none()
            || self.transition.is_some()
            || !self.backend_available()
            || self.sequencer_pane.running.is_some()
            || self.sequencer_pane.request.is_readonly_preview();
        if let Err(err) = self
            .sequencer_pane
            .request
            .handle_search_bar_action_editable(action, readonly, window, cx)
        {
            self.fail(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn render_sequencer(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = self.sequencer_pane.page;
        let settings_open = self.sequencer_pane.settings_open;
        let base64_predecode = self.sequencer_pane.base64_predecode;
        let dark = theme::is_dark();

        let live_tab = wb_render_chrome::secondary_tab(
            "sequencer-tab-live",
            "实时采样",
            page == SequencerPage::Live,
            window,
            cx,
        )
        .px(theme::scaled_px(16.))
        .font_weight(if page == SequencerPage::Live {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("Live capture — 切换至实时并发采样视图")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.sequencer_pane.page = SequencerPage::Live;
            cx.notify();
        }));

        let manual_tab = wb_render_chrome::secondary_tab(
            "sequencer-tab-manual",
            "手动加载",
            page == SequencerPage::Manual,
            window,
            cx,
        )
        .px(theme::scaled_px(16.))
        .font_weight(if page == SequencerPage::Manual {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("Manual load — 切换至手动样本粘贴与加载视图")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.sequencer_pane.page = SequencerPage::Manual;
            cx.notify();
        }));

        let show_analysis_tab =
            self.sequencer_pane.analysis.is_some() || page == SequencerPage::Analysis;
        let analysis_tab = show_analysis_tab.then(|| {
            wb_render_chrome::secondary_tab(
                "sequencer-tab-analysis",
                "统计分析",
                page == SequencerPage::Analysis,
                window,
                cx,
            )
            .px(theme::scaled_px(16.))
            .font_weight(if page == SequencerPage::Analysis {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "Analysis — 查看 FIPS 140-2 与信息熵随机性统计分析报告",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.sequencer_pane.page = SequencerPage::Analysis;
                cx.notify();
            }))
        });

        let settings_hover_bg: gpui_kit::Hsla = if dark {
            rgb(0x2f3237).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let settings_active_bg: gpui_kit::Hsla = if dark {
            rgb(0x35383d).into()
        } else {
            rgb(0xeeeeee).into()
        };
        let settings_btn = flat_button("sequencer-settings", true, window, cx)
            .accessibility_label("Sequencer settings")
            .h(theme::secondary_tab_height() - px(1.))
            .self_start()
            .flex_shrink_0()
            .px(theme::scaled_px(10.))
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .font_weight(if settings_open {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .line_height(theme::scaled_px(17.))
            .text_color(if settings_open && !dark {
                rgb(0x000000).into()
            } else {
                theme::foreground()
            })
            .flex()
            .items_center()
            .gap(theme::scaled_px(6.))
            .bg(if settings_open {
                settings_active_bg
            } else {
                theme::toolbar_surface()
            })
            .hover(move |s| s.bg(settings_hover_bg))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "展开或收起 Sequencer 设置（采样样本数、并发数与 Base64 预解码选项）",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.sequencer_pane.settings_open = !this.sequencer_pane.settings_open;
                cx.notify();
            }))
            .child(
                Icon::new(IconName::Settings)
                    .size(px(16.))
                    .text_color(if base64_predecode {
                        theme::accent()
                    } else if dark {
                        theme::foreground()
                    } else {
                        rgb(0x1f1f1f).into()
                    }),
            )
            .child("Sequencer 设置")
            .when(base64_predecode, |btn| {
                btn.child(div().size(px(6.)).rounded(px(3.)).bg(theme::accent()))
            });

        let can_edit_config = self.sequencer_can_operate() && self.sequencer_pane.running.is_none();
        let capture_panel_open = self.sequencer_pane.capture_panel_open;
        let settings_panel = settings_open.then(|| {
            let focus = window
                .use_keyed_state("sequencer-base64-predecode", cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone();
            let base64_checkbox = Checkbox::new("sequencer-base64-predecode")
                .accessibility_label("Base64 预解码")
                .checked(base64_predecode)
                .track_focus(&focus)
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(2.))
                .py(px(2.))
                .rounded(px(3.))
                .text_size(theme::ui_font_size())
                .text_color(theme::foreground())
                .when(focus.is_focused(window), |this| this.bg(theme::selected()))
                .child(
                    CheckboxIndicator::new()
                        .checked(base64_predecode)
                        .size(px(14.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(if base64_predecode {
                            theme::accent()
                        } else {
                            theme::control_border()
                        })
                        .bg(if base64_predecode {
                            theme::accent()
                        } else {
                            theme::surface()
                        })
                        .rounded(px(3.))
                        .when(base64_predecode, |this| {
                            this.child(
                                Icon::new(IconName::Check)
                                    .size(px(11.))
                                    .text_color(rgb(0xffffff)),
                            )
                        }),
                )
                .child("Base64 预解码")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.sequencer_pane.base64_predecode = !this.sequencer_pane.base64_predecode;
                    cx.notify();
                }));

            let toggle_capture_panel_btn = sequencer_stack_button(
                "sequencer-toggle-capture-panel",
                if capture_panel_open {
                    "收起采样面板"
                } else {
                    "采样进度面板"
                },
                true,
                window,
                cx,
            )
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "显示或隐藏实时采样进度与样本导入/导出面板",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.sequencer_pane.capture_panel_open = !this.sequencer_pane.capture_panel_open;
                cx.notify();
            }));

            div()
                .id("sequencer-settings-panel")
                .test_support()
                .w_full()
                .px(px(16.))
                .py(px(7.))
                .flex_shrink_0()
                .bg(theme::toolbar_surface())
                .border_b_1()
                .border_color(theme::border())
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(theme::scaled_px(12.5))
                        .text_color(theme::foreground())
                        .child("目标样本数："),
                )
                .child(
                    div().w(px(78.)).flex_shrink_0().child(
                        Input::new(&self.sequencer_pane.target_samples)
                            .id("sequencer-target-samples")
                            .aria_label("目标样本数")
                            .small()
                            .disabled(!can_edit_config),
                    ),
                )
                .child(
                    div()
                        .ml(px(4.))
                        .text_size(theme::scaled_px(12.5))
                        .text_color(theme::foreground())
                        .child("并发数："),
                )
                .child(
                    div().w(px(56.)).flex_shrink_0().child(
                        Input::new(&self.sequencer_pane.concurrency)
                            .id("sequencer-concurrency")
                            .aria_label("并发请求数")
                            .small()
                            .disabled(!can_edit_config),
                    ),
                )
                .child(div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(theme::border()))
                .child(base64_checkbox)
                .child(div().w(px(1.)).h(px(18.)).mx(px(4.)).bg(theme::border()))
                .child(toggle_capture_panel_btn)
                .child(div().flex_1())
                .child(
                    flat_button("sequencer-settings-close", false, window, cx)
                        .accessibility_label("关闭 Sequencer 设置")
                        .size(px(22.))
                        .rounded(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(move |s| s.bg(settings_hover_bg))
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(13.))
                                .text_color(theme::muted_foreground()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sequencer_pane.settings_open = false;
                            cx.notify();
                        })),
                )
        });

        let content = match page {
            SequencerPage::Live => self.render_sequencer_live(window, cx),
            SequencerPage::Manual => self.render_sequencer_manual(window, cx),
            SequencerPage::Analysis => self.render_sequencer_analysis(window, cx),
        };

        div()
            .id("sequencer-root")
            .test_support()
            .size_full()
            .bg(theme::surface())
            .flex()
            .flex_col()
            .child(
                wb_render_chrome::secondary_tab_bar()
                    .child(live_tab)
                    .child(manual_tab)
                    .children(analysis_tab)
                    .child(
                        div()
                            .w(px(20.))
                            .h(theme::secondary_tab_height() - px(1.))
                            .self_start()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().w(px(1.)).h(px(22.)).bg(theme::border())),
                    )
                    .child(settings_btn)
                    .child(div().flex_1())
                    .when_some(self.sequencer_pane.status_text.clone(), |bar, status| {
                        bar.child(
                            div()
                                .id("sequencer-status-banner")
                                .test_support()
                                .role(Role::Status)
                                .aria_label(status.clone())
                                .max_w(px(460.))
                                .pr(px(12.))
                                .truncate()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child(status),
                        )
                    }),
            )
            .children(settings_panel)
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }
}

pub(in crate::workbench) fn sequencer_table_header_bg() -> gpui_kit::Hsla {
    rgb(if theme::is_dark() { 0x25272b } else { 0xf1f1f1 }).into()
}

pub(in crate::workbench) fn sequencer_divider() -> gpui_kit::Div {
    div()
        .w_full()
        .h(px(1.))
        .my(px(12.))
        .flex_shrink_0()
        .bg(theme::border())
}

pub(in crate::workbench) fn sequencer_help_icon(
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
) -> AnyElement {
    let dark = theme::is_dark();
    Button::new(id)
        .ghost()
        .compact()
        .p_0()
        .size(px(20.))
        .child(
            Icon::new(IconName::CircleQuestionMark)
                .size(px(18.))
                .text_color(if dark {
                    theme::foreground()
                } else {
                    rgb(0x1f1f1f).into()
                }),
        )
        .accessibility_label(label)
        .tooltip(tooltip)
        .into_any_element()
}

pub(in crate::workbench) fn sequencer_section(
    help_id: &'static str,
    help_label: &'static str,
    help_tooltip: &'static str,
    title: &'static str,
    subtitle: Option<&'static str>,
    body: AnyElement,
) -> AnyElement {
    let dark = theme::is_dark();
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .w(px(20.))
                .pt(px(1.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(sequencer_help_icon(help_id, help_label, help_tooltip)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(theme::scaled_px(14.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if dark {
                            theme::foreground()
                        } else {
                            rgb(0x000000).into()
                        })
                        .child(title),
                )
                .when_some(subtitle, |col, text| {
                    col.child(
                        div()
                            .mt(px(5.))
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child(text),
                    )
                })
                .child(div().mt(px(10.)).w_full().child(body)),
        )
        .into_any_element()
}

pub(in crate::workbench) fn sequencer_list_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label: SharedString = label.into();
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = if dark {
        rgb(0x34373c).into()
    } else {
        rgb(0xefefef).into()
    };
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(0x3e4248).into()
    } else {
        rgb(0xe3e3e3).into()
    };
    let border: gpui_kit::Hsla = if dark {
        rgb(0x4e535b).into()
    } else {
        rgb(0xd4d4d4).into()
    };
    let fg: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x1a1a1a).into()
    };
    flat_button(id, false, window, cx)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(theme::scaled_px(26.))
        .px(theme::scaled_px(12.))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .when(enabled, |btn| btn.hover(move |s| s.bg(hover_bg)))
        .font_family(theme::ui_font_family())
        .text_size(theme::scaled_px(12.5))
        .font_weight(FontWeight::BOLD)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(label)
}

pub(in crate::workbench) fn sequencer_stack_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label: SharedString = label.into();
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = if dark {
        rgb(0x34373c).into()
    } else {
        rgb(0xefefef).into()
    };
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(0x3e4248).into()
    } else {
        rgb(0xe3e3e3).into()
    };
    let border: gpui_kit::Hsla = if dark {
        rgb(0x4e535b).into()
    } else {
        rgb(0xd4d4d4).into()
    };
    let fg: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x1a1a1a).into()
    };
    flat_button(id, false, window, cx)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(theme::scaled_px(26.))
        .px(theme::scaled_px(12.))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .when(enabled, |btn| btn.hover(move |s| s.bg(hover_bg)))
        .font_family(theme::ui_font_family())
        .text_size(theme::scaled_px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .styles(|styles| styles.disabled(|style| style.opacity(0.55)))
        .child(label)
}

pub(in crate::workbench) fn sequencer_outline_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label: SharedString = label.into();
    let dark = theme::is_dark();
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(0x34373c).into()
    } else {
        rgb(0xf2f2f2).into()
    };
    let border: gpui_kit::Hsla = if dark {
        rgb(0x4e535b).into()
    } else {
        rgb(0xd2d2d2).into()
    };
    let fg: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x1a1a1a).into()
    };
    flat_button(id, false, window, cx)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(theme::scaled_px(26.))
        .px(theme::scaled_px(14.))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(theme::surface())
        .when(enabled, |btn| btn.hover(move |s| s.bg(hover_bg)))
        .font_family(theme::ui_font_family())
        .text_size(theme::scaled_px(12.5))
        .font_weight(FontWeight::BOLD)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(label)
}

pub(in crate::workbench) fn sequencer_primary_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::base::Button {
    let label: SharedString = label.into();
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = if enabled {
        rgb(0xdb612f).into()
    } else if dark {
        rgb(0x35383d).into()
    } else {
        rgb(0xdfdfdf).into()
    };
    let fg: gpui_kit::Hsla = if enabled {
        rgb(0xffffff).into()
    } else if dark {
        rgb(0x888d96).into()
    } else {
        rgb(0x555555).into()
    };
    flat_button(id, false, window, cx)
        .accessibility_label(label.clone())
        .disabled(!enabled)
        .h(theme::scaled_px(28.))
        .min_w(theme::scaled_px(136.))
        .px(theme::scaled_px(18.))
        .rounded(px(3.))
        .bg(bg)
        .when(!enabled, |btn| {
            btn.border_1()
                .border_color(rgb(if dark { 0x454950 } else { 0xd4d4d4 }))
        })
        .when(enabled, |btn| btn.hover(|s| s.bg(rgb(0xc55426))))
        .font_family(theme::ui_font_family())
        .text_size(theme::scaled_px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .child(label)
}

pub(in crate::workbench) fn sequencer_radio(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> Radio {
    let focus = window
        .use_keyed_state(id, cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    let dark = theme::is_dark();
    let ring_border: gpui_kit::Hsla = if !enabled {
        if dark {
            rgb(0x4b4f57).into()
        } else {
            rgb(0xd4d4d4).into()
        }
    } else if checked {
        theme::accent()
    } else if dark {
        rgb(0x5c616a).into()
    } else {
        rgb(0xb8b8b8).into()
    };
    let dot_bg: gpui_kit::Hsla = if !enabled {
        if dark {
            rgb(0x666b75).into()
        } else {
            rgb(0xc6c6c6).into()
        }
    } else {
        theme::accent()
    };
    Radio::new(id)
        .accessibility_label(label)
        .checked(checked)
        .disabled(!enabled)
        .track_focus(&focus)
        .w(theme::scaled_px(116.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .min_h(theme::scaled_px(24.))
        .py(px(1.))
        .gap(px(8.))
        .text_size(theme::ui_font_size())
        .font_weight(FontWeight::NORMAL)
        .text_color(theme::foreground())
        .rounded(px(3.))
        .when(enabled && focus.is_focused(window), |this| {
            this.bg(theme::selected())
        })
        .child(
            div()
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .border_1()
                .border_color(ring_border)
                .bg(theme::surface())
                .when(checked, |this| {
                    this.child(div().size(px(6.)).rounded(px(3.)).bg(dot_bg))
                }),
        )
        .child(label)
}

pub(in crate::workbench) fn render_sequencer_bit_card(
    report: &Rc<SequencerAnalysis>,
) -> impl IntoElement {
    let bits = &report.bit_analysis;
    let bit_summary = format!(
        "可用比特：{} · 已分析比特：{} · 1 占比：{:.2}%（{} 个 1） · 整体二元熵：{:.4} bits · 相邻自相关：{:.4}",
        bits.available_bits,
        bits.analyzed_bits,
        bits.ones_ratio * 100.0,
        bits.ones_count,
        bits.shannon_entropy_bits,
        bits.adjacent_autocorrelation
    );
    let bit_count = bits
        .position_ones_ratio
        .len()
        .min(bits.position_shannon_entropy_bits.len());
    let row_h = theme::scaled_px(24.);
    let list_h = px((bit_count as f32 * row_h.as_f32()).min(200.));
    let report_rc = report.clone();
    let bit_rows = uniform_list("sequencer-bit-table-rows", bit_count, move |range, _, _| {
        let bits = &report_rc.bit_analysis;
        range
            .filter_map(|bit_pos| {
                let &ones_ratio = bits.position_ones_ratio.get(bit_pos)?;
                let &entropy = bits.position_shannon_entropy_bits.get(bit_pos)?;
                Some((bit_pos, ones_ratio, entropy))
            })
            .map(|(bit_pos, ones_ratio, entropy)| {
                div()
                    .id(("sequencer-bit-row", bit_pos))
                    .test_support()
                    .role(Role::Row)
                    .w_full()
                    .h(theme::scaled_px(24.))
                    .px(px(8.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .w(theme::scaled_px(64.))
                            .flex_shrink_0()
                            .truncate()
                            .child(format!("Bit #{bit_pos}")),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(110.))
                            .flex_shrink_0()
                            .truncate()
                            .child(format!("{:.2}%", ones_ratio * 100.0)),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(120.))
                            .flex_shrink_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{entropy:.4} bits")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(90.))
                            .h(px(8.))
                            .rounded(px(2.))
                            .bg(rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::relative(entropy.clamp(0.0, 1.0) as f32))
                                    .bg(theme::accent()),
                            ),
                    )
            })
            .collect::<Vec<_>>()
    })
    .test_support()
    .w_full()
    .h(list_h);

    let dark = theme::is_dark();
    div()
        .id("sequencer-bit-card")
        .test_support()
        .w_full()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .w(px(20.))
                .pt(px(1.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(sequencer_help_icon(
                    "sequencer-help-bit",
                    "比特级随机性分析说明",
                    "展示每个比特位的 1 占比与二元香农熵，识别固定或偏斜比特位。",
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .text_size(theme::scaled_px(14.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(if dark {
                                    theme::foreground()
                                } else {
                                    rgb(0x000000).into()
                                })
                                .child("比特级随机性分析 (Bit-level analysis)"),
                        )
                        .child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child(format!("共 {} 个比特位", bits.position_ones_ratio.len())),
                        ),
                )
                .child(
                    div()
                        .id("sequencer-bit-summary")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(bit_summary.clone())
                        .text_size(theme::scaled_px(12.5))
                        .text_color(theme::foreground())
                        .child(bit_summary),
                )
                .child(
                    div()
                        .w_full()
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .min_h(theme::scaled_px(24.))
                                .px(px(8.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .bg(sequencer_table_header_bg())
                                .border_b_1()
                                .border_color(theme::border())
                                .text_size(theme::scaled_px(12.))
                                .child(
                                    div()
                                        .w(theme::scaled_px(64.))
                                        .flex_shrink_0()
                                        .child("比特位"),
                                )
                                .child(
                                    div()
                                        .w(theme::scaled_px(110.))
                                        .flex_shrink_0()
                                        .child("1 占比"),
                                )
                                .child(
                                    div()
                                        .w(theme::scaled_px(120.))
                                        .flex_shrink_0()
                                        .child("二元熵"),
                                )
                                .child(div().flex_1().min_w(px(90.)).child("熵值比例")),
                        )
                        .child(bit_rows),
                ),
        )
}
