use std::path::PathBuf;

use gpui_kit::{PathPromptOptions, component::IndexPath};
use mio_core::ClientCertificateSummary;

use super::*;

impl Settings {
    pub(in crate::workbench::settings::listeners_tls) fn open_client_cert_editor(
        &mut self,
        edit_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled || self.listeners_tls.client_cert_saving {
            return;
        }
        self.listeners_tls.client_cert_error = None;
        if let Some(index) = edit_index {
            let Some(rule) = self.state.client_certificates.get(index).cloned() else {
                return;
            };
            self.listeners_tls.client_cert_editing = Some(index);
            self.listeners_tls.client_cert_format = rule.format;
            let fmt_idx = match rule.format {
                ClientCertificateFormat::Pem => 0,
                ClientCertificateFormat::Pkcs12 => 1,
            };
            self.listeners_tls
                .client_cert_format_select
                .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(fmt_idx)), window, cx));
            self.listeners_tls
                .client_cert_host
                .update(cx, |input, cx| input.set_value(rule.host_pattern, window, cx));
            for input in [
                &self.listeners_tls.client_cert_path,
                &self.listeners_tls.client_cert_key_path,
                &self.listeners_tls.client_cert_password,
            ] {
                input.update(cx, |state, cx| state.set_value("", window, cx));
            }
            self.listeners_tls.client_cert_editor_visible = true;
        } else {
            self.listeners_tls.client_cert_editing = None;
            self.listeners_tls.client_cert_format = ClientCertificateFormat::Pem;
            self.listeners_tls
                .client_cert_format_select
                .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(0)), window, cx));
            for input in [
                &self.listeners_tls.client_cert_host,
                &self.listeners_tls.client_cert_path,
                &self.listeners_tls.client_cert_key_path,
                &self.listeners_tls.client_cert_password,
            ] {
                input.update(cx, |state, cx| state.set_value("", window, cx));
            }
            self.listeners_tls.client_cert_editor_visible = true;
        }
        cx.notify();
    }

    pub(super) fn pick_client_cert_file(
        &mut self,
        key_file: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled || self.listeners_tls.client_cert_saving {
            return;
        }
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(if key_file {
                    "选择客户端 PEM 私钥".into()
                } else {
                    "选择客户端证书".into()
                }),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(selected))) = paths.await
                && let Some(first) = selected.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    let target = if key_file {
                        &this.listeners_tls.client_cert_key_path
                    } else {
                        &this.listeners_tls.client_cert_path
                    };
                    target.update(cx, |input, cx| {
                        input.set_value(first.display().to_string(), window, cx)
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn submit_add_client_cert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.enabled || self.listeners_tls.client_cert_saving {
            return;
        }
        let snapshots = [
            &self.listeners_tls.client_cert_host,
            &self.listeners_tls.client_cert_path,
            &self.listeners_tls.client_cert_key_path,
            &self.listeners_tls.client_cert_password,
        ]
        .map(|input| editing::input_snapshot(input, window, cx));
        let [
            Ok(host_raw),
            Ok(cert_path_raw),
            Ok(key_path_raw),
            Ok(password_raw),
        ] = snapshots
        else {
            self.listeners_tls.client_cert_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        };
        let host_pattern = match ClientCertificateRule::validate_host_pattern(host_raw.trim()) {
            Ok(normalized) => normalized,
            Err(error) => {
                self.listeners_tls.client_cert_error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let format = self.listeners_tls.client_cert_format;
        let cert_path = cert_path_raw.trim().to_owned();
        let key_path = key_path_raw.trim().to_owned();
        let password_input = password_raw.to_string();
        if format == ClientCertificateFormat::Pkcs12
            && (password_input.len() > MAX_CLIENT_CERT_PASSWORD_BYTES
                || password_input.contains(['\r', '\n', '\0']))
        {
            self.listeners_tls.client_cert_error = Some(format!(
                "PKCS#12 口令不能超过 {MAX_CLIENT_CERT_PASSWORD_BYTES} 字节或包含换行／空字节。"
            ));
            cx.notify();
            return;
        }

        let edit_index = self.listeners_tls.client_cert_editing;
        let existing_rule = edit_index.and_then(|idx| self.state.client_certificates.get(idx).cloned());
        if cert_path.is_empty() && existing_rule.as_ref().is_none_or(|r| r.format != format) {
            self.listeners_tls.client_cert_error = Some("请填写客户端证书文件路径。".into());
            cx.notify();
            return;
        }

        let project = self.state.project;
        self.listeners_tls.client_cert_saving = true;
        self.listeners_tls.client_cert_error = None;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let parsed_outcome: Result<(ClientCertificateRule, ClientCertificateSummary), String> = cx
                .background_executor()
                .spawn(async move {
                    let certificate_bytes = if cert_path.is_empty() {
                        existing_rule
                            .as_ref()
                            .map(|r| r.certificate_bytes.clone())
                            .unwrap_or_default()
                    } else {
                        read_bounded_cert_file(&PathBuf::from(&cert_path), "客户端证书文件")?
                    };
                    let (private_key_pem, password) = match format {
                        ClientCertificateFormat::Pem => {
                            let key_bytes = if !key_path.is_empty() {
                                read_bounded_cert_file(&PathBuf::from(&key_path), "客户端 PEM 私钥文件")?
                            } else if cert_path.is_empty() {
                                existing_rule
                                    .as_ref()
                                    .map(|r| r.private_key_pem.clone())
                                    .unwrap_or_default()
                            } else {
                                Vec::new()
                            };
                            (key_bytes, String::new())
                        }
                        ClientCertificateFormat::Pkcs12 => {
                            let pw = if password_input.is_empty() && cert_path.is_empty() {
                                existing_rule
                                    .as_ref()
                                    .map(|r| r.password.clone())
                                    .unwrap_or_default()
                            } else {
                                password_input
                            };
                            (Vec::new(), pw)
                        }
                    };
                    let rule = ClientCertificateRule {
                        enabled: existing_rule.as_ref().is_none_or(|r| r.enabled),
                        host_pattern,
                        format,
                        certificate_bytes,
                        private_key_pem,
                        password,
                    };
                    let parsed = rule.parse_identity().map_err(|err| err.to_string())?;
                    let summary = parsed.summary().clone();
                    Ok((rule, summary))
                })
                .await;

            let _ = this.update_in(cx, |this, window, cx| {
                this.listeners_tls.client_cert_saving = false;
                if this.state.project != project {
                    cx.notify();
                    return;
                }
                match parsed_outcome {
                    Ok((rule, summary)) => {
                        this.listeners_tls
                            .cache_client_cert_summary(rule.clone(), summary);
                        let outcome = this.owner.update(cx, |view, cx| {
                            if let Some(idx) = edit_index {
                                view.edit_client_certificate_rule(project, idx, rule, cx)
                            } else {
                                view.add_client_certificate_rule(project, rule, cx)
                            }
                        });
                        match outcome {
                            Ok(Ok(())) => {
                                this.listeners_tls.client_cert_error = None;
                                this.listeners_tls.client_cert_editor_visible = false;
                                this.listeners_tls.client_cert_editing = None;
                                for input in [
                                    &this.listeners_tls.client_cert_host,
                                    &this.listeners_tls.client_cert_path,
                                    &this.listeners_tls.client_cert_key_path,
                                    &this.listeners_tls.client_cert_password,
                                ] {
                                    input.update(cx, |state, cx| state.set_value("", window, cx));
                                }
                            }
                            Ok(Err(error)) => {
                                this.listeners_tls.client_cert_error = Some(error);
                            }
                            Err(_) => {
                                this.listeners_tls.client_cert_error =
                                    Some("主工作台已关闭；客户端证书未保存。".into());
                            }
                        }
                    }
                    Err(error) => {
                        this.listeners_tls.client_cert_error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
