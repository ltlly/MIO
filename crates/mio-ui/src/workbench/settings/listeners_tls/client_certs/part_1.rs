use gpui_kit::PathPromptOptions;

use super::*;

impl Settings {
    pub(super) fn pick_client_cert_file(
        &mut self,
        key_file: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled {
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
        let cert_path = cert_path_raw.trim();
        if cert_path.is_empty() {
            self.listeners_tls.client_cert_error = Some("请填写客户端证书文件路径。".into());
            cx.notify();
            return;
        }
        let certificate_bytes = match read_bounded_cert_file(Path::new(cert_path), "客户端证书文件")
        {
            Ok(bytes) => bytes,
            Err(error) => {
                self.listeners_tls.client_cert_error = Some(error);
                cx.notify();
                return;
            }
        };
        let format = self.listeners_tls.client_cert_format;
        let (private_key_pem, password) = match format {
            ClientCertificateFormat::Pem => {
                let key_path = key_path_raw.trim();
                let key_bytes = if key_path.is_empty() {
                    Vec::new()
                } else {
                    match read_bounded_cert_file(Path::new(key_path), "客户端 PEM 私钥文件")
                    {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            self.listeners_tls.client_cert_error = Some(error);
                            cx.notify();
                            return;
                        }
                    }
                };
                (key_bytes, String::new())
            }
            ClientCertificateFormat::Pkcs12 => {
                if password_raw.len() > MAX_CLIENT_CERT_PASSWORD_BYTES
                    || password_raw.contains(['\r', '\n', '\0'])
                {
                    self.listeners_tls.client_cert_error = Some(format!(
                        "PKCS#12 口令不能超过 {MAX_CLIENT_CERT_PASSWORD_BYTES} 字节或包含换行／空字节。"
                    ));
                    cx.notify();
                    return;
                }
                (Vec::new(), password_raw.to_string())
            }
        };
        let rule = ClientCertificateRule {
            enabled: true,
            host_pattern,
            format,
            certificate_bytes,
            private_key_pem,
            password,
        };
        if format == ClientCertificateFormat::Pem
            && let Err(error) = rule.parse_identity()
        {
            self.listeners_tls.client_cert_error = Some(error.to_string());
            cx.notify();
            return;
        }
        let project = self.state.project;
        let outcome = self.owner.update(cx, |view, cx| {
            view.add_client_certificate_rule(project, rule, cx)
        });
        match outcome {
            Ok(Ok(())) => {
                self.listeners_tls.client_cert_error = None;
                self.listeners_tls.client_cert_editor_visible = false;
                for input in [
                    &self.listeners_tls.client_cert_host,
                    &self.listeners_tls.client_cert_path,
                    &self.listeners_tls.client_cert_key_path,
                    &self.listeners_tls.client_cert_password,
                ] {
                    input.update(cx, |state, cx| state.set_value("", window, cx));
                }
            }
            Ok(Err(error)) => {
                self.listeners_tls.client_cert_error = Some(error);
            }
            Err(_) => {
                self.listeners_tls.client_cert_error =
                    Some("主工作台已关闭；客户端证书未添加。".into());
            }
        }
        cx.notify();
    }
}
