use super::widgets::{sequencer_help_icon, sequencer_primary_button, sequencer_stack_button};
use super::*;

#[derive(Clone, Copy, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
struct ClearSequencerManualEditor;

fn sequencer_manual_editor_context_menu(
    input: Entity<EditorState>,
    readonly: bool,
) -> impl Fn(
    gpui_kit::component::native_menu::NativeMenu,
    &mut Window,
    &mut App,
) -> gpui_kit::component::native_menu::NativeMenu
+ 'static {
    use gpui_kit::component::input::{Copy, Cut, Paste, SelectAll};
    move |menu, window, cx| {
        input.update(cx, |state, cx| state.focus(window, cx));
        let selected = !input.read(cx).selected_range().is_empty();
        let empty = input.read(cx).value().is_empty();
        menu.menu_with_disabled("剪切", readonly || !selected, Box::new(Cut))
            .menu_with_disabled("复制", !selected, Box::new(Copy))
            .menu_with_disabled("粘贴", readonly, Box::new(Paste))
            .separator()
            .menu_with_disabled("全选", empty, Box::new(SelectAll))
            .menu_with_disabled(
                "清空",
                readonly || empty,
                Box::new(ClearSequencerManualEditor),
            )
    }
}

impl SequencerView {
    pub(super) fn render_manual(
        &self,
        wb_entity: &Entity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let (editable, file_picker_pending, manual_editor, stats) = {
            let wb = wb_entity.read(cx);
            let pane = &wb.sequencer_pane;
            let editable = wb.sequencer_can_operate() && pane.pending_analysis.is_none();
            let raw_text = pane.manual_editor.read(cx).value();
            let stats = compute_manual_stats(raw_text.as_ref());
            (
                editable,
                wb.file_picker_pending(),
                pane.manual_editor.clone(),
                stats,
            )
        };
        let summary = format!(
            "有效行数：{} · 最小长度：{} B · 最大长度：{} B · 总计：{} B",
            stats.count, stats.min_len, stats.max_len, stats.total_bytes
        );

        let paste_btn = sequencer_stack_button(
            "sequencer-paste-clipboard",
            "粘贴",
            editable,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("从剪贴板按行追加样本").build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let _ = this.workbench.update(cx, |wb, cx| {
                wb.paste_sequencer_manual_clipboard(window, cx)
            });
            cx.notify();
        }));

        let load_btn = sequencer_stack_button(
            "sequencer-import-manual-file",
            "从文件加载…",
            editable && !file_picker_pending,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("从本地文件加载 Token 样本到手动列表")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let _ = this.workbench.update(cx, |wb, cx| {
                wb.import_sequencer_samples_from_file(true, window, cx)
            });
            cx.notify();
        }));

        let copy_btn = sequencer_stack_button(
            "sequencer-copy-manual",
            "复制全部",
            editable && stats.count > 0,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("复制全部手动样本到剪贴板").build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let _ = this
                .workbench
                .update(cx, |wb, cx| wb.copy_all_sequencer_samples(window, cx));
            cx.notify();
        }));

        let export_btn = sequencer_stack_button(
            "sequencer-export-manual-file",
            "导出样本…",
            editable && stats.count > 0 && !file_picker_pending,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("将手动样本保存到本地文本文件")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let _ = this
                .workbench
                .update(cx, |wb, cx| wb.export_sequencer_samples_to_file(window, cx));
            cx.notify();
        }));

        let clear_btn = sequencer_stack_button(
            "sequencer-clear-samples",
            "清空",
            editable,
            window,
            cx,
        )
        .w_full()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("清空手动样本列表").build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let _ = this
                .workbench
                .update(cx, |wb, cx| wb.clear_sequencer_manual_samples(window, cx));
            cx.notify();
        }));

        let analyze_btn = sequencer_primary_button(
            "sequencer-analyze-manual",
            "立即统计分析",
            editable,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("对当前样本执行随机性统计分析")
                .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            let _ = this.workbench.update(cx, |wb, cx| {
                wb.analyze_manual_sequencer_samples(window, cx)
            });
            cx.notify();
        }));

        let editor_border: gpui_kit::Hsla = if dark {
            theme::border()
        } else {
            rgb(0xd0d0d0).into()
        };

        div()
            .id("sequencer-manual-root")
            .test_support()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(theme::surface())
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
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
                                "sequencer-help-manual",
                                "手动加载帮助",
                                "每行输入一条 Token（自动忽略首尾空行），支持从剪贴板或文本文件批量加载样本并执行统计分析。",
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
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
                                    .child("手动加载 Token 样本"),
                            )
                            .child(
                                div()
                                    .mt(px(5.))
                                    .text_size(theme::ui_font_size())
                                    .text_color(theme::foreground())
                                    .child("从剪贴板粘贴或从文件加载待测 Token（每行一条，自动忽略首尾空行），然后点击“立即统计分析”。"),
                            )
                            .child(
                                div()
                                    .mt(px(10.))
                                    .flex_1()
                                    .min_h(px(240.))
                                    .w_full()
                                    .flex()
                                    .items_stretch()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(px(104.))
                                            .flex_shrink_0()
                                            .flex()
                                            .flex_col()
                                            .gap(px(6.))
                                            .child(paste_btn)
                                            .child(load_btn)
                                            .child(copy_btn)
                                            .child(export_btn)
                                            .child(clear_btn),
                                    )
                                    .child(
                                        div()
                                            .id("sequencer-manual-editor")
                                            .test_support()
                                            .flex_1()
                                            .min_w_0()
                                            .min_h_0()
                                            .border_1()
                                            .border_color(editor_border)
                                            .bg(theme::surface())
                                            .on_action(cx.listener(
                                                |this, _: &ClearSequencerManualEditor, window, cx| {
                                                    cx.stop_propagation();
                                                    let _ = this.workbench.update(cx, |wb, cx| {
                                                        wb.clear_sequencer_manual_samples(window, cx)
                                                    });
                                                    cx.notify();
                                                },
                                            ))
                                            .child(
                                                Editor::new(&manual_editor)
                                                    .readonly(!editable)
                                                    .bordered(false)
                                                    .aria_label("Sequencer 手动加载样本编辑器")
                                                    .context_menu(
                                                        sequencer_manual_editor_context_menu(
                                                            manual_editor.clone(),
                                                            !editable,
                                                        ),
                                                    )
                                                    .size_full(),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .mt(px(12.))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(14.))
                                    .child(analyze_btn)
                                    .child(
                                        div()
                                            .id("sequencer-manual-stats")
                                            .test_support()
                                            .role(Role::Status)
                                            .aria_label(summary.clone())
                                            .text_size(theme::scaled_px(12.5))
                                            .text_color(theme::muted_foreground())
                                            .child(summary),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
