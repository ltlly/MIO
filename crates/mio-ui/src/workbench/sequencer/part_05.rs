use gpui_kit::WeakEntity;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::uniform_list;

use super::*;

fn sequencer_analysis_panel_menu(
    view: WeakEntity<Workbench>,
    menu: PopupMenu,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(owner) = view.upgrade() else {
        return menu;
    };
    let state = owner.read(cx);
    let has_report = state.sequencer_pane.analysis.is_some();
    let can_export = has_report && state.sequencer_can_operate() && !state.file_picker_pending();
    let copy_view = view.clone();
    let export_view = view;
    menu.item(
        PopupMenuItem::new("复制完整分析报告")
            .disabled(!has_report)
            .on_click(move |_, _, cx| {
                let _ = copy_view.update(cx, |v, cx| v.copy_sequencer_analysis_report(cx));
            }),
    )
    .item(
        PopupMenuItem::new("导出分析结果…")
            .disabled(!can_export)
            .on_click(move |_, window, cx| {
                let _ = export_view.update(cx, |v, cx| {
                    v.export_sequencer_analysis_to_file(window, cx)
                });
            }),
    )
}

impl Workbench {
    pub(in crate::workbench) fn render_sequencer_analysis(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = &self.sequencer_pane;
        let dark = theme::is_dark();
        let Some(report_rc) = pane.analysis.as_ref() else {
            let go_live_btn = sequencer_stack_button(
                "sequencer-go-live",
                "前往实时采样",
                true,
                window,
                cx,
            )
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "切换到实时采样视图配置并启动并发 Token 采集",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.sequencer_pane.page = SequencerPage::Live;
                cx.notify();
            }));

            let go_manual_btn = sequencer_stack_button(
                "sequencer-go-manual",
                "前往手动加载",
                true,
                window,
                cx,
            )
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "切换到手动加载视图粘贴或输入外部 Token 样本集",
                )
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.sequencer_pane.page = SequencerPage::Manual;
                cx.notify();
            }));

            let empty_menu_weak = cx.weak_entity();
            return div()
                .id("sequencer-analysis-empty")
                .test_support()
                .size_full()
                .bg(theme::surface())
                .px(px(16.))
                .py(px(12.))
                .flex()
                .flex_col()
                .context_menu(move |menu, _, cx| {
                    sequencer_analysis_panel_menu(empty_menu_weak.clone(), menu, cx)
                })
                .child(sequencer_section(
                    "sequencer-help-analysis",
                    "随机性统计分析帮助",
                    "执行统计分析后，此处将展示 Token 的有效熵评估、FIPS 140-2 四项检验结果、字符级逐位置熵表及比特级分析。",
                    "尚无 Sequencer 随机性统计分析报告",
                    Some("请先在“实时采样”采集至少 2 条 Token，或在“手动加载”输入至少 2 条 Token 后执行统计分析。"),
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .when_some(pane.analysis_error.clone(), |this, err| {
                            this.child(
                                div()
                                    .id("sequencer-analysis-error")
                                    .test_support()
                                    .role(Role::Alert)
                                    .aria_label(err.clone())
                                    .text_color(rgb(if dark { 0xf28b82 } else { 0xb42318 }))
                                    .child(format!("上次分析未完成：{err}")),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(go_live_btn)
                                .child(go_manual_btn),
                        )
                        .into_any_element(),
                ))
                .into_any_element();
        };
        let report = report_rc.as_ref();

        let (rating_text, badge_bg, badge_fg) = quality_badge(report.quality);
        let entropy_label = format!("{:.1} bits", report.effective_entropy_bits);
        let length_range = if report.min_length == report.max_length {
            format!("{} B", report.min_length)
        } else {
            format!("{}–{} B", report.min_length, report.max_length)
        };
        let encoding_text = encoding_label(report.encoding);
        let max_char_bits = max_theoretical_char_bits(report.encoding, report.charset_size);

        let export_report_btn = sequencer_outline_button(
            "sequencer-export-analysis-file",
            "导出报告 (JSON)…",
            self.sequencer_can_operate() && !self.file_picker_pending(),
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("将当前统计分析报告导出为本地 JSON 文件")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.export_sequencer_analysis_to_file(window, cx);
        }));

        let summary_card = div()
            .id("sequencer-summary-card")
            .test_support()
            .role(Role::Region)
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
                        "sequencer-help-summary",
                        "总体摘要帮助",
                        "汇总样本数量、识别编码、有效熵估值与整体随机性评级。",
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
                            .flex_wrap()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(14.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if dark {
                                        theme::foreground()
                                    } else {
                                        rgb(0x000000).into()
                                    })
                                    .child("核心摘要 (Overall summary)"),
                            )
                            .child(
                                div()
                                    .id("sequencer-quality-badge")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(rating_text)
                                    .px(px(8.))
                                    .py(px(2.))
                                    .rounded(px(2.))
                                    .bg(rgb(badge_bg))
                                    .text_color(rgb(badge_fg))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(rating_text),
                            )
                            .child(
                                div()
                                    .id("sequencer-effective-entropy")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(format!("有效熵 {entropy_label}"))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("有效熵：{entropy_label}")),
                            )
                            .child(format!("样本总数：{}", report.sample_count))
                            .child(format!("长度范围：{length_range}"))
                            .child(format!("字符集大小：{}", report.charset_size))
                            .child(format!("识别编码：{encoding_text}"))
                            .child(div().flex_1())
                            .child(export_report_btn),
                    )
                    .child(
                        div()
                            .id("sequencer-diagnosis")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(report.diagnosis.clone())
                            .text_size(theme::scaled_px(12.5))
                            .text_color(theme::foreground())
                            .child(report.diagnosis.clone()),
                    ),
            );

        let fips_panel = match &report.bit_analysis.fips {
            FipsStatus::Pass => {
                let text = format!(
                    "Pass (4/4 全部通过) · 已检验前 {} bits（Monobit / Poker / Runs / Long run (长游程) 均在标准区间内）",
                    MIN_FIPS_TEST_BITS
                );
                div()
                    .id("sequencer-fips-panel")
                    .test_support()
                    .aria_label(text.clone())
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
                                "sequencer-help-fips",
                                "FIPS 140-2 检验帮助",
                                "基于前 20,000 比特执行 Monobit、Poker、Runs 与 Long run 四项标准统计检验。",
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(14.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if dark {
                                        theme::foreground()
                                    } else {
                                        rgb(0x000000).into()
                                    })
                                    .child("FIPS 140-2 四项随机性检验"),
                            )
                            .child(
                                div()
                                    .id("sequencer-fips-status")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label("Pass (4/4 全部通过)")
                                    .text_color(rgb(if dark { 0x81c995 } else { 0x137333 }))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(text),
                            ),
                    )
            }
            FipsStatus::InsufficientBits { available_bits } => {
                let text = format!(
                    "样本比特不足：当前可用比特数 {available_bits} bits < {MIN_FIPS_TEST_BITS} bits，无法执行 FIPS 140-2 检验。"
                );
                div()
                    .id("sequencer-fips-panel")
                    .test_support()
                    .aria_label(text.clone())
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
                                "sequencer-help-fips",
                                "FIPS 140-2 检验帮助",
                                "至少需要 20,000 可用比特才能执行标准 FIPS 140-2 四项统计检验。",
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(14.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if dark {
                                        theme::foreground()
                                    } else {
                                        rgb(0x000000).into()
                                    })
                                    .child("FIPS 140-2 四项随机性检验"),
                            )
                            .child(
                                div()
                                    .id("sequencer-fips-status")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(text.clone())
                                    .text_color(rgb(if dark { 0xfdd663 } else { 0xb06000 }))
                                    .child(text),
                            ),
                    )
            }
            FipsStatus::Failed { failures } => {
                let headline = format!("Failed（{} / 4 项未通过）", failures.len());
                let mut body_col = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if dark {
                                theme::foreground()
                            } else {
                                rgb(0x000000).into()
                            })
                            .child("FIPS 140-2 四项随机性检验"),
                    )
                    .child(
                        div()
                            .id("sequencer-fips-status")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(headline.clone())
                            .text_color(rgb(if dark { 0xf28b82 } else { 0xc5221f }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(headline.clone()),
                    );
                for (idx, failure) in failures.iter().enumerate() {
                    let detail = format!(
                        "{} — 实测：{} | 标准区间：{}",
                        fips_test_label(failure.test),
                        failure.observed,
                        failure.requirement
                    );
                    body_col = body_col.child(
                        div()
                            .id(("sequencer-fips-failure", idx))
                            .test_support()
                            .aria_label(detail.clone())
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(2.))
                            .bg(rgb(if dark { 0x4a2020 } else { 0xfce8e6 }))
                            .text_color(rgb(if dark { 0xf28b82 } else { 0x8c1d18 }))
                            .child(detail),
                    );
                }
                div()
                    .id("sequencer-fips-panel")
                    .test_support()
                    .aria_label(headline)
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
                                "sequencer-help-fips",
                                "FIPS 140-2 检验帮助",
                                "未通过的 FIPS 140-2 检验项表明 Token 比特流存在统计学偏斜或可预测特征。",
                            )),
                    )
                    .child(body_col)
            }
        };

        let char_count = report.character_analysis.len();
        let row_h = theme::scaled_px(24.);
        let list_h = px((char_count as f32 * row_h.as_f32()).min(200.));
        let char_report = report_rc.clone();
        let char_rows = uniform_list(
            "sequencer-char-table-rows",
            char_count,
            move |range, _, _| {
                range
                    .filter_map(|idx| char_report.character_analysis.get(idx))
                    .map(|item| {
                        let bar_ratio =
                            (item.effective_entropy_bits / max_char_bits).clamp(0.0, 1.0);
                        div()
                            .id(("sequencer-char-row", item.position))
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
                                    .w(theme::scaled_px(48.))
                                    .flex_shrink_0()
                                    .truncate()
                                    .child(format!("#{}", item.position)),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(72.))
                                    .flex_shrink_0()
                                    .truncate()
                                    .child(item.samples_with_position.to_string()),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(82.))
                                    .flex_shrink_0()
                                    .truncate()
                                    .child(item.observed_characters.to_string()),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(96.))
                                    .flex_shrink_0()
                                    .truncate()
                                    .child(format!("{:.3}", item.shannon_entropy_bits)),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(88.))
                                    .flex_shrink_0()
                                    .truncate()
                                    .child(format!("{:.2}", item.chi_square)),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(92.))
                                    .flex_shrink_0()
                                    .truncate()
                                    .child(format!("{:.4}", item.p_value)),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(96.))
                                    .flex_shrink_0()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{:.3}", item.effective_entropy_bits)),
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
                                            .w(gpui_kit::relative(bar_ratio as f32))
                                            .bg(theme::accent()),
                                    ),
                            )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .test_support()
        .w_full()
        .h(list_h);

        let char_card = div()
            .id("sequencer-char-card")
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
                        "sequencer-help-char",
                        "字符级分析帮助",
                        "按字符位置统计样本覆盖数、唯一字符数、香农熵、卡方值与 1% 显著性有效熵。",
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
                                    .child("字符级逐位置熵表 (Character-level analysis)"),
                            )
                            .child(
                                div()
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(theme::muted_foreground())
                                    .child(format!("共 {} 个字符位置", report.character_analysis.len())),
                            ),
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
                                            .w(theme::scaled_px(48.))
                                            .flex_shrink_0()
                                            .child("位置 #"),
                                    )
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.))
                                            .flex_shrink_0()
                                            .child("样本数"),
                                    )
                                    .child(
                                        div()
                                            .w(theme::scaled_px(82.))
                                            .flex_shrink_0()
                                            .child("字符种类"),
                                    )
                                    .child(
                                        div()
                                            .w(theme::scaled_px(96.))
                                            .flex_shrink_0()
                                            .child("Shannon (b)"),
                                    )
                                    .child(
                                        div()
                                            .w(theme::scaled_px(88.))
                                            .flex_shrink_0()
                                            .child("χ²"),
                                    )
                                    .child(
                                        div()
                                            .w(theme::scaled_px(92.))
                                            .flex_shrink_0()
                                            .child("p-value"),
                                    )
                                    .child(
                                        div()
                                            .w(theme::scaled_px(96.))
                                            .flex_shrink_0()
                                            .child("1% 有效熵"),
                                    )
                                    .child(div().flex_1().min_w(px(90.)).child("有效熵分布")),
                            )
                            .child(char_rows),
                    ),
            );

        let bit_card = render_sequencer_bit_card(report_rc);

        let compact_divider = || {
            div()
                .w_full()
                .h(px(1.))
                .my(px(10.))
                .flex_shrink_0()
                .bg(theme::border())
        };

        let menu_weak = cx.weak_entity();
        div()
            .id("sequencer-analysis-scroll")
            .test_support()
            .size_full()
            .bg(theme::surface())
            .overflow_y_scroll()
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_col()
            .context_menu(move |menu, _, cx| {
                sequencer_analysis_panel_menu(menu_weak.clone(), menu, cx)
            })
            .child(summary_card)
            .child(compact_divider())
            .child(fips_panel)
            .child(compact_divider())
            .child(char_card)
            .child(compact_divider())
            .child(bit_card)
            .into_any_element()
    }
}
