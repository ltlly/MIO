use gpui_kit::accesskit::Role;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::{
    scroll::ScrollbarMode,
    table::{Column, TableDelegate},
};
use mio_core::Uuid;

use super::*;

pub(super) struct CertificateTable {
    pub(super) owner: WeakEntity<Settings>,
}

const CUSTOM_CA_COL_WIDTH: f32 = 822.;

impl TableDelegate for CertificateTable {
    fn columns_count(&self, _: &App) -> usize {
        1
    }

    fn column(&self, _: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new("name", "Name")
            .width((CUSTOM_CA_COL_WIDTH * scale).round())
            .p_0()
            .movable(false)
            .resizable(false)
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |owner| owner.read(cx).tls_names.len())
    }

    fn render_th(
        &mut self,
        _: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .px(px(6.))
            .text_color(theme::foreground())
            .text_size(theme::scaled_px(12.5))
            .truncate()
            .child("Name")
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("settings-tls-ca-row", row)).border_b_0()
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        listeners_tls::mio_table_td_text(self.cell_label(row, column, cx).unwrap_or_default())
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        listeners_tls::mio_table_empty()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().w_0().h_full()
    }

    fn cell_label(&self, row: usize, _: usize, cx: &App) -> Option<SharedString> {
        self.owner
            .upgrade()
            .and_then(|owner| owner.read(cx).tls_names.get(row).cloned())
            .map(Into::into)
    }
}

pub(super) struct ServerCertTable {
    pub(super) owner: WeakEntity<Settings>,
}

const SERVER_CERT_COLUMNS: [(&str, f32); 5] = [
    ("Host", 156.),
    ("Port", 60.),
    ("Subject", 184.),
    ("Issuer", 184.),
    ("Valid To", 128.),
];

impl TableDelegate for ServerCertTable {
    fn columns_count(&self, _: &App) -> usize {
        SERVER_CERT_COLUMNS.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let (label, width) = SERVER_CERT_COLUMNS[index];
        Column::new(index.to_string(), label)
            .width((width * theme::ui_scale()).round())
            .p_0()
            .movable(false)
            .resizable(index + 1 < SERVER_CERT_COLUMNS.len())
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |owner| owner.read(cx).state.observed_server_certs.len())
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (label, _) = SERVER_CERT_COLUMNS[col_ix];
        div()
            .px(px(6.))
            .text_color(theme::foreground())
            .text_size(theme::scaled_px(12.5))
            .truncate()
            .child(label)
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("settings-server-cert-row", row)).border_b_0()
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        listeners_tls::mio_table_td_text(self.cell_label(row, column, cx).unwrap_or_default())
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        listeners_tls::mio_table_empty()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().w_0().h_full()
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let settings = owner.read(cx);
        let entry = settings.state.observed_server_certs.get(row)?;
        Some(
            match column {
                0 => entry.host.clone(),
                1 => entry.port.to_string(),
                2 => entry.subject.clone(),
                3 => entry.issuer.clone(),
                _ => entry.valid_to.clone(),
            }
            .into(),
        )
    }
}

pub(super) fn certificate_names(preferences: &user_preferences::UserPreferencesUi) -> Vec<String> {
    preferences
        .confirmed
        .as_ref()
        .map_or_else(Vec::new, |preferences| {
            tls_certificate_names(&preferences.custom_ca_ders)
        })
}

pub(super) fn tls_certificate_names(certificates: &[Vec<u8>]) -> Vec<String> {
    certificates
        .iter()
        .map(|certificate| {
            mio_core::UpstreamTlsSettings::certificate_display_name(certificate)
                .unwrap_or_else(|error| format!("证书名称无法显示：{error}"))
        })
        .collect()
}

fn read_ca_certificate(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::{fs::OpenOptions, io::Read};

    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let file = opts
        .open(path)
        .map_err(|error| format!("无法读取 CA 证书：{error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("无法读取 CA 证书信息：{error}"))?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err("CA 证书必须是非空且不超过 64 KiB 的普通文件。".into());
    }
    let mut raw = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut raw)
        .map_err(|error| format!("无法完整读取 CA 证书：{error}"))?;
    let der = mio_core::UpstreamTlsSettings::parse_pem_or_der_cert(&raw)
        .map_err(|error| error.to_string())?;
    mio_core::UpstreamTlsSettings::certificate_display_name(&der)
        .map_err(|error| error.to_string())?;
    Ok(der)
}

impl Workbench {
    pub(in crate::workbench) fn set_upstream_tls(
        &mut self,
        project: Option<ProjectKey>,
        settings: mio_core::UpstreamTlsSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
            || !self.flush_workspace(window, cx)
        {
            return false;
        }
        let previous = std::mem::replace(&mut self.workspace.upstream_tls, settings);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.upstream_tls = previous;
            self.fail(error.to_string(), cx);
            return false;
        }
        if self.workspace.upstream_tls == previous {
            self.error = None;
            cx.notify();
            return true;
        }
        let saved = self.save_workspace(cx);
        cx.notify();
        saved
    }
}

impl Settings {
    fn add_user_tls_certificate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tls_import.is_some()
            || !self.state.preferences_available
            || !self.state.preferences.can_save()
        {
            return;
        }
        let Some(confirmed) = self.state.preferences.confirmed.as_ref() else {
            return;
        };
        if confirmed.custom_ca_ders.len() >= 32 {
            self.tls_error = Some("自定义 CA 最多支持 32 个证书；原列表未修改。".into());
            cx.notify();
            return;
        }
        let revision = confirmed.revision;
        let mut certificates = confirmed.custom_ca_ders.clone();
        let picker = Uuid::new_v4();
        let admitted = self
            .owner
            .update(cx, |view, cx| {
                if view.file_picker_pending()
                    || !view.backend_available()
                    || view.transition.is_some()
                {
                    return false;
                }
                view.file_picker = Some(picker);
                cx.notify();
                true
            })
            .unwrap_or(false);
        if !admitted {
            return;
        }
        self.tls_import = Some(picker);
        self.tls_error = None;
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择 CA 证书".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = match paths.await {
                Ok(Ok(Some(paths))) => {
                    let mut paths = paths.into_iter();
                    if let Some(path) = paths.next().filter(|_| paths.next().is_none()) {
                        let parsed = cx
                            .background_executor()
                            .spawn(async move { read_ca_certificate(&path) });
                        parsed.await.map(Some)
                    } else {
                        Err("证书选择器未返回单个文件；原列表未修改。".into())
                    }
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("无法选择 CA 证书：{error}")),
                Err(_) => Err("CA 证书选择器已关闭；原列表未修改。".into()),
            };
            let _ = this.update_in(cx, |this, _, cx| {
                if this.tls_import != Some(picker) {
                    return;
                }
                this.tls_import = None;
                let current = this
                    .owner
                    .update(cx, |view, cx| {
                        view.finish_file_picker(picker, cx) && view.backend_available()
                    })
                    .unwrap_or(false);
                if !current {
                    return;
                }
                match result {
                    Ok(Some(certificate)) => {
                        if certificates.contains(&certificate) {
                            this.tls_error = Some("该 CA 证书已在列表中；未重复添加。".into());
                        } else {
                            certificates.push(certificate);
                            let saved = this.owner.update(cx, |view, cx| {
                                view.set_user_tls_certificates(certificates, revision, cx)
                            });
                            this.tls_error = match saved {
                                Ok(Ok(_)) => None,
                                Ok(Err(error)) => Some(error),
                                Err(_) => Some("工作台已关闭；CA 证书列表未修改。".into()),
                            };
                        }
                    }
                    Ok(None) => {}
                    Err(error) => this.tls_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_user_tls_certificate(&mut self, cx: &mut Context<Self>) {
        if self.tls_import.is_some()
            || !self.state.preferences_available
            || !self.state.preferences.can_save()
        {
            return;
        }
        let Some(selected) = &self.tls_selected else {
            return;
        };
        let Some(confirmed) = self.state.preferences.confirmed.as_ref() else {
            return;
        };
        let revision = confirmed.revision;
        let mut certificates = confirmed.custom_ca_ders.clone();
        let Some(index) = certificates
            .iter()
            .position(|certificate| certificate == selected)
        else {
            return;
        };
        certificates.remove(index);
        let saved = self.owner.update(cx, |view, cx| {
            view.set_user_tls_certificates(certificates, revision, cx)
        });
        self.tls_error = match saved {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some(error),
            Err(_) => Some("工作台已关闭；CA 证书列表未修改。".into()),
        };
        cx.notify();
    }

    pub(super) fn update_upstream_tls(
        &self,
        mutate: impl FnOnce(&mut mio_core::UpstreamTlsSettings) + 'static,
        cx: &mut Context<Self>,
    ) {
        let project = self.state.project;
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        cx.defer(move |cx| {
            let _ = owner_window.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    let mut updated = view.workspace.upstream_tls.clone();
                    mutate(&mut updated);
                    view.set_upstream_tls(project, updated, window, cx)
                });
            });
        });
    }

    pub(in crate::workbench) fn render_tls(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.state.project;
        let project_enabled = self.state.enabled && self.tls_import.is_none();
        let verify_upstream = !self.state.upstream_tls.insecure_skip_verify;
        let proto_mode = self.state.upstream_tls.protocol_mode;
        let disable_session_resume = self.state.upstream_tls.disable_session_resume;
        let session_resume_owner = cx.entity();

        // Section 1: TLS negotiation (TOP — matches MIO Network > TLS)
        let negotiation_section = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(listeners_tls::mio_section_heading(
                "settings-tls-negotiation-heading",
                "TLS 协商",
                listeners_tls::mio_project_pill(),
            ))
            .child(listeners_tls::mio_desc_row(
                "使用这些设置控制与上游服务器进行 TLS 协商时使用的 TLS 协议和密码套件。请谨慎修改，错误配置可能导致出站 TLS 连接中断。",
            ))
            .child(
                div()
                    .pl(px(30.))
                    .pt(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_color(theme::foreground())
                            .child("TLS 信任"),
                    )
                    .child({
                        let verify_owner = cx.entity();
                        listeners_tls::indexed_checkbox(
                            "settings-tls-verify-upstream",
                            "验证上游 TLS",
                            verify_upstream,
                            project_enabled,
                        )
                        .on_change(move |state, _, _, cx| {
                            let insecure = state != CheckboxState::Checked;
                            verify_owner.update(cx, |this, cx| {
                                this.update_upstream_tls(
                                    move |s| s.insecure_skip_verify = insecure,
                                    cx,
                                );
                            });
                        })
                    })
                    .child(
                        div()
                            .mt(px(8.))
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child({
                                let proto_owner = cx.entity();
                                settings_radio(
                                    "settings-tls-proto-all",
                                    "使用运行环境支持的全部协议与密码套件",
                                    proto_mode == mio_core::TlsProtocolMode::AllSupported,
                                    project_enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |_, _, _, cx| {
                                    proto_owner.update(cx, |this, cx| {
                                        this.update_upstream_tls(
                                            |s| {
                                                s.protocol_mode =
                                                    mio_core::TlsProtocolMode::AllSupported
                                            },
                                            cx,
                                        );
                                    });
                                })
                            })
                            .child({
                                let proto_owner = cx.entity();
                                settings_radio(
                                    "settings-tls-proto-default",
                                    "使用运行环境的默认协议与密码套件",
                                    proto_mode == mio_core::TlsProtocolMode::Default,
                                    project_enabled,
                                    window,
                                    cx,
                                )
                                .on_change(move |_, _, _, cx| {
                                    proto_owner.update(cx, |this, cx| {
                                        this.update_upstream_tls(
                                            |s| s.protocol_mode = mio_core::TlsProtocolMode::Default,
                                            cx,
                                        );
                                    });
                                })
                            }),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .text_color(theme::foreground())
                            .child("TLS 协商变通方案"),
                    )
                    .child(
                        listeners_tls::indexed_checkbox(
                            "settings-tls-disable-session-resume",
                            "禁用 TLS 会话恢复",
                            disable_session_resume,
                            project_enabled,
                        )
                        .on_change(move |state, _, _, cx| {
                            let checked = state == CheckboxState::Checked;
                            session_resume_owner.update(cx, |this, cx| {
                                this.update_upstream_tls(
                                    move |s| s.disable_session_resume = checked,
                                    cx,
                                );
                            });
                        }),
                    ),
            );

        // Section 2: Client TLS certificates
        let client_tls_section = self.render_client_tls_card(window, cx);

        // Section 3: Server TLS certificates
        let server_certs_empty = self.state.observed_server_certs.is_empty();
        let has_selected_server_cert = self
            .listeners_tls
            .selected_server_cert
            .is_some_and(|idx| idx < self.state.observed_server_certs.len());
        let mut server_tls_section = div()
            .mt(px(6.))
            .pt(px(12.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(listeners_tls::mio_section_heading(
                "settings-tls-server-certs-heading",
                "服务端 TLS 证书",
                listeners_tls::mio_project_pill(),
            ))
            .child(listeners_tls::mio_desc_row_with_reset(
                "settings-tls-server-certs-reset",
                "重置服务端 TLS 证书列表",
                !project_enabled || server_certs_empty,
                cx.listener(|this, _, _, cx| {
                    let _ = this.owner.update(cx, |view, cx| {
                        view.observed_server_certs.clear();
                        cx.notify();
                    });
                    this.state.observed_server_certs.clear();
                    this.listeners_tls.selected_server_cert = None;
                    this.listeners_tls.inspected_server_cert = None;
                    this.listeners_tls.server_cert_copied = None;
                    this.listeners_tls
                        .server_cert_table
                        .update(cx, |table, cx| {
                            table.clear_selection(cx);
                            table.refresh(cx);
                        });
                    cx.notify();
                }),
                "此面板显示从 Web 服务器收到的唯一 TLS 证书列表。单击或双击条目可查看证书的完整详情。",
            ))
            .child(
                div().pl(px(30.)).child({
                    let mut server_table_el = DataTable::new(&self.listeners_tls.server_cert_table)
                        .with_size(theme::scaled_px(23.).max(px(20.)))
                        .header_row_height(listeners_tls::mio_table_header_height())
                        .bordered(false)
                        .stripe(false)
                        .scrollbar_visible(!server_certs_empty, false)
                        .scroll_bounce(false)
                        .accessibility_label("Server TLS certificates");
                    if !server_certs_empty {
                        server_table_el = server_table_el.scrollbar_mode(ScrollbarMode::Always);
                    }
                    let dark = theme::is_dark();
                    let track_bg = rgb(if dark { 0x25272b } else { 0xf5f5f5 });
                    let track_border = rgb(if dark { 0x3b3e44 } else { 0xe0e0e0 });
                    let thumb_bg = rgb(if dark { 0x5c626b } else { 0xcbcbcb });
                    div()
                        .w(gpui_kit::relative(0.78))
                        .h(px(132.))
                        .border_1()
                        .border_color(listeners_tls::mio_table_border())
                        .bg(theme::surface())
                        .overflow_hidden()
                        .flex()
                        .child(div().flex_1().min_w_0().h_full().child(server_table_el))
                        .when(server_certs_empty, |this| {
                            this.child(
                                div()
                                    .w(px(11.))
                                    .h_full()
                                    .flex_shrink_0()
                                    .border_l_1()
                                    .border_color(track_border)
                                    .bg(track_bg)
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .pt(px(2.))
                                    .child(
                                        div()
                                            .w(px(6.))
                                            .h(px(22.))
                                            .rounded(px(3.))
                                            .bg(thumb_bg),
                                    ),
                            )
                        })
                }),
            )
            .child(
                div().pl(px(30.)).flex().items_center().gap(px(8.)).child(
                    listeners_tls::mio_action_button(
                        "settings-tls-server-cert-inspect",
                        "查看详情",
                        !has_selected_server_cert,
                    )
                    .tooltip("查看当前选中的服务端 TLS 证书详情与 PEM 内容")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(idx) = this.listeners_tls.selected_server_cert
                            && idx < this.state.observed_server_certs.len()
                        {
                            this.listeners_tls.inspected_server_cert = Some(idx);
                            this.listeners_tls.server_cert_copied = None;
                            cx.notify();
                        }
                    })),
                ),
            );
        if let Some(idx) = self.listeners_tls.inspected_server_cert
            && let Some(entry) = self.state.observed_server_certs.get(idx)
        {
            let pem_to_copy = entry.pem.clone();
            let copied = self.listeners_tls.server_cert_copied == Some(idx);
            let sans_text = if entry.san.is_empty() {
                "—".to_owned()
            } else {
                entry.san.join(", ")
            };
            let detail_row = |label: &'static str, value: String, mono: bool| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .text_size(theme::scaled_px(12.))
                    .child(
                        div()
                            .w(px(136.))
                            .flex_shrink_0()
                            .text_color(theme::muted_foreground())
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme::foreground())
                            .when(mono, |this| this.font_family(theme::mono_font_family()))
                            .child(value),
                    )
            };
            server_tls_section = server_tls_section.child(
                div()
                    .id("settings-tls-server-cert-detail")
                    .test_support()
                    .ml(px(30.))
                    .w(gpui_kit::relative(0.78))
                    .p(px(10.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::background())
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .pb(px(4.))
                            .border_b_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_color(theme::foreground())
                                    .child(format!("{}:{}", entry.host, entry.port)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .when(copied, |this| {
                                        this.child(
                                            div()
                                                .id("settings-tls-server-cert-copied")
                                                .test_support()
                                                .role(Role::Status)
                                                .text_size(theme::scaled_px(11.5))
                                                .text_color(theme::muted_foreground())
                                                .child("已复制 PEM"),
                                        )
                                    })
                                    .child(
                                        listeners_tls::mio_action_button(
                                            "settings-tls-server-cert-copy-pem",
                                            "复制 PEM",
                                            pem_to_copy.is_empty(),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.write_to_clipboard(
                                                gpui_kit::ClipboardItem::new_string(
                                                    pem_to_copy.clone(),
                                                ),
                                            );
                                            this.listeners_tls.server_cert_copied = Some(idx);
                                            cx.notify();
                                        })),
                                    )
                                    .child(
                                        listeners_tls::mio_action_button(
                                            "settings-tls-server-cert-close",
                                            "关闭",
                                            false,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.listeners_tls.inspected_server_cert = None;
                                            this.listeners_tls.server_cert_copied = None;
                                            cx.notify();
                                        })),
                                    ),
                            ),
                    )
                    .child(detail_row("Subject", entry.subject.clone(), false))
                    .child(detail_row("Issuer", entry.issuer.clone(), false))
                    .child(detail_row("Valid From", entry.valid_from.clone(), false))
                    .child(detail_row("Valid To", entry.valid_to.clone(), false))
                    .child(detail_row("SANs", sans_text, true))
                    .child(detail_row(
                        "SHA-256 Fingerprint",
                        entry.sha256_fingerprint.clone(),
                        true,
                    )),
            );
        }

        // Section 4: Custom CA certificates (accessible below the External default viewport)
        let preferences = &self.state.preferences;
        let ca_enabled =
            self.state.preferences_available && preferences.can_save() && self.tls_import.is_none();
        let removable = self.tls_selected.as_ref().is_some_and(|selected| {
            preferences
                .confirmed
                .as_ref()
                .is_some_and(|preferences| preferences.custom_ca_ders.contains(selected))
        });
        let mut custom_ca_section = div()
            .id("settings-tls-custom-ca-section")
            .test_support()
            .mt(px(8.))
            .pt(px(14.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(listeners_tls::mio_section_heading(
                "settings-tls-custom-ca-heading",
                "Custom CA certificates",
                listeners_tls::mio_user_pill_with_id("settings-tls-user-scope"),
            ))
            .child(listeners_tls::mio_desc_row(
                "配置连接上游服务器时信任的自定义 CA 证书。证书作为用户设置跨项目保存，不修改系统信任。",
            ))
            .child(
                div()
                    .pl(px(30.))
                    .flex()
                    .gap(px(9.))
                    .h(px(132.))
                    .flex_shrink_0()
                    .child(
                        div()
                            .w(theme::scaled_px(88.).max(px(84.)))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(7.))
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-tls-ca-add",
                                    "添加",
                                    !ca_enabled,
                                )
                                .w_full()
                                .tooltip("从本地选择并导入 PEM 或 DER 格式的自定义 CA 根证书")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_user_tls_certificate(window, cx)
                                })),
                            )
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-tls-ca-remove",
                                    "移除",
                                    !ca_enabled || !removable,
                                )
                                .w_full()
                                .tooltip("从用户信任列表中移除当前选中的自定义 CA 根证书")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remove_user_tls_certificate(cx)
                                })),
                            ),
                    )
                    .child(listeners_tls::mio_table_frame(
                        &self.tls_table,
                        "Custom CA certificates",
                    )),
            )
            .child(
                div()
                    .id("settings-tls-activation-note")
                    .test_support()
                    .role(Role::Status)
                    .aria_label("新发起的 Repeater／Intruder 请求使用已确认的用户证书；当前 Proxy 监听器需停止后重新启动才生效。")
                    .pl(px(30.))
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child("新发起的 Repeater／Intruder 请求使用已确认的用户证书；当前 Proxy 监听器需停止后重新启动才生效。"),
            );
        if self.state.upstream_tls.has_custom_root_override() {
            let legacy = &self.state.upstream_tls;
            let message = format!(
                "旧项目 TLS 覆盖仍生效：{} 个自定义根证书（{}），{}系统根证书。用户证书不会覆盖旧项目设置。",
                legacy.custom_ca_ders.len(),
                self.tls_legacy_names,
                if legacy.include_system_roots {
                    "同时信任"
                } else {
                    "不信任"
                },
            );
            custom_ca_section = custom_ca_section
                .child(
                    div()
                        .id("settings-tls-legacy-override")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(message.clone())
                        .ml(px(30.))
                        .p(px(8.))
                        .rounded(px(4.))
                        .bg(rgb(if theme::is_dark() { 0x45391e } else { 0xfff4d9 }))
                        .child(message),
                )
                .child(
                    div().pl(px(30.)).child(
                        listeners_tls::mio_action_button(
                            "settings-tls-use-user-settings",
                            "使用用户设置",
                            !project_enabled,
                        )
                        .tooltip(
                            "明确移除当前项目专有 TLS 根设置并使用用户设置，不迁移或删除证书文件。",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let owner = this.owner.clone();
                            let owner_window = this.owner_window;
                            cx.defer(move |cx| {
                                let _ = owner_window.update(cx, |_, window, cx| {
                                    let _ = owner.update(cx, |view, cx| {
                                        let mut reset = view.workspace.upstream_tls.clone();
                                        reset.custom_ca_ders.clear();
                                        reset.include_system_roots = true;
                                        view.set_upstream_tls(project, reset, window, cx)
                                    });
                                });
                            });
                        })),
                    ),
                );
        }
        if self.tls_import.is_some() {
            custom_ca_section = custom_ca_section.child(
                div()
                    .id("settings-tls-import-status")
                    .test_support()
                    .role(Role::Status)
                    .aria_label("正在选择或读取 CA 证书，原列表仍保留。")
                    .pl(px(30.))
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child("正在选择或读取 CA 证书，原列表仍保留。"),
            );
        }
        for (id, error) in [
            ("settings-tls-error", &self.tls_error),
            ("settings-tls-load-error", &preferences.load_error),
            ("settings-tls-save-error", &preferences.error),
        ] {
            if let Some(error) = error {
                custom_ca_section = custom_ca_section.child(
                    div()
                        .id(id)
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .ml(px(30.))
                        .p(px(8.))
                        .rounded(px(4.))
                        .bg(theme::danger_bg())
                        .text_color(theme::danger_fg())
                        .child(error.clone()),
                );
            }
        }
        if preferences.load_error.is_some() || preferences.error.is_some() {
            custom_ca_section = custom_ca_section.child(
                div().pl(px(30.)).child(
                    listeners_tls::mio_action_button(
                        "settings-tls-reload",
                        "重新读取",
                        !self.state.preferences_available
                            || preferences.busy()
                            || self.tls_import.is_some(),
                    )
                    .tooltip("重新从后台读取用户级自定义 CA 证书列表")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let _ = this
                            .owner
                            .update(cx, |view, cx| view.query_user_preferences(cx));
                    })),
                ),
            );
        }

        let content = div()
            .px(px(22.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(negotiation_section)
            .child(client_tls_section)
            .child(server_tls_section)
            .child(custom_ca_section);
        settings_scrollable_pane("settings-tls-page", &self.content_scroll, content)
            .into_any_element()
    }
}
