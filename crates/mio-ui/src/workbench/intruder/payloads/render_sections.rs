use gpui_kit::FontWeight;
use gpui_kit::assets::IconName;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use mio_core::intruder::{DateOutputFormat, PayloadProcessingRule};

use super::*;

impl Workbench {
    fn generator_card(
        title: &'static str,
        buttons: impl IntoIterator<Item = AnyElement>,
    ) -> gpui_kit::Div {
        div()
            .p(px(6.))
            .border_1()
            .border_color(theme::border())
            .rounded(px(4.))
            .bg(theme::toolbar_surface())
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::foreground())
                    .child(title),
            )
            .child(div().flex().flex_wrap().gap(px(4.)).children(buttons))
    }

    #[allow(clippy::too_many_arguments)]
    fn proc_rule_btn(
        &self,
        id: TabId,
        editable: bool,
        btn_id: &'static str,
        label: &'static str,
        icon: IconName,
        tip: &'static str,
        rule: PayloadProcessingRule,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        button(btn_id, label)
            .icon(icon)
            .tooltip(tip)
            .disabled(!editable)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.process_intruder_payloads(id, rule.clone(), window, cx)
            }))
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_intruder_numbers_section(
        &self,
        id: TabId,
        editors: &IntruderEditors,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .border_t_1()
            .border_color(theme::border())
            .px(px(14.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(theme::scaled_px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Numbers 生成器"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&editors.numbers_from)
                                .id("intruder-numbers-from")
                                .aria_label("起始数字")
                                .small()
                                .disabled(!editable),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&editors.numbers_to)
                                .id("intruder-numbers-to")
                                .aria_label("结束数字")
                                .small()
                                .disabled(!editable),
                        ),
                    )
                    .child(
                        div().w(px(64.)).child(
                            Input::new(&editors.numbers_step)
                                .id("intruder-numbers-step")
                                .aria_label("步长")
                                .small()
                                .disabled(!editable),
                        ),
                    )
                    .child(
                        div().w(px(64.)).child(
                            Input::new(&editors.numbers_min_digits)
                                .id("intruder-numbers-min-digits")
                                .aria_label("最少位数")
                                .small()
                                .disabled(!editable),
                        ),
                    )
                    .child(
                        button("intruder-numbers-hex", "Hex")
                            .tooltip("以十六进制而非十进制格式生成数字序列")
                            .selected(editors.numbers_hex)
                            .disabled(!editable)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(editors) = this.intruder.editors.get_mut(&id) {
                                    editors.numbers_hex = !editors.numbers_hex;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        button("intruder-numbers-generate", "生成数字")
                            .icon(IconName::Sparkles)
                            .tooltip("按起止范围、步长与最少位数生成数字序列并追加到当前载荷列表")
                            .disabled(!editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.generate_intruder_numbers(id, window, cx)
                            })),
                    ),
            )
            .when(
                !self.live_payload_plugins("payload_generator").is_empty(),
                |section| {
                    let generators = self.live_payload_plugins("payload_generator");
                    section.child(div().flex().flex_wrap().gap(px(6.)).children(
                        generators.into_iter().map(|(plugin_uuid, plugin_id)| {
                            button(
                                SharedString::from(format!("intruder-plugin-gen-{plugin_uuid}")),
                                format!("插件生成：{plugin_id}"),
                            )
                            .icon(IconName::Sparkles)
                            .tooltip("调用已加载扩展插件生成自定义载荷序列并追加到当前列表")
                            .disabled(!editable)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.generate_plugin_intruder_payloads(
                                        id,
                                        plugin_uuid,
                                        window,
                                        cx,
                                    )
                                },
                            ))
                        }),
                    ))
                },
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_intruder_processing_section(
        &self,
        id: TabId,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pipeline_entries = self
            .intruder
            .editors
            .get(&id)
            .map(|editors| self.current_intruder_processing_rules(id, editors))
            .unwrap_or_default();
        let has_pipeline = !pipeline_entries.is_empty();
        let add_rule_weak = cx.weak_entity();
        let add_rule_dropdown = button("intruder-proc-rule-add-dropdown", "添加规则到管线")
            .dropdown_caret(true)
            .disabled(!editable)
            .tooltip("将所选 Payload Processing 规则追加到当前位置的持久化处理管线列表")
            .dropdown_menu(move |menu, _, _| {
                [
                    (
                        "Add prefix (从输入框)",
                        PayloadProcessingRule::AddPrefix(String::new()),
                    ),
                    (
                        "Add suffix (从输入框)",
                        PayloadProcessingRule::AddSuffix(String::new()),
                    ),
                    (
                        "Match/replace (old=>new)",
                        PayloadProcessingRule::MatchReplace {
                            match_pattern: String::new(),
                            replace_with: String::new(),
                        },
                    ),
                    (
                        "Regex match/replace (re=>new)",
                        PayloadProcessingRule::MatchReplaceRegex {
                            pattern: String::new(),
                            replace_with: String::new(),
                        },
                    ),
                    (
                        "Substring (offset:len)",
                        PayloadProcessingRule::Substring {
                            offset: 0,
                            length: 8,
                        },
                    ),
                    ("Reverse payload", PayloadProcessingRule::Reverse),
                    ("To upper case", PayloadProcessingRule::Uppercase),
                    ("To lower case", PayloadProcessingRule::Lowercase),
                    ("URL-encode", PayloadProcessingRule::UrlEncode),
                    ("URL-decode", PayloadProcessingRule::UrlDecode),
                    ("HTML-encode", PayloadProcessingRule::HtmlEncode),
                    ("HTML-decode", PayloadProcessingRule::HtmlDecode),
                    ("Base64-encode", PayloadProcessingRule::Base64Encode),
                    ("Base64-decode", PayloadProcessingRule::Base64Decode),
                    (
                        "Base64 URL-safe encode",
                        PayloadProcessingRule::Base64UrlEncode,
                    ),
                    (
                        "Base64 URL-safe decode",
                        PayloadProcessingRule::Base64UrlDecode,
                    ),
                    ("ASCII hex encode", PayloadProcessingRule::AsciiHexEncode),
                    ("ASCII hex decode", PayloadProcessingRule::AsciiHexDecode),
                    ("Hash: SHA-1", PayloadProcessingRule::Sha1Hex),
                    ("Hash: SHA-256", PayloadProcessingRule::Sha256Hex),
                    ("Hash: SHA-384", PayloadProcessingRule::Sha384Hex),
                    ("Hash: SHA-512", PayloadProcessingRule::Sha512Hex),
                    ("Hash: MD5", PayloadProcessingRule::Md5Hex),
                    ("Compress: Gzip", PayloadProcessingRule::GzipCompress),
                    (
                        "Add if not match regex",
                        PayloadProcessingRule::AddIfNotMatchRegex {
                            pattern: String::new(),
                            addition: String::new(),
                        },
                    ),
                    (
                        "Skip if matches regex",
                        PayloadProcessingRule::SkipIfMatchesRegex {
                            pattern: String::new(),
                        },
                    ),
                ]
                .into_iter()
                .fold(menu, |menu, (label, rule)| {
                    let weak = add_rule_weak.clone();
                    menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.add_intruder_processing_pipeline_rule(
                                id,
                                rule.clone(),
                                window,
                                cx,
                            );
                        });
                    }))
                })
            });
        div()
            .border_t_1()
            .border_color(theme::border())
            .px(px(14.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Payload 处理 (Processing)"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(add_rule_dropdown)
                            .when(has_pipeline, |row| {
                                row.child(
                                    button("intruder-proc-pipeline-run", "执行管线")
                                        .icon(IconName::Play)
                                        .tooltip("按顺序对当前载荷列表执行所有已启用的 Payload Processing 管线规则")
                                        .disabled(!editable)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.run_intruder_processing_pipeline(id, window, cx)
                                        })),
                                )
                            }),
                    ),
            )
            .when(has_pipeline, |section| {
                section.child(self.render_processing_pipeline_rules(
                    id,
                    pipeline_entries,
                    editable,
                    cx,
                ))
            })
            .child(self.render_processing_rule_palette(id, editable, cx))
            .child(self.render_intruder_payload_encoding_row(id, editable, cx))
            .into_any_element()
    }

    #[inline(never)]
    fn render_processing_pipeline_rules(
        &self,
        id: TabId,
        pipeline_entries: Vec<IntruderPayloadProcessingEntry>,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total_rules = pipeline_entries.len();
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .children(pipeline_entries.into_iter().enumerate().map(|(idx, entry)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .child(
                        div().flex_1().min_w_0().truncate().child(
                            Checkbox::new(("intruder-proc-rule-toggle", idx))
                                .label(entry.rule.label())
                                .checked(entry.enabled)
                                .disabled(!editable)
                                .on_click(cx.listener(move |this, value, window, cx| {
                                    this.update_intruder_payload_set_configs(
                                        id,
                                        |configs, selected| {
                                            let target = selected.unwrap_or(0);
                                            if let Some(cfg) = configs.get_mut(target)
                                                && let Some(item) =
                                                    cfg.processing_rules.get_mut(idx)
                                            {
                                                item.enabled = *value;
                                            }
                                        },
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                    )
                    .child(
                        button(("intruder-proc-rule-up", idx), "上移")
                            .disabled(!editable || idx == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.update_intruder_payload_set_configs(
                                    id,
                                    |configs, selected| {
                                        let target = selected.unwrap_or(0);
                                        if let Some(cfg) = configs.get_mut(target)
                                            && idx > 0
                                            && idx < cfg.processing_rules.len()
                                        {
                                            cfg.processing_rules.swap(idx - 1, idx);
                                        }
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button(("intruder-proc-rule-down", idx), "下移")
                            .disabled(!editable || idx + 1 >= total_rules)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.update_intruder_payload_set_configs(
                                    id,
                                    |configs, selected| {
                                        let target = selected.unwrap_or(0);
                                        if let Some(cfg) = configs.get_mut(target)
                                            && idx + 1 < cfg.processing_rules.len()
                                        {
                                            cfg.processing_rules.swap(idx, idx + 1);
                                        }
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button(("intruder-proc-rule-remove", idx), "删除")
                            .icon(IconName::Trash)
                            .disabled(!editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.update_intruder_payload_set_configs(
                                    id,
                                    |configs, selected| {
                                        let target = selected.unwrap_or(0);
                                        if let Some(cfg) = configs.get_mut(target)
                                            && idx < cfg.processing_rules.len()
                                        {
                                            cfg.processing_rules.remove(idx);
                                        }
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    )
            }))
            .into_any_element()
    }

    #[inline(never)]
    fn render_processing_rule_palette(
        &self,
        id: TabId,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let specs: [(&'static str, &'static str, IconName, &'static str, fn() -> PayloadProcessingRule); 26] = [
            ("intruder-proc-prefix", "加前缀", IconName::Plus, "将载荷输入框中的文本作为前缀添加到当前列表每条载荷开头", || PayloadProcessingRule::AddPrefix(String::new())),
            ("intruder-proc-suffix", "加后缀", IconName::Plus, "将载荷输入框中的文本作为后缀追加到当前列表每条载荷末尾", || PayloadProcessingRule::AddSuffix(String::new())),
            ("intruder-proc-substring", "子串截取", IconName::Scissors, "按输入框中的 偏移:长度（留空默认 0:8）截取每条载荷子串", || PayloadProcessingRule::Substring { offset: 0, length: 8 }),
            ("intruder-proc-replace", "字面替换", IconName::Replace, "按输入框中的 原文本=>新文本 对当前列表每条载荷执行字面替换", || PayloadProcessingRule::MatchReplace { match_pattern: String::new(), replace_with: String::new() }),
            ("intruder-proc-add-if-not-match", "不匹配追加", IconName::Plus, "按输入框中的 正则=>后缀 规则，对不匹配正则的载荷追加指定后缀", || PayloadProcessingRule::AddIfNotMatchRegex { pattern: String::new(), addition: String::new() }),
            ("intruder-proc-skip-if-match", "匹配则跳过", IconName::ListFilter, "移除当前列表中匹配载荷输入框内正则表达式的载荷条目", || PayloadProcessingRule::SkipIfMatchesRegex { pattern: String::new() }),
            ("intruder-proc-url-encode", "URL 编码", IconName::Percent, "对当前列表中的每条载荷执行标准百分号 URL 编码", || PayloadProcessingRule::UrlEncode),
            ("intruder-proc-url-decode", "URL 解码", IconName::Percent, "对当前列表中的每条载荷执行百分号 URL 解码", || PayloadProcessingRule::UrlDecode),
            ("intruder-proc-base64-encode", "Base64 编码", IconName::Binary, "对当前列表中的每条载荷执行标准 Base64 编码", || PayloadProcessingRule::Base64Encode),
            ("intruder-proc-base64-decode", "Base64 解码", IconName::Binary, "对当前列表中的每条载荷执行 Base64 解码", || PayloadProcessingRule::Base64Decode),
            ("intruder-proc-hex-encode", "Hex 编码", IconName::Code, "将当前列表中的每条载荷字节编码为小写十六进制字符串", || PayloadProcessingRule::AsciiHexEncode),
            ("intruder-proc-hex-decode", "Hex 解码", IconName::Code, "将当前列表中的十六进制字符串解码还原为原始字节载荷", || PayloadProcessingRule::AsciiHexDecode),
            ("intruder-proc-uppercase", "大写", IconName::ArrowUp, "将当前列表每条载荷中的英文字母全部转换为大写", || PayloadProcessingRule::Uppercase),
            ("intruder-proc-lowercase", "小写", IconName::ArrowDown, "将当前列表每条载荷中的英文字母全部转换为小写", || PayloadProcessingRule::Lowercase),
            ("intruder-proc-sha256", "SHA-256", IconName::Hash, "将当前列表中的每条载荷替换为对应的 SHA-256 十六进制摘要", || PayloadProcessingRule::Sha256Hex),
            ("intruder-proc-md5", "MD5", IconName::Hash, "将当前列表中的每条载荷替换为对应的 MD5 十六进制摘要", || PayloadProcessingRule::Md5Hex),
            ("intruder-proc-reverse", "反转", IconName::ArrowLeftRight, "按字符/字节顺序反转当前列表中的每条载荷", || PayloadProcessingRule::Reverse),
            ("intruder-proc-regex-replace", "正则替换", IconName::Replace, "按输入框中的 正则=>替换文本 对当前列表每条载荷执行正则替换", || PayloadProcessingRule::MatchReplaceRegex { pattern: String::new(), replace_with: String::new() }),
            ("intruder-proc-html-encode", "HTML 编码", IconName::Code, "对当前列表中的每条载荷执行 HTML 实体编码", || PayloadProcessingRule::HtmlEncode),
            ("intruder-proc-html-decode", "HTML 解码", IconName::Code, "对当前列表中的每条载荷执行 HTML 实体解码", || PayloadProcessingRule::HtmlDecode),
            ("intruder-proc-base64-url-encode", "Base64Url 编码", IconName::Binary, "对当前列表中的每条载荷执行 URL 安全无填充 Base64Url 编码", || PayloadProcessingRule::Base64UrlEncode),
            ("intruder-proc-base64-url-decode", "Base64Url 解码", IconName::Binary, "对当前列表中的每条载荷执行 Base64Url 解码", || PayloadProcessingRule::Base64UrlDecode),
            ("intruder-proc-sha1", "SHA-1", IconName::Hash, "将当前列表中的每条载荷替换为对应的 SHA-1 十六进制摘要", || PayloadProcessingRule::Sha1Hex),
            ("intruder-proc-sha384", "SHA-384", IconName::Hash, "将当前列表中的每条载荷替换为对应的 SHA-384 十六进制摘要", || PayloadProcessingRule::Sha384Hex),
            ("intruder-proc-sha512", "SHA-512", IconName::Hash, "将当前列表中的每条载荷替换为对应的 SHA-512 十六进制摘要", || PayloadProcessingRule::Sha512Hex),
            ("intruder-proc-gzip", "Gzip 压缩", IconName::Binary, "将当前列表中的每条载荷压缩为 Gzip 字节流", || PayloadProcessingRule::GzipCompress),
        ];
        let mut row = div().flex().flex_wrap().gap(px(3.));
        for (btn_id, label, icon, tip, make_rule) in specs {
            row = row.child(self.proc_rule_btn(id, editable, btn_id, label, icon, tip, make_rule(), cx));
        }
        row.children(
            self.live_payload_plugins("payload_processor")
                .into_iter()
                .map(|(plugin_uuid, plugin_id)| {
                    button(
                        SharedString::from(format!("intruder-plugin-proc-{plugin_uuid}")),
                        format!("插件处理：{plugin_id}"),
                    )
                    .icon(IconName::WandSparkles)
                    .tooltip("调用已加载扩展插件批量转换当前列表中的每条载荷")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.process_plugin_intruder_payloads(id, plugin_uuid, window, cx)
                    }))
                }),
        )
        .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_payload_encoding_row(
        &self,
        id: TabId,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .pt(px(2.))
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::muted_foreground())
                    .child("URL 编码指定字符 (Payload encoding)"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().child(WithWindow(Rc::new(
                        move |window, cx| {
                            let input = ensure_payload_encode_input(id, window, cx);
                            Input::new(&input)
                                .id("intruder-payload-encode-chars")
                                .aria_label("URL 编码指定字符 (Payload encoding)")
                                .small()
                                .disabled(!editable)
                                .into_any_element()
                        },
                    ))))
                    .child(
                        button("intruder-payload-encode-selected", "编码保留字符")
                            .icon(IconName::Percent)
                            .tooltip("对当前载荷集中出现的指定保留字符执行百分号 URL 编码")
                            .disabled(!editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.encode_intruder_payload_selected_chars(id, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_intruder_generators_cards(
        &self,
        id: TabId,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_copy_first = self
            .intruder
            .editors
            .get(&id)
            .and_then(|editors| editors.payload_position)
            .is_some_and(|index| index > 0);
        let mut null_buttons = vec![
            button("intruder-case-variants", "大小写变体")
                .icon(IconName::ALargeSmall)
                .tooltip(
                    "根据载荷输入框中的标识符（逗号分隔）生成驼峰、下划线、短横线及大小写命名变体",
                )
                .disabled(!editable)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.generate_intruder_case_variants(id, window, cx)
                }))
                .into_any_element(),
            button("intruder-null-payloads", "重复基准次数 (Null payloads)")
                .icon(IconName::Repeat)
                .tooltip("按输入框指定次数（默认 10 次）生成空载荷，用于条件竞争或无参重放测试")
                .disabled(!editable)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.generate_intruder_empty_repeats(id, window, cx)
                }))
                .into_any_element(),
        ];
        if can_copy_first {
            null_buttons.push(
                button("intruder-copy-first-set", "复制第 1 组列表")
                    .icon(IconName::Copy)
                    .tooltip("将 Payload 位置 1 的完整载荷列表复制到当前选中的载荷位置")
                    .disabled(!editable)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.copy_first_intruder_payload_set(id, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        div()
            .border_t_1()
            .border_color(theme::border())
            .px(px(14.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(Self::generator_card(
                "日期序列生成器 (Dates)",
                [
                    (
                        "intruder-dates-iso",
                        "生成 YYYY-MM-DD",
                        "按输入框起止日期与步长（留空默认 7 天）生成 ISO 标准格式日期载荷",
                        DateOutputFormat::IsoDate,
                    ),
                    (
                        "intruder-dates-compact",
                        "生成 YYYYMMDD",
                        "按输入框起止日期与步长生成紧凑纯数字格式日期载荷",
                        DateOutputFormat::Compact,
                    ),
                    (
                        "intruder-dates-slash",
                        "生成 YYYY/MM/DD",
                        "按输入框起止日期与步长生成斜杠分隔格式日期载荷",
                        DateOutputFormat::Slash,
                    ),
                ]
                .into_iter()
                .map(|(btn_id, label, tip, fmt)| {
                    button(btn_id, label)
                        .icon(IconName::Calendar)
                        .tooltip(tip)
                        .disabled(!editable)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.generate_intruder_dates(id, fmt, window, cx)
                        }))
                        .into_any_element()
                }),
            ))
            .child(Self::generator_card(
                "字符块长度生成器 (Character blocks)",
                [
                    (
                        "intruder-blocks-short",
                        "生成 1–16 长度",
                        "以输入框字符为基块（默认 A），按步长 1 生成重复 1 至 16 次的边界测试载荷",
                        Some((1, 16, 1)),
                    ),
                    (
                        "intruder-blocks-boundary",
                        "生成 16–256 长度",
                        "以输入框字符为基块（默认 A），按步长 16 生成重复 16 至 256 次的缓冲区探测载荷",
                        Some((16, 256, 16)),
                    ),
                    (
                        "intruder-blocks-spec",
                        "按规格生成",
                        "按输入框规格 基块:起始~结束:步长（留空默认 A:4~32:4）生成递增长度字符块",
                        None,
                    ),
                ]
                .into_iter()
                .map(|(btn_id, label, tip, range)| {
                    button(btn_id, label)
                        .icon(IconName::Blocks)
                        .tooltip(tip)
                        .disabled(!editable)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.generate_intruder_char_blocks(id, range, window, cx)
                        }))
                        .into_any_element()
                }),
            ))
            .child(Self::generator_card(
                "大小写变体 / 空载荷基准 / 跨组复制",
                null_buttons,
            ))
            .into_any_element()
    }
}
