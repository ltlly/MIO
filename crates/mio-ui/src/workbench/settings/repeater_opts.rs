use gpui_kit::accesskit::Role;
use mio_core::RepeaterRedirectMode;

use super::*;

impl Workbench {
    pub(in crate::workbench) fn set_repeater_strip_http2_connection_headers(
        &mut self,
        project: Option<ProjectKey>,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
            || self
                .workspace
                .repeater_settings
                .strip_connection_headers_over_http2
                == checked
        {
            return;
        }
        self.workspace
            .repeater_settings
            .strip_connection_headers_over_http2 = checked;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_repeater_enforce_http1_connection_close(
        &mut self,
        project: Option<ProjectKey>,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
            || self
                .workspace
                .repeater_settings
                .enforce_http1_connection_close
                == checked
        {
            return;
        }
        self.workspace
            .repeater_settings
            .enforce_http1_connection_close = checked;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_repeater_redirect_max_hops(
        &mut self,
        project: Option<ProjectKey>,
        max_hops: u8,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        if !(1..=20).contains(&max_hops) {
            self.fail("Repeater 最大重定向跳数必须在 1 到 20 之间。", cx);
            return;
        }
        if self
            .workspace
            .repeater_settings
            .auto_follow_redirections_max_hops
            == max_hops
        {
            return;
        }
        self.workspace
            .repeater_settings
            .auto_follow_redirections_max_hops = max_hops;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_repeater_update_content_length(
        &mut self,
        project: Option<ProjectKey>,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
            || self.workspace.repeater_settings.update_content_length == checked
        {
            return;
        }
        self.workspace.repeater_settings.update_content_length = checked;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_repeater_unpack(
        &mut self,
        project: Option<ProjectKey>,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
            || self.workspace.repeater_settings.unpack_compressed_responses == checked
        {
            return;
        }
        self.workspace.repeater_settings.unpack_compressed_responses = checked;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_repeater_normalize_http1_line_endings(
        &mut self,
        project: Option<ProjectKey>,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
            || self
                .workspace
                .repeater_settings
                .normalize_http1_line_endings
                == checked
        {
            return;
        }
        self.workspace
            .repeater_settings
            .normalize_http1_line_endings = checked;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_repeater_follow_redirects(
        &mut self,
        project: Option<ProjectKey>,
        mode: RepeaterRedirectMode,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
            || self.workspace.repeater_settings.follow_redirects == mode
        {
            return;
        }
        self.workspace.repeater_settings.follow_redirects = mode;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn reset_repeater_redirect_settings(
        &mut self,
        project: Option<ProjectKey>,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        let rs = &mut self.workspace.repeater_settings;
        let changed = rs.follow_redirects != RepeaterRedirectMode::Never
            || rs.auto_follow_redirections_max_hops != 10
            || rs.process_cookies_in_redirects
            || rs.cross_domain_protocol_redirects;
        if !changed {
            return;
        }
        rs.follow_redirects = RepeaterRedirectMode::Never;
        rs.auto_follow_redirections_max_hops = 10;
        rs.process_cookies_in_redirects = false;
        rs.cross_domain_protocol_redirects = false;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_repeater_request_timeout(
        &mut self,
        project: Option<ProjectKey>,
        timeout_secs: u64,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        if !(1..=3600).contains(&timeout_secs) {
            self.fail("Repeater 请求超时必须在 1 到 3600 秒之间。", cx);
            return;
        }
        if self.workspace.repeater_settings.request_timeout_secs == timeout_secs {
            return;
        }
        self.workspace.repeater_settings.request_timeout_secs = timeout_secs;
        self.save_workspace(cx);
        cx.notify();
    }
}

impl Settings {
    pub(in crate::workbench) fn apply_repeater_timeout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = match editing::input_snapshot(&self.repeater_timeout_input, window, cx) {
            Ok(value) => value,
            Err(error) => {
                self.settings_error(error, cx);
                return;
            }
        };
        let Ok(secs) = value.trim().parse::<u64>() else {
            self.settings_error("Repeater 流式响应超时必须在 0 到 86400 秒之间。", cx);
            return;
        };
        if secs > 86_400 {
            self.settings_error("Repeater 流式响应超时必须在 0 到 86400 秒之间。", cx);
            return;
        }
        let project = self.state.project;
        let _ = self.owner.update(cx, |view, cx| {
            view.update_repeater_settings(
                project,
                |s| s.streaming_response_timeout_secs = secs,
                cx,
            )
        });
    }

    pub(in crate::workbench) fn apply_repeater_redirect_max_hops(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value =
            match editing::input_snapshot(&self.repeater_redirect_max_hops_input, window, cx) {
                Ok(value) => value,
                Err(error) => {
                    self.settings_error(error, cx);
                    return;
                }
            };
        let Ok(hops) = value.trim().parse::<u8>() else {
            self.settings_error("Repeater 最大重定向跳数必须在 1 到 20 之间。", cx);
            return;
        };
        if !(1..=20).contains(&hops) {
            self.settings_error("Repeater 最大重定向跳数必须在 1 到 20 之间。", cx);
            return;
        }
        let project = self.state.project;
        let _ = self
            .owner
            .update(cx, |view, cx| view.set_repeater_redirect_max_hops(project, hops, cx));
    }

    pub(in crate::workbench) fn apply_repeater_request_timeout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value =
            match editing::input_snapshot(&self.repeater_request_timeout_input, window, cx) {
                Ok(value) => value,
                Err(error) => {
                    self.settings_error(error, cx);
                    return;
                }
            };
        let Ok(secs) = value.trim().parse::<u64>() else {
            self.settings_error("Repeater 请求超时必须在 1 到 3600 秒之间。", cx);
            return;
        };
        if !(1..=3600).contains(&secs) {
            self.settings_error("Repeater 请求超时必须在 1 到 3600 秒之间。", cx);
            return;
        }
        let project = self.state.project;
        let _ = self
            .owner
            .update(cx, |view, cx| view.set_repeater_request_timeout(project, secs, cx));
    }

    pub(super) fn render_timeout(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.state.project;
        let enabled = self.state.enabled;
        div()
            .pt(px(10.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(settings_section_heading(
                "settings-repeater-streaming-heading",
                "流式响应",
                project_scope_badge(),
            ))
            .child(settings_desc_row_with_action(
                settings_gear_icon_button(
                    "settings-repeater-timeout-reset",
                    "将 Repeater 流式响应超时重置为默认值（600 秒）",
                    !enabled,
                )
                .tab_stop(false)
                .on_click(cx.listener(move |this, _, window, cx| {
                    let _ = this.owner.update(cx, |view, cx| {
                        view.update_repeater_settings(
                            project,
                            |s| s.streaming_response_timeout_secs = 600,
                            cx,
                        )
                    });
                    this.repeater_timeout_input
                        .update(cx, |input, cx| input.set_value("600", window, cx));
                })),
                "控制 Repeater 保持流式响应连接打开的时长。要关闭超时限制，请将其设为 0。",
            ))
            .child(
                div()
                    .pl(px(31.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child("流式响应超时（秒）：")
                    .child(
                        div()
                            .id("settings-repeater-timeout-input")
                            .test_support()
                            .w(theme::scaled_px(86.).max(px(86.)))
                            .child(
                                Input::new(&self.repeater_timeout_input)
                                    .small()
                                    .disabled(!enabled),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn append_repeater_feedback(
        &self,
        container: gpui_kit::Stateful<gpui_kit::Div>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        container
            .when_some(self.state.status, |this, status| {
                this.child(
                    div()
                        .id("repeater-settings-status")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(status)
                        .child(status),
                )
            })
            .when_some(self.state.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("repeater-settings-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .p(px(8.))
                        .bg(theme::danger_bg())
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            })
    }
}
