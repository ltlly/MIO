use super::*;
use gpui_kit::base::Radio;
use gpui_kit::component::input::EditorGutterStyle;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarHandle as _, ScrollbarMode};

#[derive(Clone, Copy)]
enum DecoderMenuEntry {
    Op {
        id: &'static str,
        label: &'static str,
        step_label: &'static str,
        op: DecoderOp,
    },
    Separator,
}

const DECODE_MENU_ITEMS: &[DecoderMenuEntry] = &[
    DecoderMenuEntry::Op {
        id: "decoder-url-decode",
        label: "URL 解码",
        step_label: "URL 解码",
        op: DecoderOp::Convert(editing::ConvertKind::UrlDecode),
    },
    DecoderMenuEntry::Op {
        id: "decoder-html-decode",
        label: "HTML 解码",
        step_label: "HTML 解码",
        op: DecoderOp::Convert(editing::ConvertKind::HtmlDecode),
    },
    DecoderMenuEntry::Op {
        id: "decoder-base64-decode",
        label: "Base64 解码",
        step_label: "Base64 解码",
        op: DecoderOp::Convert(editing::ConvertKind::Base64Decode),
    },
    DecoderMenuEntry::Op {
        id: "decoder-base64-url-decode",
        label: "Base64 URL-Safe 解码",
        step_label: "Base64 URL-Safe 解码",
        op: DecoderOp::Convert(editing::ConvertKind::Base64UrlSafeDecode),
    },
    DecoderMenuEntry::Op {
        id: "decoder-hex-decode",
        label: "ASCII Hex 解码",
        step_label: "ASCII Hex 解码",
        op: DecoderOp::Convert(editing::ConvertKind::AsciiHexDecode),
    },
    DecoderMenuEntry::Op {
        id: "decoder-octal-decode",
        label: "八进制解码",
        step_label: "八进制解码",
        op: DecoderOp::Octal(false),
    },
    DecoderMenuEntry::Op {
        id: "decoder-binary-decode",
        label: "二进制解码",
        step_label: "二进制解码",
        op: DecoderOp::Binary(false),
    },
    DecoderMenuEntry::Separator,
    DecoderMenuEntry::Op {
        id: "decoder-gzip-decode",
        label: "Gzip 解压",
        step_label: "Gzip 解压",
        op: DecoderOp::GzipDecompress,
    },
    DecoderMenuEntry::Op {
        id: "decoder-deflate-decode",
        label: "Deflate 解压",
        step_label: "Deflate 解压",
        op: DecoderOp::DeflateDecompress,
    },
    DecoderMenuEntry::Op {
        id: "decoder-brotli-decode",
        label: "Brotli 解压",
        step_label: "Brotli 解压",
        op: DecoderOp::BrotliDecompress,
    },
];

const ENCODE_MENU_ITEMS: &[DecoderMenuEntry] = &[
    DecoderMenuEntry::Op {
        id: "decoder-url-encode",
        label: "URL 编码",
        step_label: "URL 编码",
        op: DecoderOp::Convert(editing::ConvertKind::UrlEncodeKey),
    },
    DecoderMenuEntry::Op {
        id: "decoder-url-encode-all",
        label: "URL 全字符编码",
        step_label: "URL 全字符编码",
        op: DecoderOp::Convert(editing::ConvertKind::UrlEncodeAll),
    },
    DecoderMenuEntry::Op {
        id: "decoder-html-encode",
        label: "HTML 编码",
        step_label: "HTML 编码",
        op: DecoderOp::Convert(editing::ConvertKind::HtmlEncodeKey),
    },
    DecoderMenuEntry::Op {
        id: "decoder-html-encode-decimal",
        label: "HTML 十进制实体",
        step_label: "HTML 十进制实体",
        op: DecoderOp::Convert(editing::ConvertKind::HtmlEncodeDecimal),
    },
    DecoderMenuEntry::Op {
        id: "decoder-html-encode-hex",
        label: "HTML 十六进制实体",
        step_label: "HTML 十六进制实体",
        op: DecoderOp::Convert(editing::ConvertKind::HtmlEncodeHex),
    },
    DecoderMenuEntry::Op {
        id: "decoder-base64-encode",
        label: "Base64 编码",
        step_label: "Base64 编码",
        op: DecoderOp::Convert(editing::ConvertKind::Base64Encode),
    },
    DecoderMenuEntry::Op {
        id: "decoder-base64-url-encode",
        label: "Base64 URL-Safe 编码",
        step_label: "Base64 URL-Safe 编码",
        op: DecoderOp::Convert(editing::ConvertKind::Base64UrlSafeEncode),
    },
    DecoderMenuEntry::Op {
        id: "decoder-hex-encode",
        label: "ASCII Hex 编码",
        step_label: "ASCII Hex 编码",
        op: DecoderOp::Convert(editing::ConvertKind::AsciiHexEncode),
    },
    DecoderMenuEntry::Op {
        id: "decoder-octal-encode",
        label: "八进制编码",
        step_label: "八进制编码",
        op: DecoderOp::Octal(true),
    },
    DecoderMenuEntry::Op {
        id: "decoder-binary-encode",
        label: "二进制编码",
        step_label: "二进制编码",
        op: DecoderOp::Binary(true),
    },
    DecoderMenuEntry::Separator,
    DecoderMenuEntry::Op {
        id: "decoder-gzip-encode",
        label: "Gzip 压缩",
        step_label: "Gzip 压缩",
        op: DecoderOp::GzipCompress,
    },
    DecoderMenuEntry::Op {
        id: "decoder-deflate-encode",
        label: "Deflate 压缩",
        step_label: "Deflate 压缩",
        op: DecoderOp::DeflateCompress,
    },
];

const HASH_MENU_ITEMS: &[DecoderMenuEntry] = &[
    DecoderMenuEntry::Op {
        id: "decoder-sha256",
        label: "SHA-256",
        step_label: "SHA-256 (Hex)",
        op: DecoderOp::Hash("sha256"),
    },
    DecoderMenuEntry::Op {
        id: "decoder-sha384",
        label: "SHA-384",
        step_label: "SHA-384 (Hex)",
        op: DecoderOp::Hash("sha384"),
    },
    DecoderMenuEntry::Op {
        id: "decoder-sha512",
        label: "SHA-512",
        step_label: "SHA-512 (Hex)",
        op: DecoderOp::Hash("sha512"),
    },
    DecoderMenuEntry::Op {
        id: "decoder-sha1",
        label: "SHA-1",
        step_label: "SHA-1 (Hex)",
        op: DecoderOp::Hash("sha1"),
    },
    DecoderMenuEntry::Op {
        id: "decoder-md5",
        label: "MD5",
        step_label: "MD5 (Hex)",
        op: DecoderOp::Hash("md5"),
    },
    DecoderMenuEntry::Op {
        id: "decoder-crc32",
        label: "CRC32",
        step_label: "CRC32 (Hex)",
        op: DecoderOp::Hash("crc32"),
    },
];

fn decoder_view_radio(
    id: &'static str,
    label: &'static str,
    aria_label: &'static str,
    checked: bool,
    output: bool,
    hex_view: bool,
    owner: WeakEntity<Workbench>,
) -> Radio {
    let dark = theme::is_dark();
    let checked_color: gpui_kit::Hsla = rgb(if dark { 0x589df6 } else { 0x285f99 }).into();
    let unchecked_border: gpui_kit::Hsla = rgb(if dark { 0x5c616a } else { 0xbcbcbc }).into();
    Radio::new(id)
        .accessibility_label(aria_label)
        .checked(checked)
        .tab_stop(false)
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .text_color(theme::foreground())
        .on_change(move |_, _, window, cx| {
            let _ = owner.update(cx, |this, cx| {
                this.set_decoder_hex_view(output, hex_view, window, cx);
            });
        })
        .child(
            div()
                .size(px(15.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(theme::surface())
                .when(checked, |circle| {
                    circle
                        .border_2()
                        .border_color(checked_color)
                        .child(div().size(px(7.)).rounded_full().bg(checked_color))
                })
                .when(!checked, |circle| {
                    circle.border_1().border_color(unchecked_border)
                }),
        )
        .child(label)
}

fn decoder_help_popover() -> Popover {
    let icon_fg: gpui_kit::Hsla = rgb(if theme::is_dark() { 0xdfe1e5 } else { 0x141414 }).into();
    Popover::new("decoder-help-popover")
        .anchor(gpui_kit::Anchor::TopRight)
        .trigger(
            Button::new("decoder-help-button")
                .ghost()
                .compact()
                .p_0()
                .size(theme::scaled_px(22.))
                .min_w(theme::scaled_px(22.))
                .tab_stop(false)
                .accessibility_label("Decoder 帮助")
                .tooltip("查看 Decoder 使用说明")
                .child(
                    gpui_kit::component::Icon::new(IconName::CircleQuestionMark)
                        .size(theme::scaled_px(22.))
                        .text_color(icon_fg),
                ),
        )
        .content(|_, _, _| {
            div()
                .w(theme::scaled_px(320.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_size(theme::ui_font_size())
                .text_color(theme::foreground())
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .child("Decoder 编解码与摘要工具"),
                )
                .child("在左侧输入或粘贴待处理数据，通过 Text / Hex 单选框切换文本与十六进制字节视图。")
                .child("使用“解码为…”、“编码为…”、“哈希…”下拉菜单或“智能解码”执行单步与多层自动转换；编辑输入时自动重算当前转换链。")
                .child("转换后可在下方输出阶段查看结果、复制输出、输出转输入继续串联，或点击转换链步骤快速回退。")
        })
}

fn build_decoder_menu(
    menu: PopupMenu,
    entries: &'static [DecoderMenuEntry],
    last_op: Option<DecoderOp>,
    owner: WeakEntity<Workbench>,
    menu_w: gpui_kit::Pixels,
) -> PopupMenu {
    let mut menu = menu
        .compact()
        .min_w(menu_w)
        .max_h(px(460.))
        .scrollable(true);
    for entry in entries {
        menu = match *entry {
            DecoderMenuEntry::Separator => menu.separator(),
            DecoderMenuEntry::Op {
                id,
                label,
                step_label,
                op,
            } => {
                let owner = owner.clone();
                menu.item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .id(id)
                            .test_support()
                            .role(gpui_kit::accesskit::Role::MenuItem)
                            .aria_label(label)
                            .flex_1()
                            .w_full()
                            .min_h(px(22.))
                            .flex()
                            .items_center()
                            .text_size(theme::ui_font_size())
                            .child(label)
                    })
                    .checked(last_op == Some(op))
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.decoder_operation(op, step_label, window, cx);
                        });
                    }),
                )
            }
        };
    }
    menu
}

#[allow(clippy::too_many_arguments)]
fn decoder_dropdown_select(
    id: &'static str,
    label: &'static str,
    tip: &'static str,
    entries: &'static [DecoderMenuEntry],
    last_op: Option<DecoderOp>,
    owner: WeakEntity<Workbench>,
    ctrl_h: gpui_kit::Pixels,
    menu_w: gpui_kit::Pixels,
) -> impl IntoElement {
    let dark = theme::is_dark();
    let select_border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xc6c6c6 }).into();
    let fg: gpui_kit::Hsla = rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e }).into();
    Button::new(id)
        .small()
        .dropdown_caret(true)
        .w_full()
        .h(ctrl_h)
        .px(px(8.))
        .rounded(px(3.))
        .border_color(select_border)
        .accessibility_label(label)
        .tooltip(tip)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(theme::ui_font_size())
                .font_weight(gpui_kit::FontWeight::NORMAL)
                .text_color(fg)
                .child(label),
        )
        .dropdown_menu(move |menu, _, _| {
            build_decoder_menu(menu, entries, last_op, owner.clone(), menu_w)
        })
        .offset(px(2.))
        .trigger_style(gpui_kit::StyleRefinement::default().w_full())
}

fn decoder_action_button(
    id: &'static str,
    label: &'static str,
    tip: &'static str,
    ctrl_h: gpui_kit::Pixels,
) -> gpui_kit::base::Button {
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = rgb(if dark { 0x33363a } else { 0xececec }).into();
    let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x3e4247 } else { 0xe2e2e2 }).into();
    let active_bg: gpui_kit::Hsla = rgb(if dark { 0x4b5056 } else { 0xd8d8d8 }).into();
    let border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xd0d0d0 }).into();
    let fg: gpui_kit::Hsla = rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e }).into();
    gpui_kit::base::Button::new(id)
        .accessibility_label(label)
        .w_full()
        .h(ctrl_h)
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::BOLD)
        .text_color(fg)
        .hover(move |s| s.bg(hover_bg))
        .active(move |s| s.bg(active_bg))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .child(label)
}

fn decoder_editor_context_menu(
    input: Entity<EditorState>,
    readonly: bool,
) -> impl Fn(
    gpui_kit::component::native_menu::NativeMenu,
    &mut Window,
    &mut gpui_kit::App,
) -> gpui_kit::component::native_menu::NativeMenu
+ 'static {
    use gpui_kit::component::input::{Copy, Cut, Paste};
    move |menu, window, cx| {
        input.update(cx, |state, cx| state.focus(window, cx));
        let selected = !input.read(cx).selected_range().is_empty();
        menu.menu_with_disabled("剪切", readonly || !selected, Box::new(Cut))
            .menu_with_disabled("复制", !selected, Box::new(Copy))
            .menu_with_disabled("粘贴", readonly, Box::new(Paste))
    }
}

fn decoder_editor_box(
    state: &Entity<EditorState>,
    readonly: bool,
    aria_label: &'static str,
    context_menu: impl Fn(
        gpui_kit::component::native_menu::NativeMenu,
        &mut Window,
        &mut gpui_kit::App,
    ) -> gpui_kit::component::native_menu::NativeMenu
    + 'static,
) -> impl IntoElement {
    let dark = theme::is_dark();
    let editor_border: gpui_kit::Hsla = rgb(if dark { 0x45494e } else { 0xd2d2d2 }).into();
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(theme::surface())
        .border_1()
        .border_color(editor_border)
        .overflow_hidden()
        .child(
            Editor::new(state)
                .readonly(readonly)
                .bordered(false)
                .line_height(gpui_kit::relative(theme::EDITOR_LINE_HEIGHT))
                .text_area_paddings(gpui_kit::Edges {
                    top: px(5.),
                    right: px(7.),
                    bottom: px(5.),
                    left: px(7.),
                })
                .aria_label(aria_label)
                .context_menu(context_menu)
                .size_full(),
        )
}

fn configure_decoder_editor_state(
    editor: &Entity<EditorState>,
    plain_gutter: EditorGutterStyle,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    editor.update(cx, |state, cx| {
        state.set_line_number(false, window, cx);
        state.set_gutter_style(plain_gutter, cx);
        state.set_scrollbar_renderer(Some(Rc::new(|context, _window, _cx| {
            let axis = context.axis();
            let viewport = context.viewport_bounds();
            let content = context.content_size();
            let has_vertical_overflow = content.height > viewport.size.height + px(1.);
            let has_horizontal_overflow =
                !axis.is_vertical() && content.width > viewport.size.width + px(1.);
            if !has_vertical_overflow && !has_horizontal_overflow {
                return div().into_any_element();
            }
            let active_axis = match (has_vertical_overflow, has_horizontal_overflow) {
                (true, true) => ScrollbarAxis::Both,
                (true, false) => ScrollbarAxis::Vertical,
                (false, true) => ScrollbarAxis::Horizontal,
                (false, false) => unreachable!(),
            };
            let dark = theme::is_dark();
            let track_bg = rgb(if dark { 0x2b2d30 } else { 0xfbfbfb });
            let thumb_bg = rgb(if dark { 0x5c626b } else { 0xdfdfdf });
            let thumb_hover_bg = rgb(if dark { 0x6b717a } else { 0xc9c9c9 });
            let scrollbar = Scrollbar::new(&context)
                .axis(active_axis)
                .mode(ScrollbarMode::Always)
                .styles(|styles| {
                    styles
                        .track(|s| s.width(px(10.)))
                        .track_hover(|s| s.width(px(10.)))
                        .track_active(|s| s.width(px(10.)))
                        .thumb(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_bg)
                        })
                        .thumb_hover(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_hover_bg)
                        })
                        .thumb_active(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_hover_bg)
                        })
                });
            div()
                .size_full()
                .relative()
                .when(has_vertical_overflow, |this| {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom(if has_horizontal_overflow {
                                px(10.)
                            } else {
                                px(0.)
                            })
                            .w(px(10.))
                            .bg(track_bg),
                    )
                })
                .when(has_horizontal_overflow, |this| {
                    this.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(10.))
                            .bg(track_bg),
                    )
                })
                .child(scrollbar)
                .into_any_element()
        })));
    });
}

impl Workbench {
    pub(in crate::workbench) fn render_decoder(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let plain_gutter = EditorGutterStyle::default()
            .left_padding(px(0.))
            .right_margin(px(0.))
            .right_border_width(px(0.))
            .text_left_padding(px(0.));
        for editor in [
            &self.extra_tools.decoder_input,
            &self.extra_tools.decoder_output,
        ] {
            configure_decoder_editor_state(editor, plain_gutter, window, cx);
        }

        let owner = cx.weak_entity();
        let last_op = self.extra_tools.steps.last().and_then(|step| step.op);
        let row1_h = theme::scaled_px(23.).max(px(20.));
        let ctrl_h = theme::scaled_px(24.).max(px(21.));
        let ctrl_gap = px(4.);
        let ctrl_col_w = theme::scaled_px(198.);
        let stage_h = px(row1_h.as_f32() + ctrl_h.as_f32() * 4. + ctrl_gap.as_f32() * 4.);

        let input_controls = div()
            .w(ctrl_col_w)
            .flex_shrink_0()
            .ml(px(10.))
            .flex()
            .flex_col()
            .gap(ctrl_gap)
            .child(
                div()
                    .w_full()
                    .h(row1_h)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(14.))
                            .child(decoder_view_radio(
                                "decoder-input-text-view",
                                "Text",
                                "输入 Text 视图",
                                !self.extra_tools.decoder_input_hex_view,
                                false,
                                false,
                                owner.clone(),
                            ))
                            .child(decoder_view_radio(
                                "decoder-input-hex-view",
                                "Hex",
                                "输入 Hex 视图",
                                self.extra_tools.decoder_input_hex_view,
                                false,
                                true,
                                owner.clone(),
                            )),
                    )
                    .child(
                        div()
                            .mr(px(18.))
                            .flex()
                            .items_center()
                            .child(decoder_help_popover()),
                    ),
            )
            .child(decoder_dropdown_select(
                "decoder-decode-menu",
                "解码为…",
                "选择解码或解压方式对当前数据执行解码",
                DECODE_MENU_ITEMS,
                last_op,
                owner.clone(),
                ctrl_h,
                ctrl_col_w,
            ))
            .child(decoder_dropdown_select(
                "decoder-encode-menu",
                "编码为…",
                "选择编码或压缩方式对当前数据执行编码",
                ENCODE_MENU_ITEMS,
                last_op,
                owner.clone(),
                ctrl_h,
                ctrl_col_w,
            ))
            .child(decoder_dropdown_select(
                "decoder-hash-menu",
                "哈希…",
                "选择摘要算法计算当前数据的哈希值",
                HASH_MENU_ITEMS,
                last_op,
                owner.clone(),
                ctrl_h,
                ctrl_col_w,
            ))
            .child(
                decoder_action_button(
                    "decoder-smart-decode",
                    "智能解码",
                    "自动识别并递归解码 URL、HTML 实体、Base64 与 Gzip 编码层",
                    ctrl_h,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.decoder_smart_decode(window, cx)
                })),
            );

        let input_stage = div()
            .id("decoder-input-stage")
            .test_support()
            .w_full()
            .h(stage_h)
            .flex()
            .items_stretch()
            .child(decoder_editor_box(
                &self.extra_tools.decoder_input,
                false,
                "Decoder 输入",
                decoder_editor_context_menu(self.extra_tools.decoder_input.clone(), false),
            ))
            .child(input_controls);

        let has_output_stage = !self.extra_tools.steps.is_empty()
            || self.extra_tools.decoder_output_bytes.is_some()
            || !self.extra_tools.decoder_output.read(cx).value().is_empty();

        let output_stage = has_output_stage.then(|| {
            let mode_text = self.extra_tools.decoder_mode.clone();
            let mode_tip = mode_text.clone();
            let output_controls = div()
                .w(ctrl_col_w)
                .flex_shrink_0()
                .ml(px(10.))
                .flex()
                .flex_col()
                .gap(ctrl_gap)
                .child(
                    div()
                        .w_full()
                        .h(row1_h)
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .child(decoder_view_radio(
                            "decoder-output-text-view",
                            "Text",
                            "输出 Text 视图",
                            !self.extra_tools.decoder_output_hex_view,
                            true,
                            false,
                            owner.clone(),
                        ))
                        .child(decoder_view_radio(
                            "decoder-output-hex-view",
                            "Hex",
                            "输出 Hex 视图",
                            self.extra_tools.decoder_output_hex_view,
                            true,
                            true,
                            owner.clone(),
                        )),
                )
                .child(
                    decoder_action_button(
                        "decoder-continue-button",
                        "输出转输入",
                        "将当前输出结果移入输入区继续下一轮转换",
                        ctrl_h,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.continue_decoder_output(window, cx)
                    })),
                )
                .child(
                    decoder_action_button(
                        "decoder-copy-output",
                        "复制输出",
                        "复制当前转换结果到剪贴板",
                        ctrl_h,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                            this.extra_tools.decoder_output.read(cx).value().to_string(),
                        ));
                    })),
                )
                .child(
                    decoder_action_button(
                        "decoder-clear-steps",
                        "清空步骤",
                        "清空转换链并重置输出阶段",
                        ctrl_h,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.clear_decoder_steps(window, cx)
                    })),
                )
                .child(
                    div()
                        .id("decoder-output-mode")
                        .test_support()
                        .w_full()
                        .h(ctrl_h)
                        .flex()
                        .items_center()
                        .text_size(theme::scaled_px(12.))
                        .text_color(theme::muted_foreground())
                        .when(!mode_tip.is_empty(), |this| {
                            this.tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(mode_tip.clone())
                                    .build(window, cx)
                            })
                        })
                        .child(div().min_w_0().truncate().child(mode_text)),
                );

            div()
                .id("decoder-output-stage")
                .test_support()
                .w_full()
                .h(stage_h)
                .flex()
                .items_stretch()
                .child(decoder_editor_box(
                    &self.extra_tools.decoder_output,
                    true,
                    "Decoder 输出",
                    decoder_editor_context_menu(self.extra_tools.decoder_output.clone(), true),
                ))
                .child(output_controls)
        });

        let step_chain = (!self.extra_tools.steps.is_empty()).then(|| {
            let mut step_items = div().flex().flex_wrap().items_center().gap(px(6.));
            for (idx, step) in self.extra_tools.steps.iter().enumerate() {
                let label = format!("#{} {}: {}", idx + 1, step.op_label, step.output_preview);
                step_items = step_items.child(
                    button(("decoder-step", idx), label)
                        .tooltip("点击回退到此转换步骤")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.restore_decoder_step(idx, window, cx)
                        })),
                );
            }
            div()
                .id("decoder-steps-bar")
                .test_support()
                .w_full()
                .pt(px(8.))
                .border_t_1()
                .border_color(theme::border())
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(theme::scaled_px(12.))
                        .text_color(theme::muted_foreground())
                        .child(format!(
                            "转换链 ({}/{MAX_DECODER_STEPS})",
                            self.extra_tools.steps.len()
                        )),
                )
                .child(step_items)
        });

        let workspace = div()
            .id("decoder-workspace")
            .test_support()
            .size_full()
            .bg(theme::surface())
            .overflow_y_scroll()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(input_stage)
            .children(output_stage)
            .children(step_chain)
            .into_any_element();
        if self.inspector.open && self.inspector.selection_preview.is_some() {
            self.render_inspector_dock(workspace, cx)
        } else {
            workspace
        }
    }

    pub(in crate::workbench) fn convert_extra_tool_selection(
        &mut self,
        kind: editing::ConvertKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool == Tool::Decoder
            && self
                .extra_tools
                .decoder_input
                .focus_handle(cx)
                .is_focused(window)
        {
            let state = self.extra_tools.decoder_input.clone();
            if state.update(cx, |s, cx| s.marked_text_range(window, cx).is_some()) {
                self.fail(editing::COMPOSITION_PENDING, cx);
                return;
            }
            let range = state.read(cx).selected_range();
            let reversed = state.read(cx).is_selection_reversed();
            if range.is_empty() {
                self.fail("请先在消息编辑器中选中要转换的文本。", cx);
                return;
            }
            let selected = state.read(cx).selected_value();
            match editing::convert_selection(kind, selected.as_ref()) {
                Ok(converted) => {
                    state.update(cx, |s, cx| {
                        s.replace(converted.clone(), window, cx);
                        s.set_selected_range_with_direction(
                            range.start..range.start + converted.len(),
                            reversed,
                            cx,
                        );
                    });
                    cx.notify();
                }
                Err(error) => self.fail(error, cx),
            }
        } else {
            self.dispatch_copy_converted_selection_action(
                &editing::CopyConvertedSelection(kind),
                window,
                cx,
            );
        }
    }

    pub(in crate::workbench) fn dispatch_copy_converted_selection_action(
        &mut self,
        action: &editing::CopyConvertedSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let extra_state = match self.tool {
            Tool::Decoder => {
                let out = &self.extra_tools.decoder_output;
                let inp = &self.extra_tools.decoder_input;
                if out.focus_handle(cx).is_focused(window)
                    || (!inp.focus_handle(cx).is_focused(window)
                        && !out.read(cx).selected_range().is_empty()
                        && inp.read(cx).selected_range().is_empty())
                {
                    Some(out.clone())
                } else {
                    Some(inp.clone())
                }
            }
            Tool::Comparer => {
                if self.extra_tools.comparer_side_by_side {
                    let right = &self.extra_tools.comparer_right_view;
                    let left = &self.extra_tools.comparer_left_view;
                    if right.focus_handle(cx).is_focused(window)
                        || (!left.focus_handle(cx).is_focused(window)
                            && !right.read(cx).selected_range().is_empty()
                            && left.read(cx).selected_range().is_empty())
                    {
                        Some(right.clone())
                    } else {
                        Some(left.clone())
                    }
                } else {
                    Some(self.extra_tools.comparer_view.clone())
                }
            }
            _ => None,
        };
        let Some(state) = extra_state else {
            self.copy_converted_selection_action(action, window, cx);
            return;
        };
        if self.transition.is_some() {
            self.fail("正在切换或保存项目；未转换选区。", cx);
            return;
        }
        match editing::convert_selected_editor_text(&state, action.0, window, cx) {
            Ok(converted) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(converted));
                self.error = None;
                self.status = "已转换选区并复制到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }
}
