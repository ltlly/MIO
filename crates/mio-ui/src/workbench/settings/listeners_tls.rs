use gpui_kit::accesskit::Role;
use gpui_kit::base::{Checkbox, CheckboxIndicator};
use gpui_kit::component::{
    Icon, IconName, IndexPath,
    button::ButtonVariants,
    select::{SelectEvent, SelectState},
    table::TableDelegate,
};
use gpui_kit::{ClickEvent, FontWeight, Hsla, Pixels};
use mio_core::{ClientCertificateFormat, ClientCertificateRule, ClientCertificateSummary};

use super::*;

mod client_certs;
mod listeners;

const MIO_TABLE_TRIANGLE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 7 10"><polygon points="0,0 7,5 0,10" fill="currentColor"/></svg>"#;

pub(super) struct ListenersTlsUi {
    pub(super) table: Entity<TableState<listeners::ProxyListenersTable>>,
    pub(super) selected_index: Option<usize>,
    pub(super) dialog: Option<AnyWindowHandle>,
    pub(super) dialog_opening: bool,
    pub(super) listener_error: Option<String>,
    pub(super) client_cert_editor_visible: bool,
    pub(super) client_cert_editing: Option<usize>,
    pub(super) client_cert_saving: bool,
    pub(super) client_cert_format: ClientCertificateFormat,
    client_cert_table: Entity<TableState<client_certs::ClientCertTable>>,
    pub(super) client_cert_selected: Option<usize>,
    pub(super) client_cert_summary_cache: Vec<(ClientCertificateRule, ClientCertificateSummary)>,
    pub(super) client_cert_format_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) client_cert_host: Entity<InputState>,
    pub(super) client_cert_path: Entity<InputState>,
    pub(super) client_cert_key_path: Entity<InputState>,
    pub(super) client_cert_password: Entity<InputState>,
    pub(super) client_cert_error: Option<String>,
    pub(super) server_cert_table: Entity<TableState<tls_certs::ServerCertTable>>,
    pub(super) selected_server_cert: Option<usize>,
    pub(super) inspected_server_cert: Option<usize>,
    pub(super) server_cert_copied: Option<usize>,
}

impl ListenersTlsUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Settings>) -> Self {
        let weak = cx.weak_entity();
        let table = cx.new(|tcx| {
            TableState::new(
                listeners::ProxyListenersTable {
                    owner: weak.clone(),
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&table, window, |this, _, event, window, cx| match event {
            TableEvent::SelectRow(row) => {
                if *row < this.state.proxy_listeners.len() {
                    this.listeners_tls.selected_index = Some(*row);
                } else {
                    this.listeners_tls.selected_index = None;
                }
                cx.notify();
            }
            TableEvent::DoubleClickedRow(row) => {
                if *row < this.state.proxy_listeners.len() {
                    this.listeners_tls.selected_index = Some(*row);
                    this.open_proxy_listener_editor(false, window, cx);
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        let server_cert_table = cx.new(|tcx| {
            TableState::new(
                tls_certs::ServerCertTable {
                    owner: weak.clone(),
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&server_cert_table, window, |this, _, event, _, cx| match event {
            TableEvent::SelectRow(row) => {
                if *row < this.state.observed_server_certs.len() {
                    this.listeners_tls.selected_server_cert = Some(*row);
                    this.listeners_tls.inspected_server_cert = Some(*row);
                } else {
                    this.listeners_tls.selected_server_cert = None;
                    this.listeners_tls.inspected_server_cert = None;
                }
                this.listeners_tls.server_cert_copied = None;
                cx.notify();
            }
            TableEvent::DoubleClickedRow(row) => {
                if *row < this.state.observed_server_certs.len() {
                    this.listeners_tls.selected_server_cert = Some(*row);
                    this.listeners_tls.inspected_server_cert = Some(*row);
                    this.listeners_tls.server_cert_copied = None;
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        let client_cert_table = cx.new(|tcx| {
            TableState::new(
                client_certs::ClientCertTable { owner: weak },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&client_cert_table, window, |this, _, event, window, cx| match event {
            TableEvent::SelectRow(row) => {
                this.listeners_tls.client_cert_selected =
                    (*row < this.state.client_certificates.len()).then_some(*row);
                cx.notify();
            }
            TableEvent::DoubleClickedRow(row) => {
                if *row < this.state.client_certificates.len() {
                    this.listeners_tls.client_cert_selected = Some(*row);
                    this.open_client_cert_editor(Some(*row), window, cx);
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        let client_cert_format_select = cx.new(|scx| {
            SelectState::new(vec!["PEM", "PKCS#12"], Some(IndexPath::new(0)), window, scx)
        });
        cx.subscribe_in(&client_cert_format_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(Some(fmt_str)) = event else {
                return;
            };
            this.listeners_tls.client_cert_format = if *fmt_str == "PKCS#12" {
                ClientCertificateFormat::Pkcs12
            } else {
                ClientCertificateFormat::Pem
            };
            cx.notify();
        })
        .detach();
        let init_weak = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = init_weak.update(cx, |this, cx| {
                if let Some(owner) = this.owner.upgrade() {
                    cx.observe(&owner, |this, owner, cx| {
                        let view = owner.read(cx);
                        let project_changed = this.state.project != view.project;
                        this.state.proxy_listeners = view.workspace.effective_proxy_listeners();
                        this.state.active_proxy_address = view.proxy_address;
                        this.state.active_proxy_listeners = view.proxy_listeners_bound.clone();
                        this.state.proxy_busy = view.proxy_busy;
                        this.state
                            .last_ca_fingerprint
                            .clone_from(&view.last_ca_fingerprint);
                        this.state.observed_server_certs = view.observed_server_certs.clone();
                        if view.proxy_address.is_some() {
                            this.listeners_tls.listener_error = None;
                        }
                        if project_changed {
                            this.listeners_tls.selected_index = None;
                            this.listeners_tls.listener_error = None;
                            this.listeners_tls.client_cert_editor_visible = false;
                            this.listeners_tls.client_cert_editing = None;
                            this.listeners_tls.client_cert_saving = false;
                            this.listeners_tls.client_cert_selected = None;
                            this.listeners_tls.selected_server_cert = None;
                            this.listeners_tls.inspected_server_cert = None;
                            this.listeners_tls.server_cert_copied = None;
                            if let Some(dialog) = this.listeners_tls.dialog.take() {
                                let _ = dialog.update(cx, |_, window, _| window.remove_window());
                            }
                        } else if let Some(idx) = this.listeners_tls.selected_index
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
                        cx.notify();
                    })
                    .detach();
                }
            });
        });
        cx.on_release(|settings, cx| {
            if let Some(dialog) = settings.listeners_tls.dialog.take() {
                let _ = dialog.update(cx, |_, window, _| window.remove_window());
            }
        })
        .detach();
        Self {
            table,
            selected_index: None,
            dialog: None,
            dialog_opening: false,
            listener_error: None,
            client_cert_editor_visible: false,
            client_cert_editing: None,
            client_cert_saving: false,
            client_cert_format: ClientCertificateFormat::Pem,
            client_cert_table,
            client_cert_selected: None,
            client_cert_summary_cache: Vec::new(),
            client_cert_format_select,
            client_cert_host: cx.new(|cx| {
                InputState::new(window, cx).placeholder("api.example.com 或 *.example.com")
            }),
            client_cert_path: cx.new(|cx| {
                InputState::new(window, cx).placeholder("证书文件路径（PEM 或 PKCS#12）")
            }),
            client_cert_key_path: cx.new(|cx| {
                InputState::new(window, cx).placeholder("PEM 私钥路径（合并 PEM 时可留空）")
            }),
            client_cert_password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("PKCS#12 口令（可选）")
                    .masked(true)
            }),
            client_cert_error: None,
            server_cert_table,
            selected_server_cert: None,
            inspected_server_cert: None,
            server_cert_copied: None,
        }
    }

    pub(super) fn refresh_client_cert_table(
        &mut self,
        len: usize,
        cx: &mut Context<Settings>,
    ) {
        if self.client_cert_selected.is_some_and(|idx| idx >= len) {
            self.client_cert_selected = len.checked_sub(1);
        }
        let sel = self.client_cert_selected;
        self.client_cert_table.update(cx, |table, cx| {
            if let Some(row) = sel {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
            cx.notify();
        });
    }

    pub(super) fn cache_client_cert_summary(
        &mut self,
        rule: ClientCertificateRule,
        summary: ClientCertificateSummary,
    ) {
        if let Some((_, slot)) = self
            .client_cert_summary_cache
            .iter_mut()
            .find(|(cached, _)| {
                cached.format == rule.format
                    && cached.certificate_bytes == rule.certificate_bytes
                    && cached.private_key_pem == rule.private_key_pem
                    && cached.password == rule.password
            })
        {
            *slot = summary;
            return;
        }
        if self.client_cert_summary_cache.len() >= 128 {
            self.client_cert_summary_cache.remove(0);
        }
        self.client_cert_summary_cache.push((rule, summary));
    }

    pub(super) fn sync_client_cert_summaries(&mut self, rules: &[ClientCertificateRule]) {
        for rule in rules {
            let cached = self.client_cert_summary_cache.iter().any(|(c, _)| {
                c.format == rule.format
                    && c.certificate_bytes == rule.certificate_bytes
                    && c.private_key_pem == rule.private_key_pem
                    && c.password == rule.password
            });
            if !cached
                && rule.format == ClientCertificateFormat::Pem
                && let Ok(parsed) = rule.parse_identity()
            {
                self.cache_client_cert_summary(rule.clone(), parsed.summary().clone());
            }
        }
    }

    pub(super) fn lookup_client_cert_summary(
        &self,
        rule: &ClientCertificateRule,
    ) -> Option<&ClientCertificateSummary> {
        self.client_cert_summary_cache
            .iter()
            .find(|(c, _)| {
                c.format == rule.format
                    && c.certificate_bytes == rule.certificate_bytes
                    && c.private_key_pem == rule.private_key_pem
                    && c.password == rule.password
            })
            .map(|(_, summary)| summary)
    }
}

pub(super) fn indexed_checkbox(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
) -> Checkbox {
    let label = label.into();
    let has_label = !label.is_empty();
    Checkbox::new(id)
        .accessibility_label(label.clone())
        .checked(checked)
        .disabled(!enabled)
        .flex()
        .items_center()
        .when(has_label, |this| this.gap(px(8.)).px(px(2.)).py(px(1.5)))
        .rounded(px(2.))
        .focus_visible(|style| style.bg(theme::selected()))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .child(
            CheckboxIndicator::new()
                .checked(checked)
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(settings_checkbox_border(checked))
                .bg(if checked {
                    settings_blue()
                } else {
                    theme::surface()
                })
                .rounded(px(3.))
                .when(checked, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(px(11.5))
                            .text_color(rgb(0xffffff)),
                    )
                }),
        )
        .when(has_label, |this| this.child(label))
}

fn mio_icon_color() -> Hsla {
    rgb(if theme::is_dark() { 0xdfe1e5 } else { 0x1e1e1e }).into()
}

pub(super) fn mio_table_border() -> Hsla {
    rgb(if theme::is_dark() { 0x45494e } else { 0xd0d0d0 }).into()
}

pub(super) fn mio_table_header_height() -> Pixels {
    theme::scaled_px(24.).max(px(24.))
}

pub(super) fn mio_scope_pill(label: &'static str, active: bool) -> gpui_kit::Div {
    let dark = theme::is_dark();
    let (bg, fg) = if active {
        (
            rgb(if dark { 0x273b54 } else { 0xc8d7ec }),
            rgb(if dark { 0xd4e6fa } else { 0x1f2d3d }),
        )
    } else {
        (
            rgb(if dark { 0x35383e } else { 0xebebeb }),
            rgb(if dark { 0xb8bcc4 } else { 0x2b2b2b }),
        )
    };
    div()
        .flex_shrink_0()
        .h(theme::scaled_px(19.).max(px(19.)))
        .px(px(8.))
        .rounded_full()
        .flex()
        .items_center()
        .bg(bg)
        .text_size(theme::scaled_px(11.5))
        .text_color(fg)
        .whitespace_nowrap()
        .child(label)
}

pub(super) fn mio_project_pill() -> gpui_kit::Div {
    mio_scope_pill("项目设置", true)
}

pub(super) fn mio_user_pill_with_id(id: &'static str) -> impl IntoElement {
    mio_scope_pill("用户设置", true)
        .id(id)
        .test_support()
        .aria_label("用户设置")
}

pub(super) fn mio_dual_scope_pills(override_project: bool) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .flex_shrink_0()
        .child(mio_scope_pill("用户设置", !override_project))
        .child(mio_scope_pill("项目设置", override_project))
}

pub(super) fn mio_section_heading(
    id: impl Into<gpui_kit::ElementId>,
    title: &'static str,
    badge: impl IntoElement,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .child(
                    div()
                        .w(px(22.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(gpui_kit::assets::IconName::CircleQuestionMark)
                                .size(px(21.))
                                .text_color(mio_icon_color()),
                        ),
                )
                .child(
                    div()
                        .id(id)
                        .test_support()
                        .role(Role::Heading)
                        .aria_label(title)
                        .text_size(theme::scaled_px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::foreground())
                        .child(title),
                ),
        )
        .child(badge)
}

pub(super) fn mio_desc_row(desc: &'static str) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .w(px(22.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(gpui_kit::assets::IconName::Settings)
                        .size(px(21.))
                        .text_color(mio_icon_color()),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::foreground())
                .child(desc),
        )
}

pub(super) fn mio_desc_row_with_reset(
    id: &'static str,
    tooltip: &'static str,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    desc: &'static str,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .w(px(22.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Button::new(id)
                        .ghost()
                        .size(px(22.))
                        .p_0()
                        .disabled(disabled)
                        .accessibility_label(tooltip)
                        .tooltip(tooltip)
                        .child(
                            Icon::new(gpui_kit::assets::IconName::RotateCcw)
                                .size(px(21.))
                                .text_color(mio_icon_color()),
                        )
                        .on_click(on_click),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::foreground())
                .child(desc),
        )
}

pub(super) fn mio_action_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Button {
    let dark = theme::is_dark();
    let (bg, border, fg, hover_bg) = (
        rgb(if dark { 0x3c3f44 } else { 0xeaeaea }),
        rgb(if dark { 0x4c5057 } else { 0xd4d4d4 }),
        rgb(if dark { 0xdfe1e5 } else { 0x000000 }),
        rgb(if dark { 0x474b52 } else { 0xdedede }),
    );
    button(id, label)
        .disabled(disabled)
        .h(theme::scaled_px(24.).max(px(24.)))
        .rounded(px(4.))
        .bg(bg)
        .border_color(border)
        .text_color(fg)
        .font_weight(FontWeight::SEMIBOLD)
        .when(!disabled, |this| this.hover(move |style| style.bg(hover_bg)))
}

pub(super) fn mio_right_triangle() -> gpui_kit::Div {
    div()
        .flex_shrink_0()
        .ml(px(2.))
        .h_full()
        .flex()
        .items_center()
        .child(
            Icon::default()
                .data(MIO_TABLE_TRIANGLE_SVG)
                .w(px(7.))
                .h(px(10.))
                .text_color(rgb(if theme::is_dark() { 0xd4d4d4 } else { 0x000000 })),
        )
}

pub(super) fn mio_table_frame<D: TableDelegate>(
    table: &Entity<TableState<D>>,
    label: &'static str,
) -> gpui_kit::Div {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .border_1()
                .border_color(mio_table_border())
                .bg(theme::surface())
                .overflow_hidden()
                .child(
                    DataTable::new(table)
                        .with_size(theme::scaled_px(23.).max(px(20.)))
                        .header_row_height(mio_table_header_height())
                        .bordered(false)
                        .stripe(false)
                        .scrollbar_visible(true, false)
                        .scroll_bounce(false)
                        .accessibility_label(label),
                ),
        )
        .child(mio_right_triangle())
}

pub(super) fn mio_table_td_text(text: impl Into<SharedString>) -> gpui_kit::Div {
    div()
        .size_full()
        .px(px(7.))
        .flex()
        .items_center()
        .child(div().min_w_0().flex_1().truncate().child(text.into()))
}

pub(super) fn mio_table_td_checkbox(checkbox: impl IntoElement) -> gpui_kit::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(checkbox)
}

pub(super) fn mio_table_empty() -> gpui_kit::Div {
    div().size_full().bg(theme::surface())
}
