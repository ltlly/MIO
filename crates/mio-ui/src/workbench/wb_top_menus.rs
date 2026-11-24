use super::*;
use crate::workbench::settings::{SettingsPage, SettingsScopeFilter};
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};

impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn bind_extended_top_menu_actions<E: InteractiveElement>(
        &self,
        root: &mut E,
        cx: &mut Context<Self>,
    ) {
        let r = root.interactivity();
        r.on_action(
                cx.listener(|this, _: &crate::OpenUserSettings, window, cx| {
                    this.open_settings_filtered(
                        Some(SettingsPage::UserMessageEditor),
                        Some(SettingsScopeFilter::User),
                        false,
                        window,
                        cx,
                    )
                }),
        );
        r.on_action(
                cx.listener(|this, _: &crate::OpenProjectSettings, window, cx| {
                    this.open_settings_filtered(
                        Some(SettingsPage::ProjectScope),
                        Some(SettingsScopeFilter::Project),
                        false,
                        window,
                        cx,
                    )
                }),
        );
        r.on_action(cx.listener(|this, _: &crate::SearchSettings, window, cx| {
                this.open_settings_filtered(None, Some(SettingsScopeFilter::All), true, window, cx)
            }));
        r.on_action(
                cx.listener(|this, _: &crate::OpenProxySettings, window, cx| {
                    this.open_settings_filtered(Some(SettingsPage::Proxy), None, false, window, cx)
                }),
        );
        r.on_action(
                cx.listener(|this, _: &crate::OpenIntruderSettings, window, cx| {
                    this.open_settings_filtered(
                        Some(SettingsPage::Intruder),
                        None,
                        false,
                        window,
                        cx,
                    )
                }),
        );
        r.on_action(
                cx.listener(|this, _: &crate::OpenRepeaterSettings, window, cx| {
                    this.open_settings_filtered(
                        Some(SettingsPage::Repeater),
                        None,
                        false,
                        window,
                        cx,
                    )
                }),
        );
        r.on_action(
                cx.listener(|this, _: &crate::OpenScopeSettings, window, cx| {
                    this.open_settings_filtered(
                        Some(SettingsPage::ProjectScope),
                        None,
                        false,
                        window,
                        cx,
                    )
                }),
        );
        if self.ext_import.visible {
            return;
        }
        r.on_action(
            cx.listener(|this, _: &crate::OpenConfiguredBrowser, _, cx| {
                this.open_configured_browser(cx)
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ToggleProxyIntercept, _, cx| this.toggle_intercept(cx)),
        );
        r.on_action(
            cx.listener(|this, _: &crate::StartIntruderAttack, window, cx| {
                if this.tool != Tool::Intruder {
                    this.select_tool(Tool::Intruder, window, cx);
                }
                this.start_intruder(window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::OpenSavedIntruderAttacks, window, cx| {
                if this.tool != Tool::Intruder {
                    this.select_tool(Tool::Intruder, window, cx);
                }
                this.activate_intruder_dashboard(cx);
                let saved_run = this
                    .intruder
                    .results
                    .runs
                    .rows
                    .first()
                    .map(|run| run.id);
                if let Some(run_id) = saved_run {
                    this.open_intruder_results_window(run_id, cx);
                    this.status = "已打开已保存的 Intruder 攻击结果窗口。".into();
                } else {
                    this.status = "已切换到 Intruder 并刷新已保存的攻击记录。".into();
                }
                cx.notify();
            }),
        );
        r.on_action(cx.listener(|_, _: &crate::MinimizeWindow, window, _| {
            window.minimize_window();
        }));
        r.on_action(cx.listener(|_, _: &crate::ZoomWindow, window, _| {
            window.zoom_window();
        }));
        r.on_action(
            cx.listener(|this, _: &crate::ToggleSettingsWindow, window, cx| {
                if let Some(settings_win) = this.settings_window
                    && settings_win
                        .update(cx, |_, win, _| win.remove_window())
                        .is_ok()
                    {
                        this.settings_window = None;
                        cx.notify();
                        return;
                    }
                this.open_settings(window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::BringAllWindowsToFront, _, cx| {
                cx.activate(true);
                this.status = "已前置全部 MIO 窗口。".into();
                cx.notify();
            }),
        );
        r.on_action(cx.listener(|this, _: &crate::OpenDocumentation, _, cx| {
            cx.open_url("https://github.com/ltlly/MIO");
            this.status = "已在浏览器中打开 MIO 使用文档。".into();
            cx.notify();
        }));
        r.on_action(
            cx.listener(|this, _: &crate::OpenKeyboardShortcuts, window, cx| {
                this.open_settings_filtered(
                    Some(SettingsPage::UserMisc),
                    Some(SettingsScopeFilter::User),
                    false,
                    window,
                    cx,
                );
            }),
        );
        r.on_action(cx.listener(|this, _: &crate::RunSystemDiagnostics, _, cx| {
            let listeners = this.workspace.proxy_listeners.len();
            let tabs = this.workspace.tabs.len();
            let findings = this.audit_dashboard.snapshot.findings.len();
            this.error = None;
            this.status = format!(
                "系统诊断正常：代理监听器 {listeners} 个 · Repeater 标签 {tabs} 个 · 审计告警 {findings} 条"
            );
            cx.notify();
        }));
        r.on_action(cx.listener(|this, _: &crate::OpenAboutMio, _, cx| {
            this.error = None;
            this.status = format!(
                "MIO v{} — 原生高性能 Web 安全测试与流量分析工作台",
                env!("CARGO_PKG_VERSION")
            );
            cx.notify();
        }));
        r.on_action(
            cx.listener(|this, _: &crate::AutoMarkIntruderPositions, window, cx| {
                if let Some(id) = this.workspace.intruder.active {
                    if this.tool != Tool::Intruder {
                        this.select_tool(Tool::Intruder, window, cx);
                    }
                    this.sync_intruder_editors(window, cx);
                    this.auto_intruder_positions(id, window, cx);
                } else {
                    this.fail("当前没有打开的 Intruder 标签页。", cx);
                }
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ClearIntruderPositions, window, cx| {
                if let Some(id) = this.workspace.intruder.active {
                    if this.tool != Tool::Intruder {
                        this.select_tool(Tool::Intruder, window, cx);
                    }
                    this.sync_intruder_editors(window, cx);
                    this.clear_intruder_positions(id, window, cx);
                } else {
                    this.fail("当前没有打开的 Intruder 标签页。", cx);
                }
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::DuplicateActiveTab, window, cx| {
                if this.tool != Tool::Repeater {
                    return;
                }
                if let Some(id) = this.workspace.active {
                    this.duplicate_repeater_tab(id, window, cx);
                } else if let Some(ws_id) = this.workspace.active_websocket_tab {
                    if this.project.is_none()
                        || !this.backend_available()
                        || this.transition.is_some()
                        || !this.flush_workspace(window, cx)
                    {
                        return;
                    }
                    this.duplicate_ws_repeater_tab(ws_id, cx);
                }
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::RestoreDefaultTabLayout, window, cx| {
                this.restore_default_tab_layout(window, cx);
            }),
        );
        r.on_action(cx.listener(
            |this, _: &crate::ToggleRepeaterUpdateContentLength, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.update_content_length = !s.update_content_length;
                    format!(
                        "Repeater 更新 Content-Length：{}",
                        if s.update_content_length {
                            "已开启"
                        } else {
                            "已关闭"
                        }
                    )
                });
            },
        ));
        r.on_action(cx.listener(
            |this, _: &crate::ToggleRepeaterUnpackCompressed, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.unpack_compressed_responses = !s.unpack_compressed_responses;
                    format!(
                        "Repeater 解包压缩响应：{}",
                        if s.unpack_compressed_responses {
                            "已开启"
                        } else {
                            "已关闭"
                        }
                    )
                });
            },
        ));
        r.on_action(cx.listener(
            |this, _: &crate::ToggleRepeaterNormalizeLineEndings, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.normalize_http1_line_endings = !s.normalize_http1_line_endings;
                    format!(
                        "Repeater 规范化 HTTP/1 换行符：{}",
                        if s.normalize_http1_line_endings {
                            "已开启"
                        } else {
                            "已关闭"
                        }
                    )
                });
            },
        ));
        r.on_action(
            cx.listener(|this, _: &crate::ToggleRepeaterHttp1Close, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.enforce_http1_connection_close = !s.enforce_http1_connection_close;
                    format!(
                        "Repeater HTTP/1 强制 Connection: close：{}",
                        if s.enforce_http1_connection_close {
                            "已开启"
                        } else {
                            "已关闭"
                        }
                    )
                });
            }),
        );
        r.on_action(cx.listener(
            |this, _: &crate::ToggleRepeaterStripH2Connection, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.strip_connection_headers_over_http2 = !s.strip_connection_headers_over_http2;
                    format!(
                        "Repeater HTTP/2 剥离 Connection 标头：{}",
                        if s.strip_connection_headers_over_http2 {
                            "已开启"
                        } else {
                            "已关闭"
                        }
                    )
                });
            },
        ));
        r.on_action(
            cx.listener(|this, _: &crate::SetRepeaterRedirectNever, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.follow_redirects = RepeaterRedirectMode::Never;
                    "Repeater 跟随重定向：从不 (Never)".into()
                });
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::SetRepeaterRedirectOnSite, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.follow_redirects = RepeaterRedirectMode::OnSiteOnly;
                    "Repeater 跟随重定向：仅同站 (On-site only)".into()
                });
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::SetRepeaterRedirectInScope, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.follow_redirects = RepeaterRedirectMode::InScopeOnly;
                    "Repeater 跟随重定向：仅范围内 (In-scope only)".into()
                });
            }),
        );
        r.on_action(cx.listener(
            |this, _: &crate::SetRepeaterRedirectAlways, window, cx| {
                this.mutate_repeater_menu_setting(window, cx, |s| {
                    s.follow_redirects = RepeaterRedirectMode::Always;
                    "Repeater 跟随重定向：始终 (Always)".into()
                });
            },
        ));
        r.on_action(
            cx.listener(|this, _: &crate::ChangeRequestMethodGet, window, cx| {
                this.set_request_method_to("GET", window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ChangeRequestMethodPost, window, cx| {
                this.set_request_method_to("POST", window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ChangeRequestMethodPut, window, cx| {
                this.set_request_method_to("PUT", window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ChangeRequestMethodDelete, window, cx| {
                this.set_request_method_to("DELETE", window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ChangeRequestMethodPatch, window, cx| {
                this.set_request_method_to("PATCH", window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ChangeRequestMethodOptions, window, cx| {
                this.set_request_method_to("OPTIONS", window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ChangeBodyEncodingToggle, window, cx| {
                this.change_request_body_encoding(
                    wb_shortcuts::BodyEncodingTarget::Toggle,
                    window,
                    cx,
                );
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ChangeBodyEncodingForm, window, cx| {
                this.change_request_body_encoding(
                    wb_shortcuts::BodyEncodingTarget::FormUrlEncoded,
                    window,
                    cx,
                );
            }),
        );
        r.on_action(cx.listener(
            |this, _: &crate::ChangeBodyEncodingMultipart, window, cx| {
                this.change_request_body_encoding(
                    wb_shortcuts::BodyEncodingTarget::MultipartFormData,
                    window,
                    cx,
                );
            },
        ));
        r.on_action(
            cx.listener(|this, _: &crate::ChangeBodyEncodingJson, window, cx| {
                this.change_request_body_encoding(
                    wb_shortcuts::BodyEncodingTarget::ApplicationJson,
                    window,
                    cx,
                );
            }),
        );
        r.on_action(cx.listener(|this, _: &crate::CopyToFile, window, cx| {
            this.copy_to_file(false, window, cx);
        }));
        r.on_action(
            cx.listener(|this, _: &crate::SaveSelectedTextToFile, window, cx| {
                this.save_focused_selected_text_to_file(window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::CopyResponseBodyToFile, window, cx| {
                this.copy_to_file(true, window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::ExportSelectedHistoryXml, window, cx| {
                this.export_selected_history_xml(window, cx);
            }),
        );
        r.on_action(cx.listener(|this, _: &crate::PasteFromFile, window, cx| {
            this.paste_from_file(false, window, cx);
        }));
        r.on_action(
            cx.listener(|this, _: &crate::PasteBase64FromFile, window, cx| {
                this.paste_from_file(true, window, cx);
            }),
        );
        r.on_action(
            cx.listener(|this, _: &crate::InsertCollaboratorPayload, window, cx| {
                this.insert_collaborator_payload(window, cx);
            }),
        );
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_cross_platform_menu_button(
        &self,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let rep = self.workspace.repeater_settings.clone();
        let intercept_on = self.intercept_requests || self.intercept_responses;
        let active_tool = self.tool;
        let picker_busy = self.file_picker_pending();
        let unavailable = self.project.is_none()
            || !self.backend_available()
            || self.transition.is_some()
            || picker_busy;
        Button::new("footer-top-menus")
            .small()
            .ghost()
            .compact()
            .rounded(gpui_kit::component::button::ButtonRounded::None)
            .tab_stop(false)
            .w(px(29.))
            .h_full()
            .p_0()
            .icon(
                gpui_kit::component::Icon::new(gpui_kit::assets::IconName::List)
                    .size(px(12.)),
            )
            .accessibility_label("菜单")
            .tooltip("打开工作台完整命令菜单（项目、编辑、视图、Target、Proxy、Intruder、Repeater）")
            .dropdown_menu(move |menu, window, cx| {
                let check_label = |on: bool, label: &str| {
                    if on {
                        format!("✓ {label}")
                    } else {
                        format!("   {label}")
                    }
                };
                menu.submenu("项目", window, cx, move |sub, _, _| {
                    sub.item(
                        PopupMenuItem::new("新建窗口")
                            .action(Box::new(crate::NewWindow)),
                    )
                    .item(
                        PopupMenuItem::new("新建项目")
                            .action(Box::new(NewProject))
                            .disabled(unavailable),
                    )
                    .item(
                        PopupMenuItem::new("打开项目…")
                            .action(Box::new(OpenProject))
                            .disabled(unavailable),
                    )
                    .item(
                        PopupMenuItem::new("在新窗口中打开项目…")
                            .action(Box::new(crate::OpenProjectInNewWindow))
                            .disabled(picker_busy),
                    )
                    .item(
                        PopupMenuItem::new("导入 外部归档项目…")
                            .action(Box::new(ImportExtProject))
                            .disabled(unavailable),
                    )
                    .item(
                        PopupMenuItem::new("查看 外部导入记录")
                            .action(Box::new(ShowExtReport))
                            .disabled(unavailable),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("项目设置…")
                            .action(Box::new(crate::OpenProjectSettings)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("保存项目")
                            .action(Box::new(SaveProject))
                            .disabled(unavailable),
                    )
                    .item(
                        PopupMenuItem::new("另存为…")
                            .action(Box::new(SaveProjectAs))
                            .disabled(unavailable),
                    )
                    .item(
                        PopupMenuItem::new("瘦身另存为…")
                            .action(Box::new(crate::SlimSaveProjectAs))
                            .disabled(unavailable),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("导出 Proxy CA…")
                            .action(Box::new(ExportCa))
                            .disabled(unavailable),
                    )
                })
                .submenu("编辑", window, cx, move |sub, _, _| {
                    sub.item(PopupMenuItem::new("撤销").action(Box::new(Undo)))
                        .item(PopupMenuItem::new("重做").action(Box::new(Redo)))
                        .separator()
                        .item(PopupMenuItem::new("剪切").action(Box::new(Cut)))
                        .item(PopupMenuItem::new("复制").action(Box::new(Copy)))
                        .item(PopupMenuItem::new("粘贴").action(Box::new(Paste)))
                        .item(PopupMenuItem::new("全选").action(Box::new(SelectAll)))
                        .separator()
                        .item(
                            PopupMenuItem::new("复制美化报文")
                                .action(Box::new(CopyPrettified))
                                .disabled(unavailable),
                        )
                        .item(
                            PopupMenuItem::new("保存到文件…")
                                .action(Box::new(crate::CopyToFile))
                                .disabled(unavailable),
                        )
                        .item(
                            PopupMenuItem::new("从文件粘贴…")
                                .action(Box::new(crate::PasteFromFile))
                                .disabled(unavailable),
                        )
                        .item(
                            PopupMenuItem::new("从文件 Base64 编码插入…")
                                .action(Box::new(crate::PasteBase64FromFile))
                                .disabled(unavailable),
                        )
                        .item(
                            PopupMenuItem::new("插入 Collaborator Payload")
                                .action(Box::new(crate::InsertCollaboratorPayload))
                                .disabled(unavailable),
                        )
                        .item(
                            PopupMenuItem::new("在 CyberChef 中打开")
                                .action(Box::new(editing::OpenSelectionInCyberChef)),
                        )
                })
                .submenu("视图", window, cx, move |sub, _, _| {
                    sub.item(
                        PopupMenuItem::new("恢复默认标签布局")
                            .action(Box::new(crate::RestoreDefaultTabLayout)),
                    )
                    .item(
                        PopupMenuItem::new("切换工作标签方向")
                            .action(Box::new(ToggleTabOrientation)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Dashboard, "Dashboard"))
                            .checked(active_tool == Tool::Dashboard)
                            .action(Box::new(SelectToolDashboard)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Target, "Target"))
                            .checked(active_tool == Tool::Target)
                            .action(Box::new(SelectToolTarget)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Proxy, "Proxy"))
                            .checked(active_tool == Tool::Proxy)
                            .action(Box::new(SelectToolProxy)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Intruder, "Intruder"))
                            .checked(active_tool == Tool::Intruder)
                            .action(Box::new(SelectToolIntruder)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Repeater, "Repeater"))
                            .checked(active_tool == Tool::Repeater)
                            .action(Box::new(SelectToolRepeater)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(
                            active_tool == Tool::Collaborator,
                            "Collaborator",
                        ))
                        .checked(active_tool == Tool::Collaborator)
                        .action(Box::new(SelectToolCollaborator)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Sequencer, "Sequencer"))
                            .checked(active_tool == Tool::Sequencer)
                            .action(Box::new(SelectToolSequencer)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Decoder, "Decoder"))
                            .checked(active_tool == Tool::Decoder)
                            .action(Box::new(SelectToolDecoder)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Comparer, "Comparer"))
                            .checked(active_tool == Tool::Comparer)
                            .action(Box::new(SelectToolComparer)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Logger, "Logger"))
                            .checked(active_tool == Tool::Logger)
                            .action(Box::new(SelectToolLogger)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(active_tool == Tool::Organiser, "Organiser"))
                            .checked(active_tool == Tool::Organiser)
                            .action(Box::new(SelectToolOrganiser)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(
                            active_tool == Tool::Extensions,
                            "Extensions",
                        ))
                        .checked(active_tool == Tool::Extensions)
                        .action(Box::new(SelectToolExtensions)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(
                            active_tool == Tool::Discover,
                            "Discover",
                        ))
                        .checked(active_tool == Tool::Discover)
                        .action(Box::new(SelectToolDiscover)),
                    )
                })
                .submenu("Target", window, cx, move |sub, _, _| {
                    sub.item(
                        PopupMenuItem::new("添加到范围")
                            .action(Box::new(crate::AddToScope))
                            .disabled(unavailable),
                    )
                    .item(
                        PopupMenuItem::new("从范围排除")
                            .action(Box::new(crate::ExcludeFromScope))
                            .disabled(unavailable),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("目标范围设置…")
                            .action(Box::new(crate::OpenScopeSettings)),
                    )
                })
                .submenu("Proxy", window, cx, move |sub, _, _| {
                    sub.item(
                        PopupMenuItem::new("打开配置浏览器")
                            .action(Box::new(crate::OpenConfiguredBrowser))
                            .disabled(unavailable),
                    )
                    .item(
                        PopupMenuItem::new(check_label(intercept_on, "切换 Intercept 开关"))
                            .checked(intercept_on)
                            .action(Box::new(crate::ToggleProxyIntercept)),
                    )
                    .separator()
                    .item(PopupMenuItem::new("复制 URL").action(Box::new(CopyUrl)))
                    .item(
                        PopupMenuItem::new("复制为 curl 命令").action(Box::new(CopyCurlCommand)),
                    )
                    .item(
                        PopupMenuItem::new("复制响应").action(Box::new(crate::CopyResponse)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("清空实时历史记录")
                            .action(Box::new(crate::ClearLiveProxyHistory)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("Proxy 设置…")
                            .action(Box::new(crate::OpenProxySettings)),
                    )
                })
                .submenu("Intruder", window, cx, move |sub, _, _| {
                    sub.item(
                        PopupMenuItem::new("开始攻击").action(Box::new(crate::StartIntruderAttack)),
                    )
                    .item(
                        PopupMenuItem::new("打开已保存的攻击")
                            .action(Box::new(crate::OpenSavedIntruderAttacks)),
                    )
                    .item(
                        PopupMenuItem::new("发起主动扫描 (Scan)")
                            .action(Box::new(crate::SendToScanner)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("发送到 Intruder").action(Box::new(OpenInIntruder)),
                    )
                    .item(
                        PopupMenuItem::new("发送到 Repeater").action(Box::new(OpenInRepeater)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("自动标记 Payload 位置 (§)")
                            .action(Box::new(crate::AutoMarkIntruderPositions)),
                    )
                    .item(
                        PopupMenuItem::new("清除 Payload 位置标记")
                            .action(Box::new(crate::ClearIntruderPositions)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("关闭攻击结果设置…")
                            .action(Box::new(crate::OpenIntruderSettings)),
                    )
                })
                .submenu("Repeater", window, cx, move |sub, window, cx| {
                    let redir = rep.follow_redirects;
                    sub.item(
                        PopupMenuItem::new(check_label(
                            rep.update_content_length,
                            "更新 Content-Length",
                        ))
                        .checked(rep.update_content_length)
                        .action(Box::new(crate::ToggleRepeaterUpdateContentLength)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(
                            rep.unpack_compressed_responses,
                            "解包压缩响应",
                        ))
                        .checked(rep.unpack_compressed_responses)
                        .action(Box::new(crate::ToggleRepeaterUnpackCompressed)),
                    )
                    .submenu("跟随重定向", window, cx, move |r_sub, _, _| {
                        r_sub
                            .item(
                                PopupMenuItem::new(check_label(
                                    redir == RepeaterRedirectMode::Never,
                                    "从不 (Never)",
                                ))
                                .checked(redir == RepeaterRedirectMode::Never)
                                .action(Box::new(crate::SetRepeaterRedirectNever)),
                            )
                            .item(
                                PopupMenuItem::new(check_label(
                                    redir == RepeaterRedirectMode::OnSiteOnly,
                                    "仅同站 (On-site only)",
                                ))
                                .checked(redir == RepeaterRedirectMode::OnSiteOnly)
                                .action(Box::new(crate::SetRepeaterRedirectOnSite)),
                            )
                            .item(
                                PopupMenuItem::new(check_label(
                                    redir == RepeaterRedirectMode::InScopeOnly,
                                    "仅范围内 (In-scope only)",
                                ))
                                .checked(redir == RepeaterRedirectMode::InScopeOnly)
                                .action(Box::new(crate::SetRepeaterRedirectInScope)),
                            )
                            .item(
                                PopupMenuItem::new(check_label(
                                    redir == RepeaterRedirectMode::Always,
                                    "始终 (Always)",
                                ))
                                .checked(redir == RepeaterRedirectMode::Always)
                                .action(Box::new(crate::SetRepeaterRedirectAlways)),
                            )
                    })
                    .item(
                        PopupMenuItem::new(check_label(
                            rep.normalize_http1_line_endings,
                            "规范化 HTTP/1 换行符",
                        ))
                        .checked(rep.normalize_http1_line_endings)
                        .action(Box::new(crate::ToggleRepeaterNormalizeLineEndings)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(
                            rep.enforce_http1_connection_close,
                            "HTTP/1 强制 Connection: close",
                        ))
                        .checked(rep.enforce_http1_connection_close)
                        .action(Box::new(crate::ToggleRepeaterHttp1Close)),
                    )
                    .item(
                        PopupMenuItem::new(check_label(
                            rep.strip_connection_headers_over_http2,
                            "HTTP/2 自动剥离 Connection 标头",
                        ))
                        .checked(rep.strip_connection_headers_over_http2)
                        .action(Box::new(crate::ToggleRepeaterStripH2Connection)),
                    )
                    .item(
                        PopupMenuItem::new("流式响应与超时设置…")
                            .action(Box::new(crate::OpenRepeaterSettings)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("发送到 Repeater").action(Box::new(OpenInRepeater)),
                    )
                    .item(PopupMenuItem::new("新建标签页").action(Box::new(NewTab)))
                    .item(
                        PopupMenuItem::new("复制标签页").action(Box::new(crate::DuplicateActiveTab)),
                    )
                    .item(PopupMenuItem::new("发送请求").action(Box::new(SendRequest)))
                    .item(PopupMenuItem::new("取消请求").action(Box::new(CancelRequest)))
                    .separator()
                    .item(
                        PopupMenuItem::new("切换请求方法 (GET/POST)")
                            .action(Box::new(ChangeRequestMethod)),
                    )
                    .submenu("转换请求体编码", window, cx, |enc_sub, _, _| {
                        enc_sub
                            .item(
                                PopupMenuItem::new("循环切换请求体编码")
                                    .action(Box::new(crate::ChangeBodyEncodingToggle)),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("转为 Form URL-encoded")
                                    .action(Box::new(crate::ChangeBodyEncodingForm)),
                            )
                            .item(
                                PopupMenuItem::new("转为 Multipart/form-data")
                                    .action(Box::new(crate::ChangeBodyEncodingMultipart)),
                            )
                            .item(
                                PopupMenuItem::new("转为 Application/JSON")
                                    .action(Box::new(crate::ChangeBodyEncodingJson)),
                            )
                    })
                    .item(
                        PopupMenuItem::new("粘贴 URL 为请求").action(Box::new(PasteUrlAsRequest)),
                    )
                    .separator()
                    .item(PopupMenuItem::new("关闭标签页").action(Box::new(CloseTab)))
                    .item(
                        PopupMenuItem::new("重新打开已关闭标签").action(Box::new(RestoreTab)),
                    )
                })
                .submenu("窗口", window, cx, move |sub, _, _| {
                    sub.item(
                        PopupMenuItem::new("新建窗口")
                            .action(Box::new(crate::NewWindow)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("最小化")
                            .action(Box::new(crate::MinimizeWindow)),
                    )
                    .item(
                        PopupMenuItem::new("缩放窗口")
                            .action(Box::new(crate::ZoomWindow)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("切换设置窗口")
                            .action(Box::new(crate::ToggleSettingsWindow)),
                    )
                    .item(
                        PopupMenuItem::new("前置全部窗口")
                            .action(Box::new(crate::BringAllWindowsToFront)),
                    )
                })
                .submenu("帮助", window, cx, move |sub, _, _| {
                    sub.item(
                        PopupMenuItem::new("MIO 使用文档")
                            .action(Box::new(crate::OpenDocumentation)),
                    )
                    .item(
                        PopupMenuItem::new("快捷键列表…")
                            .action(Box::new(crate::OpenKeyboardShortcuts)),
                    )
                    .item(
                        PopupMenuItem::new("运行系统与代理诊断")
                            .action(Box::new(crate::RunSystemDiagnostics)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("关于 MIO")
                            .action(Box::new(crate::OpenAboutMio)),
                    )
                })
            })
            .into_any_element()
    }

    fn restore_default_tab_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || !self.backend_available() || self.transition.is_some() {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let changed = self.workspace.orientation != Orientation::Horizontal
            || self.workspace.intruder.orientation != Orientation::Horizontal;
        self.workspace.orientation = Orientation::Horizontal;
        self.workspace.intruder.orientation = Orientation::Horizontal;
        if changed {
            let _ = self.save_workspace(cx);
        }
        self.status = "已恢复默认水平工作标签布局。".into();
        cx.notify();
    }

    fn mutate_repeater_menu_setting(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut RepeaterSettings) -> String,
    ) {
        if self.project.is_none() || !self.backend_available() || self.transition.is_some() {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let summary = apply(&mut self.workspace.repeater_settings);
        if self.save_workspace(cx) {
            self.status = summary;
        }
        cx.notify();
    }
}
