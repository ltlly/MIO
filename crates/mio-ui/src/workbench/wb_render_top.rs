use super::*;

mod footer;
#[cfg(test)]
mod tests;

pub(in crate::workbench) use footer::*;

impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workbench {
    #[inline(never)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        theme::sync_window_system_dpi(window.scale_factor(), cx);
        if self.startup_wizard_active() && !self.shutdown_requested {
            return self.render_startup_wizard(window, cx).into_any_element();
        }
        self.sync_inspector_auto_open(window, cx);
        let footer_project_name = self.current_footer_project_name();
        let window_title = match footer_project_name.as_deref() {
            Some(name) => format!("MIO — {name}"),
            None => "MIO".to_owned(),
        };
        let body = self.render_workbench_active_body(window, cx);
        let footer_bar = self.render_workbench_footer_bar(footer_project_name, window, cx);
        let banners = self.render_workbench_banners(cx);
        let mut root = self.render_workspace_action_root(window, cx);
        self.bind_extended_top_menu_actions(&mut root, cx);
        let title_bar = crate::chrome::render_title_bar(
            "mio-workbench",
            window_title,
            crate::chrome::TitleBarControls::FULL,
            |_, cx| {
                cx.global::<crate::native_terminate::NativeTerminate>()
                    .request();
            },
            window,
            cx,
        );
        root.child(title_bar)
            .child(self.render_tool_tabs(window, cx))
            .child(div().relative().flex_1().min_h_0().child(body)
                .children(self.render_global_overlays(window, cx)))
            .children(banners)
            .child(footer_bar)
            .into_any_element()
    }
}


impl Workbench {
    #[inline(never)]
    fn render_workbench_active_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.shutdown_requested {
            div()
                .id("runtime-shutdown")
                .test_support()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("正在等待后台任务、连接与项目文件完成清理…")
                .into_any_element()
        } else if self.transition == Some(TransitionPhase::Switching) {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("正在打开项目；完成前不会丢弃当前工作区。")
                .into_any_element()
        } else if self.ext_import.visible {
            self.render_ext_import(cx)
        } else {
            match self.tool {
                Tool::Dashboard => self.audit_dashboard_view.clone().into_any_element(),
                Tool::Target => self.target_view.clone().into_any_element(),
                Tool::Proxy => self.render_proxy(window, cx),
                Tool::Intruder => self.render_intruder(window, cx),
                Tool::Repeater => self.render_repeater_with_redirect(window, cx),
                Tool::Collaborator => self.collaborator_view.clone().into_any_element(),
                Tool::Sequencer => self.sequencer_view.clone().into_any_element(),
                Tool::Extensions => self.render_extensions(window, cx),
                Tool::Decoder => self.render_decoder(window, cx),
                Tool::Comparer => self.render_comparer(window, cx),
                Tool::Logger => self.render_logger(window, cx),
                Tool::Organiser => self.render_organiser(window, cx),
                Tool::Discover => self.render_discover(window, cx),
            }
        }
    }

    #[inline(never)]
    fn render_discover(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ipc_path = self.effective_ipc_socket_path();
        let ipc_path_str = ipc_path.display().to_string();
        let cli_status_cmd = format!("mio-cli --socket \"{}\" status", ipc_path_str);
        let backend_ok = self.backend_available();
        let proxy_summary = self
            .proxy_address
            .map(|addr| format!("运行中 ({addr})"))
            .unwrap_or_else(|| "未启动".into());
        let card_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x232529 } else { 0xffffff }).into();
        let hero_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x1e2024 } else { 0xf6f7f9 }).into();
        let copy_cmd = cli_status_cmd.clone();
        let copy_sock = ipc_path_str.clone();
        div()
            .id("discover-main-pane")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .bg(theme::surface())
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .py(px(16.))
                    .px(px(24.))
                    .bg(hero_bg)
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(693.))
                            .max_w_full()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(28.))
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_color(rgb(0xdb612f))
                                            .child("MIO"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(28.))
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_color(theme::foreground())
                                            .child("工作台与智能体协同"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme::foreground())
                                    .child("MioAT 将代理智能体工作流与 MIO 的流量代理、项目上下文和专业安全工具结合，辅助人工安全测试与自动化验证。"),
                            )
                            .child(
                                div()
                                    .p(px(8.))
                                    .rounded(px(6.))
                                    .bg(card_bg)
                                    .border_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .id("discover-ipc-status")
                                            .test_support()
                                            .text_size(px(12.))
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .text_color(theme::foreground())
                                            .child(format!(
                                                "Agent IPC 状态：{} · Proxy：{}",
                                                if backend_ok { "已就绪" } else { "已断开" },
                                                proxy_summary
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("discover-ipc-socket-path")
                                            .test_support()
                                            .font_family(theme::mono_font_family())
                                            .text_size(px(11.5))
                                            .text_color(theme::muted_foreground())
                                            .child(format!("MIO_IPC_SOCKET={ipc_path_str}")),
                                    )
                                    .child(
                                        div()
                                            .font_family(theme::mono_font_family())
                                            .text_size(px(11.5))
                                            .text_color(theme::foreground())
                                            .child(cli_status_cmd),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        Button::new("discover-copy-ipc-cmd")
                                            .bg(rgb(0xdb612f))
                                            .text_color(rgb(0xffffff))
                                            .small()
                                            .w(px(143.))
                                            .h(px(23.))
                                            .label("复制 CLI 接入命令")
                                            .accessibility_label("复制 CLI 接入命令")
                                            .tooltip("复制 mio-cli Agent IPC 接入命令到剪贴板")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        copy_cmd.clone(),
                                                    ),
                                                );
                                                this.error = None;
                                                this.status =
                                                    "已复制 Agent IPC CLI 接入命令到剪贴板。".into();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("discover-copy-ipc-socket")
                                            .small()
                                            .w(px(128.))
                                            .h(px(23.))
                                            .label("复制 Socket 路径")
                                            .accessibility_label("复制 Socket 路径")
                                            .tooltip("复制当前 MIO_IPC_SOCKET 套接字路径到剪贴板")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui_kit::ClipboardItem::new_string(
                                                        copy_sock.clone(),
                                                    ),
                                                );
                                                this.error = None;
                                                this.status =
                                                    "已复制 MIO_IPC_SOCKET 路径到剪贴板。".into();
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("discover-open-mio-ai-panel")
                                            .small()
                                            .h(px(23.))
                                            .px(px(10.))
                                            .bg(rgb(0x4946ae))
                                            .text_color(rgb(0xffffff))
                                            .label("MioAT 控制中心")
                                            .accessibility_label("MioAT 控制中心")
                                            .tooltip("打开交互式 MioAT / Agent IPC 提示词与上下文控制面板")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.mio_ai_panel_open = true;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(1270.))
                    .py(px(14.))
                    .px(px(24.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child("探索更多能力"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .gap(px(14.))
                            .child(self.render_discover_card(
                                "discover-card-command-palette",
                                "命令面板快速直达",
                                "通过命令面板即时搜索并执行 MIO 常用操作，无需在多层菜单中来回查找。",
                                cx.listener(|this, _, window, cx| {
                                    this.open_command_palette(window, cx);
                                }),
                            ))
                            .child(self.render_discover_card(
                                "discover-card-custom-logic",
                                "使用自定义逻辑扩展 MIO",
                                "通过自定义动作、脚本列和筛选脚本定制分析工作流，快速聚焦关键请求与响应。",
                                cx.listener(|this, _, window, cx| {
                                    this.select_tool(Tool::Proxy, window, cx);
                                    this.proxy_page = ProxyPage::History;
                                    cx.notify();
                                }),
                            ))
                            .child(self.render_discover_card(
                                "discover-card-scan-checks",
                                "使用自定义检查扩展扫描",
                                "导入或编写自定义扫描规则，扩展 MIO 审计扫描器在主动与被动检测中的覆盖能力。",
                                cx.listener(|this, _, _, cx| {
                                    this.open_dashboard(cx);
                                    this.audit_dashboard.new_scan_open = true;
                                    cx.notify();
                                }),
                            ))
                            .child(self.render_discover_card(
                                "discover-card-organiser",
                                "整理与归档测试发现",
                                "使用集合分组整理关键请求与响应记录，便于持续跟进、对比与导出共享。",
                                cx.listener(|this, _, window, cx| {
                                    this.select_tool(Tool::Organiser, window, cx);
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_discover_card(
        &self,
        button_id: &'static str,
        title: &'static str,
        description: &'static str,
        on_take_me_there: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let card_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x232529 } else { 0xffffff }).into();
        div()
            .w(px(322.))
            .p(px(12.))
            .rounded(px(6.))
            .bg(card_bg)
            .border_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .justify_between()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme::foreground())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme::muted_foreground())
                            .child(description),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        Button::new(button_id)
                            .small()
                            .w(px(145.))
                            .h(px(23.))
                            .label("立即前往")
                            .accessibility_label(format!("立即前往：{title}"))
                            .on_click(on_take_me_there),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_workbench_banners(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let visible_error = match (&self.error, &self.user_preferences.remember_error) {
            (Some(error), Some(remember)) if error != remember => {
                Some(format!("{error}\n{remember}"))
            }
            (Some(error), _) => Some(error.clone()),
            (None, remember) => remember.clone(),
        };
        let mut out = Vec::new();
        if let Some(error) = self.storage_error.clone() {
            out.push(
                toolbar()
                    .h_auto()
                    .min_h(theme::scaled_px(34.))
                    .py(px(3.))
                    .bg(theme::danger_bg())
                    .text_color(theme::danger_fg())
                    .child(div().flex_1().child(format!(
                        "存储错误（{} 条待恢复）：{error}",
                        self.pending_records
                    )))
                    .child(
                        button("storage-retry", "保存重试")
                            .icon(gpui_kit::assets::IconName::RefreshCw)
                            .tooltip("重试将待恢复数据写入当前项目文件")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_project(false, window, cx)
                            })),
                    )
                    .child(
                        button("storage-save-as", "另存为…")
                            .icon(gpui_kit::assets::IconName::FolderOpen)
                            .tooltip("将当前项目与待恢复数据另存为新的 .mio 文件")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_project(true, window, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = visible_error {
            out.push(
                toolbar()
                    .id("workspace-error")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::Alert)
                    .aria_label(error.clone())
                    .h_auto()
                    .min_h(theme::scaled_px(30.))
                    .py(px(2.))
                    .bg(theme::danger_bg())
                    .text_color(theme::danger_fg())
                    .child(div().flex_1().child(error))
                    .child(
                        button("dismiss-error", "关闭提示")
                            .icon(gpui_kit::assets::IconName::Close)
                            .tooltip("隐藏当前工作区错误提示横幅")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.error = None;
                                this.user_preferences.remember_error = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    #[inline(never)]
    fn render_workspace_action_root(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + ParentElement + InteractiveElement + Styled + use<> {
        let mut root = div()
            .id("mio-workspace")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .bg(theme::background())
            .text_color(theme::foreground())
            .key_context("MioWorkspace")
            .track_focus(&self.focus);
        let r = root.interactivity();
        r.on_action(cx.listener(|this, _: &crate::NewWindow, _, cx| {
                this.open_new_window(cx)
            }));
        if !self.file_picker_pending() {
            r.on_action(
                cx.listener(|this, _: &crate::OpenProjectInNewWindow, window, cx| {
                    this.choose_open_in_new_window(window, cx)
                }),
            );
        }
        r.on_action(cx.listener(|this, _: &crate::OpenSettings, window, cx| {
                this.open_settings(window, cx)
            }));
        r.on_action(cx.listener(|this, _: &crate::OpenCommandPalette, window, cx| {
                this.open_command_palette(window, cx)
            }));
        r.on_action(cx.listener(|this, _: &crate::OpenGlobalSearch, window, cx| {
                this.open_global_search(window, cx)
            }));
        if self.backend_available()
            && self.transition.is_none()
            && !self.file_picker_pending()
        {
                    r.on_action(cx.listener(|this, _: &NewProject, window, cx| {
                        this.request_transition(AfterSave::NewProject, window, cx)
                    }));
                    r.on_action(
                        cx.listener(|this, _: &OpenProject, window, cx| {
                            this.choose_open(window, cx)
                        }),
                    );
                    r.on_action(cx.listener(|this, _: &ImportExtProject, window, cx| {
                        this.choose_ext_import(window, cx)
                    }));
                    r.on_action(cx.listener(|this, _: &ShowExtReport, window, cx| {
                        this.show_ext_report(window, cx)
                    }));
                    r.on_action(cx.listener(|this, _: &SaveProject, window, cx| {
                        this.save_project(false, window, cx)
                    }));
                    r.on_action(cx.listener(|this, _: &SaveProjectAs, window, cx| {
                        this.save_project(true, window, cx)
                    }));
                    r.on_action(
                        cx.listener(|this, _: &crate::SlimSaveProjectAs, window, cx| {
                            this.choose_slim_save_as(window, cx)
                        }),
                    );
                    r.on_action(
                        cx.listener(|this, _: &ExportCa, window, cx| this.export_ca(window, cx)),
                    );
        }
        r.on_action(|_: &crate::Quit, _, cx| {
                cx.global::<crate::native_terminate::NativeTerminate>()
                    .request();
            });
        r.on_action(cx.listener(Self::search_message_action));
        if !self.ext_import.visible {
                r.on_action(cx.listener(|this, _: &OpenInIntruder, window, cx| {
                    this.open_in_intruder(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &AddHttpNotes, window, cx| {
                    this.open_http_notes(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::ClearHighlight, window, cx| {
                    this.clear_selected_tool_highlight(window, cx);
                }));
                r.on_action(cx.listener(|this, _: &OpenInRepeater, window, cx| {
                    this.open_in_repeater(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToDecoder, window, cx| {
                    this.send_to_decoder(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToSequencer, window, cx| {
                    this.send_to_sequencer(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToScanner, window, cx| {
                    this.send_to_scanner(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::OpenScanLauncher, window, cx| {
                    this.open_scan_launcher(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToPassiveScanner, window, cx| {
                    this.send_to_passive_scanner(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToOrganiser, window, cx| {
                    this.send_selected_to_organiser(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::SendToMioAt, _, cx| {
                    this.send_selected_to_mio_at(cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::AddToSiteMap, window, cx| {
                    this.add_to_site_map(window, cx)
                }));
                r.on_action(
                    cx.listener(|this, _: &crate::SendToComparerRequest, window, cx| {
                        this.send_to_comparer(false, window, cx)
                    }),
                );
                r.on_action(
                    cx.listener(|this, _: &crate::SendToComparerResponse, window, cx| {
                        this.send_to_comparer(true, window, cx)
                    }),
                );
                r.on_action(cx.listener(|this, _: &CopyUrl, window, cx| this.copy_url(window, cx)));
                r.on_action(cx.listener(|this, _: &CopyCurlCommand, window, cx| {
                    this.copy_curl_command(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::CopyResponse, window, cx| {
                    this.copy_response(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::CopyLinksInResponse, window, cx| {
                    this.copy_links_in_response(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &crate::OpenResponseInBrowser, window, cx| {
                    this.open_current_response_in_browser(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &CopyPrettified, window, cx| {
                    this.copy_prettified(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &ChangeRequestMethod, window, cx| {
                    this.change_request_method(window, cx)
                }));
                r.on_action(cx.listener(|this, _: &PasteUrlAsRequest, window, cx| {
                    this.paste_url_as_request(window, cx)
                }));
                r.on_action(
                    cx.listener(|this, _: &crate::DeleteSelectedHttpHistory, window, cx| {
                        this.delete_selected_http_history(window, cx)
                    }),
                );
                r.on_action(
                    cx.listener(|this, _: &crate::ClearLiveProxyHistory, window, cx| {
                        this.clear_live_proxy_history(window, cx)
                    }),
                );
                r.on_action(cx.listener(|this, _: &crate::AddToScope, window, cx| {
                    this.add_to_scope(window, cx)
                }));
                r.on_action(
                    cx.listener(|this, _: &crate::ExcludeFromScope, window, cx| {
                        this.exclude_from_scope(window, cx)
                    }),
                );
                r.on_action(cx.listener(Self::convert_selection_action));
                r.on_action(cx.listener(Self::dispatch_copy_converted_selection_action));
                r.on_action(cx.listener(
                    |this, _: &editing::OpenSelectionInCyberChef, window, cx| {
                        this.open_selection_in_cyberchef(window, cx)
                    },
                ));
                r.on_action(cx.listener(
                    |this, action: &editing::InvokePluginContextMenu, window, cx| {
                        this.invoke_plugin_context_menu(action.0, window, cx)
                    },
                ));
                r.on_action(cx.listener(|this, _: &SendRequest, window, cx| this.send(window, cx)));
                r.on_action(cx.listener(|this, _: &CancelRequest, _, cx| this.cancel(cx)));
                r.on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)));
                r.on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                    let active = match this.tool {
                        Tool::Dashboard | Tool::Target | Tool::Sequencer => None,
                        Tool::Collaborator => None,
                        Tool::Intruder => this.workspace.intruder.active,
                        Tool::Repeater => this
                            .workspace
                            .active
                            .or(this.workspace.active_websocket_tab),
                        _ => None,
                    };
                    if let Some(id) = active {
                        this.close_tab(id, window, cx);
                    }
                }));
                r.on_action(cx.listener(|this, _: &crate::CloseOtherTabs, window, cx| {
                    if this.tool != Tool::Repeater {
                        return;
                    }
                    let Some(active_id) = this
                        .workspace
                        .active
                        .or(this.workspace.active_websocket_tab)
                    else {
                        return;
                    };
                    let mut ids: Vec<TabId> = this
                        .workspace
                        .tabs
                        .iter()
                        .map(|tab| tab.id)
                        .filter(|id| *id != active_id)
                        .collect();
                    ids.extend(
                        this.workspace
                            .websocket_tabs
                            .iter()
                            .filter(|tab| !tab.closed && tab.id != active_id)
                            .map(|tab| tab.id),
                    );
                    this.close_tabs(ids, window, cx);
                }));
                r.on_action(
                    cx.listener(|this, _: &RestoreTab, window, cx| this.restore_tab(window, cx)),
                );
                r.on_action(
                    cx.listener(|this, _: &NextTab, window, cx| this.step_tab(1, window, cx)),
                );
                r.on_action(
                    cx.listener(|this, _: &PreviousTab, window, cx| this.step_tab(-1, window, cx)),
                );
                r.on_action(
                    cx.listener(|this, _: &MoveTabUp, window, cx| this.move_active(-1, window, cx)),
                );
                r.on_action(
                    cx.listener(|this, _: &MoveTabDown, window, cx| {
                        this.move_active(1, window, cx)
                    }),
                );
                r.on_action(cx.listener(|this, _: &SelectToolDashboard, window, cx| {
                    this.select_tool(Tool::Dashboard, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolTarget, window, cx| {
                    this.select_tool(Tool::Target, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolProxy, window, cx| {
                    this.select_tool(Tool::Proxy, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolIntruder, window, cx| {
                    this.select_tool(Tool::Intruder, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolRepeater, window, cx| {
                    this.select_tool(Tool::Repeater, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolCollaborator, window, cx| {
                    this.select_tool(Tool::Collaborator, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolSequencer, window, cx| {
                    this.select_tool(Tool::Sequencer, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolDecoder, window, cx| {
                    this.select_tool(Tool::Decoder, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolComparer, window, cx| {
                    this.select_tool(Tool::Comparer, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolLogger, window, cx| {
                    this.select_tool(Tool::Logger, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolOrganiser, window, cx| {
                    this.select_tool(Tool::Organiser, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolExtensions, window, cx| {
                    this.select_tool(Tool::Extensions, window, cx)
                }));
                r.on_action(cx.listener(|this, _: &SelectToolDiscover, window, cx| {
                    this.select_tool(Tool::Discover, window, cx)
                }));
                r.on_action(cx.listener(
                    |this, _: &ToggleTabOrientation, window, cx| {
                        this.toggle_tab_orientation(window, cx)
                    },
                ));
        }
        root
    }
}
