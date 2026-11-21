use std::path::Path;

use gpui_kit::accesskit::Role;
use gpui_kit::base::{CheckboxState, Switch as BaseSwitch};
use gpui_kit::component::table::{Column, TableDelegate};
use mio_core::{
    ClientCertificateFormat, ClientCertificateRule, MAX_CLIENT_CERT_BYTES,
    MAX_CLIENT_CERT_PASSWORD_BYTES,
};

use super::*;

pub(super) struct ClientCertTable {
    pub(super) owner: WeakEntity<Settings>,
}

const CLIENT_CERT_COLUMNS: [(&str, f32); 8] = [
    ("Enabled", 72.),
    ("Host", 110.),
    ("Type", 58.),
    ("Alias", 84.),
    ("Subject", 136.),
    ("Issuer", 136.),
    ("Intermediate CA", 144.),
    ("Key", 82.),
];

impl TableDelegate for ClientCertTable {
    fn columns_count(&self, _: &App) -> usize {
        CLIENT_CERT_COLUMNS.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        Column::new(index.to_string(), CLIENT_CERT_COLUMNS[index].0)
            .width((CLIENT_CERT_COLUMNS[index].1 * theme::ui_scale()).round())
            .p_0()
            .movable(false)
            .resizable(index + 1 < CLIENT_CERT_COLUMNS.len())
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |owner| owner.read(cx).state.client_certificates.len())
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (label, _) = CLIENT_CERT_COLUMNS[col_ix];
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
        div().id(("settings-client-cert-row", row)).border_b_0()
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (rule, enabled, project, wb) = {
            let settings = owner.read(cx);
            let Some(rule) = settings.state.client_certificates.get(row).cloned() else {
                return div().into_any_element();
            };
            (
                rule,
                settings.state.enabled,
                settings.state.project,
                settings.owner.clone(),
            )
        };
        if column == 0 {
            return mio_table_td_checkbox(
                indexed_checkbox(("settings-client-cert-toggle", row), "", rule.enabled, enabled)
                    .accessibility_label(format!("启用客户端证书 {}", rule.host_pattern))
                    .on_change(move |state, _, _, cx| {
                        let checked = state == CheckboxState::Checked;
                        let res = wb.update(cx, |view, cx| {
                            view.toggle_client_certificate_rule(project, row, checked, cx)
                        });
                        owner.update(cx, |this, cx| {
                            this.listeners_tls.client_cert_error = match res {
                                Ok(Ok(())) => None,
                                Ok(Err(error)) => Some(error),
                                Err(_) => Some("主工作台已关闭；客户端证书未修改。".into()),
                            };
                            cx.notify();
                        });
                    }),
            )
            .into_any_element();
        }
        mio_table_td_text(client_cert_cell(&rule, column)).into_any_element()
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        mio_table_empty()
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
        let rule = settings.state.client_certificates.get(row)?;
        Some(client_cert_cell(rule, column).into())
    }
}

fn client_cert_cell(rule: &ClientCertificateRule, column: usize) -> String {
    match column {
        0 => if rule.enabled { "Yes" } else { "No" }.into(),
        1 => rule.host_pattern.clone(),
        2 => match rule.format {
            ClientCertificateFormat::Pem => "PEM",
            ClientCertificateFormat::Pkcs12 => "PKCS#12",
        }
        .into(),
        3 | 4 => {
            if rule.format == ClientCertificateFormat::Pem {
                rule.parse_identity()
                    .map(|id| id.summary().subject_hint.clone())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
        5 => String::new(),
        6 => {
            if rule.format == ClientCertificateFormat::Pem {
                rule.parse_identity()
                    .ok()
                    .map(|id| id.summary().cert_count.saturating_sub(1))
                    .filter(|&count| count > 0)
                    .map(|count| format!("{count} 个"))
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
        _ => match (rule.format, rule.private_key_pem.is_empty()) {
            (ClientCertificateFormat::Pkcs12, _) => "PKCS#12".into(),
            (ClientCertificateFormat::Pem, false) => "PEM 私钥".into(),
            (ClientCertificateFormat::Pem, true) => "内嵌私钥".into(),
        },
    }
}

fn read_bounded_cert_file(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    mio_core::ui_files::read_regular_file_bounded(
        path,
        MAX_CLIENT_CERT_BYTES as u64,
        false,
        label,
    )
}

impl Workbench {
    fn apply_client_certificates_change(
        &mut self,
        project: Option<ProjectKey>,
        next: Vec<ClientCertificateRule>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return Err("当前项目尚未就绪或后台不可用；客户端证书未修改。".into());
        }
        let mut candidate = self.workspace.clone();
        candidate.client_certificates = next;
        candidate
            .validate_structure()
            .map_err(|error| error.to_string())?;
        if self.workspace.client_certificates != candidate.client_certificates {
            self.workspace.client_certificates = candidate.client_certificates;
            self.save_workspace(cx);
        }
        self.error = None;
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn add_client_certificate_rule(
        &mut self,
        project: Option<ProjectKey>,
        rule: ClientCertificateRule,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mut next = self.workspace.client_certificates.clone();
        next.push(rule);
        self.apply_client_certificates_change(project, next, cx)
    }

    pub(in crate::workbench) fn toggle_client_certificate_rule(
        &mut self,
        project: Option<ProjectKey>,
        index: usize,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mut next = self.workspace.client_certificates.clone();
        let Some(rule) = next.get_mut(index) else {
            return Ok(());
        };
        rule.enabled = enabled;
        self.apply_client_certificates_change(project, next, cx)
    }

    pub(in crate::workbench) fn remove_client_certificate_rule(
        &mut self,
        project: Option<ProjectKey>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mut next = self.workspace.client_certificates.clone();
        if index >= next.len() {
            return Ok(());
        }
        next.remove(index);
        self.apply_client_certificates_change(project, next, cx)
    }

    pub(in crate::workbench) fn move_client_certificate_rule(
        &mut self,
        project: Option<ProjectKey>,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mut next = self.workspace.client_certificates.clone();
        if from >= next.len() || to >= next.len() || from == to {
            return Ok(());
        }
        next.swap(from, to);
        self.apply_client_certificates_change(project, next, cx)
    }
}

fn mio_override_card(
    id: &'static str,
    checked: bool,
    enabled: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> gpui_kit::Div {
    let dark = theme::is_dark();
    let track_bg: gpui_kit::Hsla = if checked {
        settings_blue()
    } else {
        rgb(if dark { 0x4e535b } else { 0xb0afaf }).into()
    };
    let thumb_bg = if checked {
        rgb(0xffffff)
    } else if dark {
        rgb(0xd4d4d4)
    } else {
        rgb(0x2b2b2b)
    };
    div()
        .ml(px(30.))
        .px(px(12.))
        .py(px(9.))
        .rounded(px(3.))
        .border_1()
        .border_color(mio_table_border())
        .bg(theme::surface())
        .child(
            BaseSwitch::new(id)
                .checked(checked)
                .disabled(!enabled)
                .accessibility_label("仅为当前项目覆盖选项")
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_not_allowed()))
                .on_change(move |next, _, window, cx| on_change(&next, window, cx))
                .child(
                    div()
                        .w(px(24.))
                        .h(px(13.))
                        .flex_shrink_0()
                        .rounded_full()
                        .p(px(2.))
                        .flex()
                        .items_center()
                        .when(checked, |this| this.justify_end())
                        .when(!checked, |this| this.justify_start())
                        .bg(track_bg)
                        .child(div().size(px(9.)).rounded_full().bg(thumb_bg)),
                )
                .child(
                    div()
                        .text_size(theme::ui_font_size())
                        .text_color(theme::foreground())
                        .child("仅为当前项目覆盖选项"),
                ),
        )
}

mod part_1;
mod part_2;
