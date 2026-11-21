use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, button::ButtonVariants};
use mio_core::protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR, Permission};

use super::*;

mod api_docs;
mod details;
mod installed;
mod settings;

pub(in crate::workbench::plugins) use self::api_docs::{
    build_mio_plugin_api_json, build_mio_plugin_api_markdown,
};

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_extensions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.sync_active_top_suite_tab();
        if self.extensions.search_subscription.is_none() {
            let search = self.extensions.search.clone();
            self.extensions.search_subscription = Some(cx.subscribe(
                &search,
                |_, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                },
            ));
        }
        if let Some(top_id) = self.extensions.active_top_suite_tab
            && let Some(instance) = self.extensions.instance(top_id)
        {
            return div()
                .id("plugin-top-suite-view")
                .test_support()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .min_w_0()
                .child(instance.panel.clone())
                .into_any_element();
        }

        let inspecting = self.extensions.operations.values().any(|operation| {
            matches!(
                operation,
                Operation::Inspect { .. }
                    | Operation::Load { .. }
                    | Operation::Reload { .. }
                    | Operation::EnableInstalled { .. }
                    | Operation::Uninstall { .. }
            )
        });
        let moving = self
            .extensions
            .operations
            .values()
            .any(|operation| matches!(operation, Operation::Reorder));
        let busy = inspecting
            || moving
            || !self.backend_available()
            || self.transition.is_some()
            || self.file_picker_pending();

        let body = match self.extensions.page {
            ExtensionsPage::Installed => {
                self.render_extensions_installed(busy, inspecting, cx)
            }
            ExtensionsPage::PluginStore => self.render_extensions_plugin_store(busy, cx),
            ExtensionsPage::Apis => self.render_extensions_apis(busy, cx),
            ExtensionsPage::CustomScanChecks => {
                self.render_extensions_custom_scan_checks(busy, cx)
            }
            ExtensionsPage::ScriptPresetLibrary => self.render_extensions_script_library(busy, cx),
            ExtensionsPage::Settings => self.render_extensions_settings(busy, cx),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .bg(theme::surface())
            .child(self.render_extensions_sub_tabs(cx))
            .child(div().flex_1().min_h_0().min_w_0().flex().flex_col().child(body))
            .into_any_element()
    }

    #[inline(never)]
    fn render_extensions_sub_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = self.extensions.page;
        div()
            .id("extensions-sub-tab-bar")
            .relative()
            .w_full()
            .h(theme::secondary_tab_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(theme::toolbar_surface())
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(theme::border()),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-installed",
                    "已安装",
                    None,
                    page == ExtensionsPage::Installed,
                    "管理当前项目中已安装的本地 Native 扩展",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::Installed;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-plugin-store",
                    "Plugin Store",
                    None,
                    page == ExtensionsPage::PluginStore,
                    "查看离线扩展包说明与本地安装入口",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::PluginStore;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-apis",
                    "APIs",
                    None,
                    page == ExtensionsPage::Apis,
                    "查看 MIO 宿主插件协议 API 与宿主权限参考",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::Apis;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-custom-scan-checks",
                    "自定义扫描检查",
                    None,
                    page == ExtensionsPage::CustomScanChecks,
                    "查看与管理自定义被动/主动安全扫描检查规则与扩展入口",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::CustomScanChecks;
                    cx.notify();
                })),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-script-library",
                    "脚本库",
                    None,
                    page == ExtensionsPage::ScriptPresetLibrary,
                    "查看在 Proxy 历史筛选与 Match and replace 中复用的 脚本库",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::ScriptPresetLibrary;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .px(px(6.))
                    .h(theme::secondary_tab_height() - px(1.))
                    .self_start()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .child(div().w(px(1.)).h(px(22.)).bg(theme::border())),
            )
            .child(
                mio_flat_tab(
                    "extensions-tab-settings",
                    "扩展设置",
                    Some(IconName::Settings),
                    page == ExtensionsPage::Settings,
                    "配置手动加载路径、查看安全策略并打开全局设置",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.extensions.page = ExtensionsPage::Settings;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn export_plugin_api_interface_files(
        &mut self,
        markdown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.file_picker_pending() {
            return;
        }
        let suggested = if markdown {
            format!("mio-plugin-api-v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}.md")
        } else {
            format!("mio-plugin-api-v{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}.json")
        };
        let json_payload = match build_mio_plugin_api_json() {
            Ok(bytes) => bytes,
            Err(err) => {
                self.fail(format!("生成 MIO 插件接口定义失败：{err}"), cx);
                return;
            }
        };
        let md_payload = build_mio_plugin_api_markdown().into_bytes();
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(&suggested),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) {
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            let use_md = markdown
                                || path
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .is_some_and(|e| e.eq_ignore_ascii_case("md"));
                            if path.extension().is_none() {
                                path.set_extension(if use_md { "md" } else { "json" });
                            }
                            let bytes = if use_md { &md_payload } else { &json_payload };
                            match std::fs::write(&path, bytes) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已导出 MIO 插件接口定义文件：{}",
                                        path.display()
                                    );
                                    view.extensions.push_log(format!(
                                        "已导出 MIO Extensions API 接口定义到：{}",
                                        path.display()
                                    ));
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("写入接口定义文件失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(error)) => view.fail(format!("无法选择保存路径：{error}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }
}

pub(super) fn mio_flat_tab(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    selected: bool,
    tip: &'static str,
) -> gpui_kit::base::Button {
    let label = label.into();
    let dark = theme::is_dark();
    let hover_bg: gpui_kit::Hsla = if dark {
        rgb(if selected { 0x25272b } else { 0x2f3237 }).into()
    } else {
        rgb(if selected { 0xfbfbfb } else { 0xeeeeee }).into()
    };
    gpui_kit::base::Button::new(id)
        .role(gpui_kit::accesskit::Role::Tab)
        .accessibility_label(label.clone())
        .aria_selected(selected)
        .relative()
        .h(theme::secondary_tab_height() - px(1.))
        .self_start()
        .flex_shrink_0()
        .px(theme::scaled_px(15.))
        .font_family(theme::ui_font_family())
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::NORMAL)
        .line_height(theme::scaled_px(17.))
        .text_color(if selected && !dark {
            rgb(0x000000).into()
        } else {
            theme::foreground()
        })
        .flex()
        .items_center()
        .gap(px(6.))
        .bg(theme::toolbar_surface())
        .hover(move |s| s.bg(hover_bg))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .when_some(icon, |tab, ic| {
            tab.child(
                Icon::new(ic)
                    .size(px(18.))
                    .text_color(theme::foreground()),
            )
        })
        .child(label)
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(3.))
                    .bg(theme::accent()),
            )
        })
}

pub(in crate::workbench::plugins) fn mio_neutral_button(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    tip: &'static str,
    available: bool,
) -> gpui_kit::base::Button {
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = rgb(if dark { 0x33363a } else { 0xececec }).into();
    let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x3e4247 } else { 0xe2e2e2 }).into();
    let active_bg: gpui_kit::Hsla = rgb(if dark { 0x4b5056 } else { 0xd8d8d8 }).into();
    let border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xd0d0d0 }).into();
    let fg: gpui_kit::Hsla = rgb(if dark { 0xdfe1e5 } else { 0x1e1e1e }).into();
    gpui_kit::base::Button::new(id)
        .accessibility_label(label)
        .disabled(!available)
        .w_full()
        .h(theme::scaled_px(26.).max(px(22.)))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::BOLD)
        .text_color(fg)
        .hover(move |s| if available { s.bg(hover_bg) } else { s })
        .active(move |s| if available { s.bg(active_bg) } else { s })
        .styles(|styles| styles.disabled(|s| s.opacity(0.45)))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .child(label)
}

pub(super) fn security_warning_callout() -> gpui_kit::Div {
    div()
        .w_full()
        .px(px(10.))
        .py(px(6.))
        .rounded(px(3.))
        .bg(theme::danger_bg())
        .border_1()
        .border_color(theme::danger_border())
        .text_size(px(12.))
        .text_color(theme::danger_fg())
        .child("Native worker 是可信本地程序，不受 OS 沙箱限制，可直接使用当前用户的文件与网络权限。下面的授权只限制 MIO 宿主 API，并不能沙箱化进程。")
}

pub(super) fn permission_short_name(permission: Permission) -> &'static str {
    match permission {
        Permission::TrafficRead => "traffic.read",
        Permission::DraftWrite => "draft.write",
        Permission::RepeaterOpen => "repeater.open",
        Permission::NetworkSend => "network.send",
        Permission::TrafficModify => "traffic.modify",
        Permission::SessionHook => "session.hook",
        Permission::PayloadProcess => "payload.process",
        Permission::Storage => "storage.kv",
    }
}

pub(super) fn permission_label(permission: Permission) -> &'static str {
    match permission {
        Permission::TrafficRead => "traffic.read：读取当前项目的流量摘要、请求与响应",
        Permission::DraftWrite => "draft.write：申请修改已绑定的 Repeater 草稿",
        Permission::RepeaterOpen => "repeater.open：申请创建新的 Repeater / Intruder 标签",
        Permission::NetworkSend => "network.send：经宿主发送请求（仅限获授权目标）",
        Permission::TrafficModify => "traffic.modify：注册 Proxy / HTTP / WebSocket 流量处理钩子",
        Permission::SessionHook => "session.hook：在发送前后执行会话处理动作",
        Permission::PayloadProcess => "payload.process：提供 Intruder payload 生成与处理规则",
        Permission::Storage => "storage.kv：读写当前项目内此插件专属键值存储",
    }
}
