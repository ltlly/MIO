use gpui_kit::assets::IconName;

use super::*;

impl Workbench {
    #[inline(never)]
    pub(super) fn render_extensions_plugin_store(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let card_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        div()
            .id("extensions-plugin-store-view")
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
                    .child("Plugin Store (离线模式)"),
            )
            .child(
                div()
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child("MIO 采用本地优先、零远程商店依赖的扩展加载模型，不会自动连接第三方在线插件商店。"),
            )
            .child(
                div()
                    .max_w(px(720.))
                    .p(px(14.))
                    .border_1()
                    .border_color(card_border)
                    .rounded(px(3.))
                    .bg(theme::toolbar_surface())
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("安装本地插件包"),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child("将包含 manifest.json 的本地插件目录或单文件归档（.tar.gz / .tgz / .mio-plugin）放置于本地后，通过「添加本地扩展」检查清单并显式授予所需宿主权限。"),
                    )
                    .child(
                        div()
                            .pt(px(4.))
                            .w(px(140.))
                            .child(
                                mio_neutral_button(
                                    "extensions-plugin-add-local",
                                    "添加本地扩展…",
                                    "返回已安装页并选择本地插件包目录或归档文件",
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
            .into_any_element()
    }

    #[inline(never)]
    pub(super) fn render_extensions_settings(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let card_border: gpui_kit::Hsla = rgb(if dark { 0x45484f } else { 0xd4d4d4 }).into();
        let plugins_dir_display = self
            .extensions
            .plugins_dir
            .clone()
            .or_else(|| mio_core::default_plugins_dir().ok())
            .map_or_else(|| "仅内存会话（未配置磁盘目录）".to_owned(), |p| p.display().to_string());

        div()
            .id("extensions-settings-view")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.))
            .bg(theme::surface())
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child("扩展设置"),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .p(px(14.))
                    .border_1()
                    .border_color(card_border)
                    .rounded(px(3.))
                    .bg(theme::toolbar_surface())
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("从指定本地目录或单文件归档加载扩展包"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().child(
                                Input::new(&self.extensions.directory).small(),
                            ))
                            .child(
                                button("plugin-choose", "选择插件…")
                                    .icon(IconName::FolderOpen)
                                    .tooltip("浏览并选择包含 manifest.json 的插件目录或单文件归档（.tar.gz / .tgz / .mio-plugin）")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_plugin(window, cx)
                                    })),
                            )
                            .child(
                                button("plugin-inspect", "检查插件包")
                                    .icon(IconName::ShieldCheck)
                                    .tooltip("解析插件清单并检查申请的宿主权限（不执行插件代码）")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.inspect_plugin(None, window, cx)
                                    })),
                            ),
                    )
                    .child(security_warning_callout()),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .p(px(14.))
                    .border_1()
                    .border_color(card_border)
                    .rounded(px(3.))
                    .bg(theme::toolbar_surface())
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("运行安全与生命周期策略"),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child(format!(
                                "• 用户持久化插件目录：{plugins_dir_display}（通过「确认并加载」安装后自动保存并跨重启/项目生效）"
                            )),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child("• 检查插件包（Inspect）仅解析 manifest.json 与入口路径，绝不执行插件代码。"),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child("• 已启用的持久化扩展会在启动或切换项目时按保存的权限与顺序自动加载；若启用「禁用扩展」则跳过自动加载（仍可卸载移除）。"),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child("• 取消勾选「已加载」仅停用插件而不删除文件；点击「移除」才会彻底卸载并清理磁盘文件。"),
                    )
                    .child(
                        div()
                            .pt(px(4.))
                            .w(px(140.))
                            .child(
                                mio_neutral_button(
                                    "extensions-open-global-settings",
                                    "打开全局设置…",
                                    "打开 MIO 项目与用户全局设置窗口",
                                    true,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_settings(window, cx);
                                })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
