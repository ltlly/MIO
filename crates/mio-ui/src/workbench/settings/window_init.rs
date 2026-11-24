use gpui_kit::{AnyWindowHandle, ScrollHandle, size};

use super::*;

#[inline(never)]
fn boxed_with<T>(build: impl FnOnce() -> T) -> Box<T> {
    Box::new(build())
}

impl Workbench {
    pub(in crate::workbench) fn open_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_filtered(None, None, false, window, cx);
    }

    pub(in crate::workbench) fn open_project_tasks_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_filtered(Some(SettingsPage::ProjectTasks), None, false, window, cx);
    }

    pub(in crate::workbench) fn open_project_tasks_scheduled_prefill(
        &mut self,
        task_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_inner(
            Some(SettingsPage::ProjectTasks),
            None,
            false,
            Some(task_name),
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn open_settings_filtered(
        &mut self,
        page: Option<SettingsPage>,
        scope: Option<SettingsScopeFilter>,
        focus_search: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_inner(page, scope, focus_search, None, window, cx);
    }

    fn open_settings_inner(
        &mut self,
        page: Option<SettingsPage>,
        scope: Option<SettingsScopeFilter>,
        focus_search: bool,
        scheduled_task_prefill: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity();
        let owner_window = window.window_handle();
        // Synchronous window creation requires the owner update to have finished.
        cx.defer(move |cx| {
            if !cx.windows().contains(&owner_window) {
                return;
            }
            let existing = owner.read(cx).settings_window;
            if let Some(settings) = existing
                && settings
                    .update(cx, |_, window, cx| {
                        if let Some(settings) = settings_entity(window, cx) {
                            settings.update(cx, |settings, cx| {
                                if let Some(scope) = scope {
                                    settings.set_scope_filter(scope, cx);
                                }
                                if let Some(page) = page {
                                    if !settings.is_page_visible(page, cx) {
                                        settings.scope_filter = SettingsScopeFilter::All;
                                        settings.search_input.update(cx, |input, cx| {
                                            if input.marked_text_range(window, cx).is_none() {
                                                input.set_value("", window, cx);
                                            }
                                        });
                                    }
                                    settings.select_page(page, window, cx);
                                }
                                if let Some(prefill_name) = scheduled_task_prefill.clone() {
                                    settings.open_scheduled_task_prefill(prefill_name, window, cx);
                                }
                                if focus_search {
                                    settings.search_input.update(cx, |input, cx| {
                                        input.focus(window, cx);
                                    });
                                }
                            });
                        }
                        window.activate_window();
                        cx.activate(true);
                    })
                    .is_ok()
            {
                return;
            }
            let state = boxed_with(|| SettingsState::new(owner.read(cx)));
            let scale = theme::ui_scale();
            let desired_w = if cfg!(test) || scale <= 1.0 {
                px(1327.)
            } else {
                theme::scaled_px(1327.)
            };
            let desired_h = if cfg!(test) {
                px(790.)
            } else {
                let content_h = if scale <= 1.0 {
                    px(790.)
                } else {
                    theme::scaled_px(790.)
                };
                content_h
                    + theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                        .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
            };
            let bounds = crate::fit_window_bounds(size(desired_w, desired_h), cx);
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, true, true),
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut view = Settings::new(
                            owner.clone(),
                            owner_window,
                            state,
                            page.unwrap_or(SettingsPage::Repeater),
                            window,
                            cx,
                        );
                        if let Some(scope) = scope {
                            view.set_scope_filter(scope, cx);
                        }
                        if let Some(prefill_name) = scheduled_task_prefill.clone() {
                            view.open_scheduled_task_prefill(prefill_name, window, cx);
                        }
                        if focus_search {
                            view.search_input
                                .update(cx, |input, cx| input.focus(window, cx));
                        }
                        view
                    })
                },
            );
            owner.update(cx, |view, cx| match opened {
                Ok((window, _)) => view.settings_window = Some(window),
                Err(error) => view.fail(format!("无法打开设置窗口：{error}"), cx),
            });
        });
    }
}

impl Settings {
    pub(in crate::workbench) fn new(
        owner: Entity<Workbench>,
        owner_window: AnyWindowHandle,
        state: Box<SettingsState>,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let (
                project_changed,
                normal_timeout_changed,
                streaming_timeout_changed,
                redirect_max_hops_changed,
                network_http_changed,
                scheduled_tasks_changed,
                observed_certs_changed,
                listeners_changed,
                upstream_changed,
                legacy_changed,
                certificates_changed,
                client_certs_changed,
                scope_changed,
                interception_changed,
                sessions_changed,
                user_prefs_changed,
            ) = {
                let view = owner.read(cx);
                let project_changed = this.state.project != view.project;
                let normal_timeout_changed = project_changed
                    || this.state.request_timeout_secs
                        != view.workspace.repeater_settings.request_timeout_secs;
                let streaming_timeout_changed = project_changed
                    || this.state.repeater_settings.streaming_response_timeout_secs
                        != view
                            .workspace
                            .repeater_settings
                            .streaming_response_timeout_secs;
                let redirect_max_hops_changed = project_changed
                    || this
                        .state
                        .repeater_settings
                        .auto_follow_redirections_max_hops
                        != view
                            .workspace
                            .repeater_settings
                            .auto_follow_redirections_max_hops;
                let network_http_changed =
                    project_changed || this.state.network_http != view.workspace.network_http;
                let scheduled_tasks_changed =
                    project_changed || this.state.scheduled_tasks != view.workspace.scheduled_tasks;
                let observed_certs_changed = project_changed
                    || this.state.observed_server_certs != view.observed_server_certs;
                let listeners_changed = project_changed
                    || this.state.proxy_listeners != view.workspace.effective_proxy_listeners()
                    || this.state.active_proxy_address != view.proxy_address
                    || this.state.active_proxy_listeners != view.proxy_listeners_bound
                    || this.state.proxy_busy != view.proxy_busy
                    || this.state.last_ca_fingerprint != view.last_ca_fingerprint;
                let upstream_changed =
                    project_changed || this.state.upstream_proxy != view.workspace.upstream_proxy;
                let legacy_changed = this.state.upstream_tls != view.workspace.upstream_tls;
                let client_certs_changed = project_changed
                    || this.state.client_certificates != view.workspace.client_certificates;
                let scope_changed = project_changed || this.state.scope != view.workspace.scope;
                let interception_changed = project_changed
                    || this.state.proxy_interception != view.workspace.proxy_interception;
                let sessions_changed =
                    project_changed || this.state.sessions != view.workspace.sessions;
                let prev_user_rev = this
                    .state
                    .preferences
                    .confirmed
                    .as_ref()
                    .map(|prefs| prefs.revision);
                let next_user_rev = view
                    .user_preferences
                    .confirmed
                    .as_ref()
                    .map(|prefs| prefs.revision);
                let user_prefs_changed = prev_user_rev != next_user_rev;
                let certificates_changed = this.state.refresh(view);
                (
                    project_changed,
                    normal_timeout_changed,
                    streaming_timeout_changed,
                    redirect_max_hops_changed,
                    network_http_changed,
                    scheduled_tasks_changed,
                    observed_certs_changed,
                    listeners_changed,
                    upstream_changed,
                    legacy_changed,
                    certificates_changed,
                    client_certs_changed,
                    scope_changed,
                    interception_changed,
                    sessions_changed,
                    user_prefs_changed,
                )
            };
            if this.state.active_proxy_address.is_some() {
                this.listeners_tls.listener_error = None;
            }
            if project_changed {
                this.focus.focus(window, cx);
                this.pools.close_dialog(cx);
                this.pools.selected = None;
                this.pools.confirming = false;
                this.interception_selected = [None, None];
                this.interception_restore = None;
                this.project_json_feedback = None;
                this.tls_negotiation_error = None;
                this.http_streaming_scope_selected = None;
                this.scheduled_tasks_selected = None;
                this.listeners_tls.selected_index = None;
                this.listeners_tls.listener_error = None;
                this.listeners_tls.client_cert_selected = None;
                this.listeners_tls.selected_server_cert = None;
                this.listeners_tls.inspected_server_cert = None;
                this.listeners_tls.server_cert_copied = None;
                if let Some(dialog) = this.listeners_tls.dialog.take() {
                    let _ = dialog.update(cx, |_, dialog_window, _| dialog_window.remove_window());
                }
                if let Some(dialog) = this.proxy_advanced.http_dialog.take() {
                    let _ = dialog.update(cx, |_, dialog_window, _| dialog_window.remove_window());
                }
                this.reset_inline_editors(cx);
            }
            if listeners_changed {
                if !project_changed
                    && let Some(idx) = this.listeners_tls.selected_index
                    && idx >= this.state.proxy_listeners.len()
                {
                    this.listeners_tls.selected_index =
                        this.state.proxy_listeners.len().checked_sub(1);
                }
                let sel = this.listeners_tls.selected_index;
                this.listeners_tls.table.update(cx, |table, cx| {
                    if let Some(row) = sel {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }
            if observed_certs_changed {
                let cert_len = this.state.observed_server_certs.len();
                if this
                    .listeners_tls
                    .selected_server_cert
                    .is_some_and(|idx| idx >= cert_len)
                {
                    this.listeners_tls.selected_server_cert = cert_len.checked_sub(1);
                }
                if this
                    .listeners_tls
                    .inspected_server_cert
                    .is_some_and(|idx| idx >= cert_len)
                {
                    this.listeners_tls.inspected_server_cert = None;
                }
                let cert_sel = this.listeners_tls.selected_server_cert;
                this.listeners_tls.server_cert_table.update(cx, |table, cx| {
                    if let Some(row) = cert_sel {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }
            if legacy_changed && !project_changed {
                this.tls_negotiation_error = None;
            }
            if user_prefs_changed {
                this.sync_user_pages_inputs(window, cx);
            }
            if sessions_changed || user_prefs_changed {
                this.sync_sessions_tables(project_changed, window, cx);
            }
            if interception_changed || user_prefs_changed {
                for response in [false, true] {
                    let index = usize::from(response);
                    let selected = this.interception_selected[index].and_then(|id| {
                        intercept_set(&this.state.proxy_interception, response)
                            .rules
                            .iter()
                            .position(|rule| rule.id == id)
                    });
                    if selected.is_none() {
                        this.interception_selected[index] = None;
                    }
                    this.interception_tables[index].update(cx, |table, cx| {
                        if let Some(row) = selected {
                            table.set_selected_row(row, cx);
                        } else {
                            table.clear_selection(cx);
                        }
                        table.refresh(cx);
                        cx.notify();
                    });
                }
            }
            if streaming_timeout_changed {
                let display_timeout = this
                    .state
                    .repeater_settings
                    .streaming_response_timeout_secs
                    .to_string();
                this.repeater_timeout_input.update(cx, |input, cx| {
                    if input.value().as_ref() != display_timeout.as_str() {
                        input.set_value(display_timeout, window, cx)
                    }
                });
            }
            if redirect_max_hops_changed {
                let display_hops = this
                    .state
                    .repeater_settings
                    .auto_follow_redirections_max_hops
                    .to_string();
                this.repeater_redirect_max_hops_input.update(cx, |input, cx| {
                    if !input.focus_handle(cx).is_focused(window)
                        && input.value().as_ref() != display_hops.as_str()
                    {
                        input.set_value(display_hops, window, cx)
                    }
                });
            }
            if normal_timeout_changed {
                let req_timeout_str = this.state.request_timeout_secs.to_string();
                this.connections.normal_timeout_input.update(cx, |input, cx| {
                    input.set_value(req_timeout_str.clone(), window, cx)
                });
                this.repeater_request_timeout_input.update(cx, |input, cx| {
                    if !input.focus_handle(cx).is_focused(window)
                        && input.value().as_ref() != req_timeout_str.as_str()
                    {
                        input.set_value(req_timeout_str, window, cx)
                    }
                });
            }
            if network_http_changed {
                let len = this.state.network_http.streaming_scope_rules.len();
                if this.http_streaming_scope_selected.is_some_and(|idx| idx >= len) {
                    this.http_streaming_scope_selected = len.checked_sub(1);
                }
                let sel = this.http_streaming_scope_selected;
                this.http_streaming_scope_table.update(cx, |table, cx| {
                    if let Some(row) = sel {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }
            if scheduled_tasks_changed {
                let len = this.state.scheduled_tasks.len();
                if this.scheduled_tasks_selected.is_some_and(|idx| idx >= len) {
                    this.scheduled_tasks_selected = len.checked_sub(1);
                }
                let sel = this.scheduled_tasks_selected;
                this.scheduled_tasks_table.update(cx, |table, cx| {
                    if let Some(row) = sel {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }
            if observed_certs_changed {
                let len = this.state.observed_server_certs.len();
                if this
                    .listeners_tls
                    .selected_server_cert
                    .is_some_and(|idx| idx >= len)
                {
                    this.listeners_tls.selected_server_cert = len.checked_sub(1);
                }
                if this
                    .listeners_tls
                    .inspected_server_cert
                    .is_some_and(|idx| idx >= len)
                {
                    this.listeners_tls.inspected_server_cert = None;
                }
                let sel = this.listeners_tls.selected_server_cert;
                this.listeners_tls.server_cert_table.update(cx, |table, cx| {
                    if let Some(row) = sel {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }
            if client_certs_changed {
                let len = this.state.client_certificates.len();
                let certs = this.state.client_certificates.clone();
                this.listeners_tls.sync_client_cert_summaries(&certs);
                this.listeners_tls.refresh_client_cert_table(len, cx);
            }
            if scope_changed {
                this.scope_ui.refresh_tables(&this.state.scope, cx);
            }
            if upstream_changed {
                this.restore_upstream_draft(&this.state.upstream_proxy.clone(), window, cx);
                this.sync_connections_ui(project_changed, window, cx);
            }
            if certificates_changed || user_prefs_changed {
                this.tls_names = certificate_names(&this.state.preferences);
                let selected = this.tls_selected.as_ref().and_then(|selected| {
                    this.state
                        .preferences
                        .confirmed
                        .as_ref()?
                        .custom_ca_ders
                        .iter()
                        .position(|certificate| certificate == selected)
                });
                if selected.is_none() {
                    this.tls_selected = None;
                }
                this.tls_table.update(cx, |table, cx| {
                    if let Some(row) = selected {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }
            if legacy_changed {
                this.tls_legacy_names =
                    tls_certificate_names(&this.state.upstream_tls.custom_ca_ders).join("、");
            }
            this.pools.refresh(cx);
            if this.page == SettingsPage::ProjectTasks {
                this.advance_pool_page(cx);
            }
            cx.notify();
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        window.set_window_title("设置");
        window.on_window_should_close(cx, |_, _| true);
        let pools = resource_pools::PoolPage::new(window, cx);
        let weak_owner = owner.downgrade();
        cx.on_release(move |settings, cx| {
            settings.pools.close_dialog(cx);
            if let Some(dialog) = settings.interception_dialog.take() {
                let _ = dialog.update(cx, |_, window, _| window.remove_window());
            }
            if let Some(dialog) = settings.proxy_advanced.http_dialog.take() {
                let _ = dialog.update(cx, |_, window, _| window.remove_window());
            }
            let _ = weak_owner.update(cx, |view, cx| {
                view.intruder.resource_pools.management.cancel_pending();
                if let Some(picker) = settings.tls_import {
                    view.finish_file_picker(picker, cx);
                }
                if let Some(picker) = settings.active_file_picker.take() {
                    view.finish_file_picker(picker, cx);
                }
            });
        })
        .detach();
        let weak_init = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = weak_init.update(cx, |this, cx| {
                let cert_len = this.state.client_certificates.len();
                let certs = this.state.client_certificates.clone();
                this.listeners_tls.sync_client_cert_summaries(&certs);
                this.listeners_tls.refresh_client_cert_table(cert_len, cx);
                this.scope_ui.refresh_tables(&this.state.scope, cx);
                if page == SettingsPage::ProjectTasks {
                    this.refresh_pool_page(cx);
                }
            });
        });
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索设置…"));
        let _ = search_input.focus_handle(cx).tab_stop(false);
        cx.subscribe_in(&search_input, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                this.sync_active_page_with_filter(cx);
            }
        })
        .detach();
        let initial_timeout = state
            .repeater_settings
            .streaming_response_timeout_secs
            .to_string();
        let repeater_timeout_input =
            cx.new(|cx| InputState::new(window, cx).default_value(initial_timeout));
        cx.subscribe_in(&repeater_timeout_input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let raw = this.repeater_timeout_input.read(cx).value().trim().to_owned();
                if let Ok(secs) = raw.parse::<u64>()
                    && secs <= 86_400
                {
                    let project = this.state.project;
                    let _ = this.owner.update(cx, |view, cx| {
                        view.update_repeater_settings(
                            project,
                            |s| s.streaming_response_timeout_secs = secs,
                            cx,
                        )
                    });
                } else if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.apply_repeater_timeout(window, cx);
                }
            }
        })
        .detach();
        let initial_max_hops = state
            .repeater_settings
            .auto_follow_redirections_max_hops
            .to_string();
        let repeater_redirect_max_hops_input =
            cx.new(|cx| InputState::new(window, cx).default_value(initial_max_hops));
        let _ = repeater_redirect_max_hops_input.focus_handle(cx).tab_stop(false);
        cx.subscribe_in(
            &repeater_redirect_max_hops_input,
            window,
            |this, _, event, window, cx| {
                if matches!(
                    event,
                    InputEvent::Change | InputEvent::PressEnter { .. } | InputEvent::Blur
                ) {
                    let raw = this
                        .repeater_redirect_max_hops_input
                        .read(cx)
                        .value()
                        .trim()
                        .to_owned();
                    if let Ok(hops) = raw.parse::<u8>()
                        && (1..=20).contains(&hops)
                    {
                        let project = this.state.project;
                        let _ = this.owner.update(cx, |view, cx| {
                            view.set_repeater_redirect_max_hops(project, hops, cx)
                        });
                    } else if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.apply_repeater_redirect_max_hops(window, cx);
                    }
                }
            },
        )
        .detach();
        let initial_request_timeout = state.repeater_settings.request_timeout_secs.to_string();
        let repeater_request_timeout_input =
            cx.new(|cx| InputState::new(window, cx).default_value(initial_request_timeout));
        let _ = repeater_request_timeout_input.focus_handle(cx).tab_stop(false);
        cx.subscribe_in(
            &repeater_request_timeout_input,
            window,
            |this, _, event, window, cx| {
                if matches!(
                    event,
                    InputEvent::Change | InputEvent::PressEnter { .. } | InputEvent::Blur
                ) {
                    let raw = this
                        .repeater_request_timeout_input
                        .read(cx)
                        .value()
                        .trim()
                        .to_owned();
                    if let Ok(secs) = raw.parse::<u64>()
                        && (1..=3600).contains(&secs)
                    {
                        let project = this.state.project;
                        let _ = this.owner.update(cx, |view, cx| {
                            view.set_repeater_request_timeout(project, secs, cx)
                        });
                    } else if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.apply_repeater_request_timeout(window, cx);
                    }
                }
            },
        )
        .detach();
        let mut listeners_tls = boxed_with(|| listeners_tls::ListenersTlsUi::new(window, cx));
        if !state.proxy_listeners.is_empty() {
            listeners_tls.selected_index = Some(0);
            listeners_tls
                .table
                .update(cx, |table, cx| table.set_selected_row(0, cx));
        }
        let streaming_owner = cx.weak_entity();
        let http_streaming_scope_table = cx.new(|tcx| {
            TableState::new(
                window_render::HttpStreamingScopeTable {
                    owner: streaming_owner,
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&http_streaming_scope_table, window, |this, _, event, _, cx| {
            if let TableEvent::SelectRow(row) = event {
                this.http_streaming_scope_selected = (*row
                    < this.state.network_http.streaming_scope_rules.len())
                .then_some(*row);
                cx.notify();
            }
        })
        .detach();
        let sched_owner = cx.weak_entity();
        let scheduled_tasks_table = cx.new(|tcx| {
            TableState::new(
                resource_pools::ScheduledTasksTable { owner: sched_owner },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&scheduled_tasks_table, window, |this, _, event, _, cx| {
            if let TableEvent::SelectRow(row) = event {
                this.scheduled_tasks_selected =
                    (*row < this.state.scheduled_tasks.len()).then_some(*row);
                cx.notify();
            }
        })
        .detach();
        let scheduled_task_action_select = cx.new(|scx| {
            gpui_kit::component::select::SelectState::new(
                vec!["暂停任务", "恢复任务", "运行扫描"],
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                scx,
            )
        });
        cx.subscribe_in(
            &scheduled_task_action_select,
            window,
            |_, _, _: &gpui_kit::component::select::SelectEvent<Vec<&'static str>>, _, cx| {
                cx.notify();
            },
        )
        .detach();
        let connections = boxed_with(|| connections::ConnectionsUi::new(&state, window, cx));
        let sessions = boxed_with(|| sessions::SessionsUi::new(window, cx));
        let upstream_host = cx.new(|cx| {
            InputState::new(window, cx).default_value(state.upstream_proxy.socks.host.clone())
        });
        let upstream_port = cx.new(|cx| {
            InputState::new(window, cx).default_value(state.upstream_proxy.socks.port.to_string())
        });
        let upstream_username = cx.new(|cx| {
            InputState::new(window, cx).default_value(state.upstream_proxy.socks.username.clone())
        });
        let upstream_password = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(state.upstream_proxy.socks.password.clone())
                .masked(true)
        });
        let tls_names = certificate_names(&state.preferences);
        let tls_legacy_names = tls_certificate_names(&state.upstream_tls.custom_ca_ders).join("、");
        let table_owner = cx.weak_entity();
        let tls_table = cx.new(|table_cx| {
            TableState::new(CertificateTable { owner: table_owner }, window, table_cx)
                .sortable(false)
                .col_selectable(false)
        });
        cx.subscribe_in(&tls_table, window, |this, _, event, _, cx| {
            if let TableEvent::SelectRow(row) = event {
                this.tls_selected = this
                    .state
                    .preferences
                    .confirmed
                    .as_ref()
                    .and_then(|preferences| preferences.custom_ca_ders.get(*row))
                    .cloned();
                cx.notify();
            }
        })
        .detach();
        let scope_ui = boxed_with(|| scope::ScopeUi::new(window, cx));
        let scope_include_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("例如 http://127.0.0.1:9001"));
        let scope_exclude_input = cx
            .new(|cx| InputState::new(window, cx).placeholder("例如 http://127.0.0.1:9001/logout"));
        let user_pages = boxed_with(|| user_pages::UserPagesUi::new(&state.preferences, window, cx));
        let proxy_advanced = boxed_with(|| proxy_advanced::ProxyAdvancedUi::new(&owner, window, cx));
        let interception_tables = std::array::from_fn(|index| {
            let owner = cx.weak_entity();
            let table = cx.new(|table_cx| {
                TableState::new(
                    InterceptionTable {
                        owner,
                        response: index == 1,
                    },
                    window,
                    table_cx,
                )
                .sortable(false)
                .col_selectable(false)
            });
            cx.subscribe_in(&table, window, move |this, _, event, window, cx| match event {
                TableEvent::SelectRow(row) => {
                    this.interception_selected[index] =
                        intercept_set(&this.state.proxy_interception, index == 1)
                            .rules
                            .get(*row)
                            .map(|rule| rule.id);
                    cx.notify();
                }
                TableEvent::DoubleClickedRow(row) => {
                    this.interception_selected[index] =
                        intercept_set(&this.state.proxy_interception, index == 1)
                            .rules
                            .get(*row)
                            .map(|rule| rule.id);
                    if this.interception_selected[index].is_some() {
                        this.open_interception_editor(index == 1, false, window, cx);
                    }
                    cx.notify();
                }
                _ => {}
            })
            .detach();
            table
        });
        Self {
            owner: owner.downgrade(),
            owner_window,
            state,
            focus,
            search_input,
            scope_filter: SettingsScopeFilter::All,
            collapsed_groups: [false; 6],
            nav_scroll: ScrollHandle::new(),
            content_scroll: ScrollHandle::new(),
            project_json_feedback: None,
            update_content_length_focus: cx.focus_handle(),
            checkbox_focus: cx.focus_handle(),
            normalize_http1_focus: cx.focus_handle(),
            repeater_timeout_input,
            repeater_redirect_max_hops_input,
            repeater_request_timeout_input,
            http_streaming_scope_table,
            http_streaming_scope_selected: None,
            http_streaming_scope_editing: None,
            http_streaming_scope_editor_open: false,
            http_streaming_scope_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("例如 https://example.com/events")),
            http_streaming_scope_subdomains: false,
            scheduled_tasks_table,
            scheduled_tasks_selected: None,
            scheduled_task_editing: None,
            scheduled_task_editor_open: false,
            scheduled_task_action_select,
            scheduled_task_time: cx.new(|cx| InputState::new(window, cx).placeholder("09:00")),
            scheduled_task_repeat: cx.new(|cx| InputState::new(window, cx).placeholder("每日")),
            scheduled_task_name: cx.new(|cx| InputState::new(window, cx).placeholder("描述（可选）")),
            scope_editor_open: [false, false],
            scope_editor_subdomains: [false, false],
            scope_oos_custom_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("例如 https://api.example.com")),
            scope_oos_custom_editor_open: false,
            scope_oos_custom_editing: None,
            scope_oos_custom_subdomains: false,
            listeners_tls,
            connections,
            upstream_host,
            upstream_port,
            upstream_username,
            upstream_password,
            tls_names,
            tls_legacy_names,
            tls_selected: None,
            tls_table,
            tls_import: None,
            tls_error: None,
            tls_negotiation_error: None,
            scope_ui,
            scope_include_input,
            scope_exclude_input,
            page,
            pools,
            sessions,
            interception_tables,
            interception_selected: [None, None],
            interception_focus: std::array::from_fn(|_| cx.focus_handle()),
            proxy_message_focus: std::array::from_fn(|_| cx.focus_handle()),
            proxy_ws_interception_focus: [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()],
            proxy_advanced,
            user_pages,
            interception_dialog: None,
            interception_opening: false,
            interception_restore: None,
            interception_error: None,
            _subscription: subscription,
            active_file_picker: None,
        }
    }

    pub(in crate::workbench) fn open_scheduled_task_prefill(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scheduled_task_editing = None;
        self.scheduled_task_editor_open = true;
        self.scheduled_task_action_select.update(cx, |select, cx| {
            select.set_selected_index(
                Some(gpui_kit::component::IndexPath::new(2)),
                window,
                cx,
            )
        });
        self.scheduled_task_time
            .update(cx, |input, cx| input.set_value("09:00", window, cx));
        self.scheduled_task_repeat
            .update(cx, |input, cx| input.set_value("每日", window, cx));
        self.scheduled_task_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.scheduled_task_time
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }
}
