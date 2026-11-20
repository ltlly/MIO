use super::*;

pub struct PluginSupervisor {
    pub identity: ChannelIdentity,
    pub manifest: PluginManifest,
    pub initial_panel: Arc<PanelSnapshot>,
    pub events: mpsc::Receiver<PluginEvent>,
    handle: PluginHandle,
    task: Option<JoinHandle<Result<()>>>,
    terminal_error: Option<String>,
}
#[cfg(test)]
pub(crate) fn test_supervisor(
    identity: ChannelIdentity,
    manifest: PluginManifest,
    panel: Arc<PanelSnapshot>,
) -> (PluginSupervisor, mpsc::Sender<PluginEvent>) {
    let (control, _commands) = mpsc::channel(QUEUE_CAPACITY);
    let (events, receiver) = mpsc::channel(QUEUE_CAPACITY);
    (
        PluginSupervisor {
            identity,
            manifest,
            initial_panel: panel,
            events: receiver,
            handle: PluginHandle {
                identity,
                control,
                cancel: CancellationToken::new(),
            },
            task: None,
            terminal_error: None,
        },
        events,
    )
}
impl PluginSupervisor {
    pub async fn start(
        package: VerifiedPackage,
        project: ProjectKey,
        trust: NativeTrust,
        services: mpsc::Sender<ServiceRequest>,
    ) -> Result<Self> {
        ensure!(
            trust.executable == package.executable
                && trust.executable_size == package.executable_size
                && trust.modified == package.modified
                && trust.plugin_id == package.manifest.id
                && trust.version == package.manifest.version
                && trust.manifest_bytes == package.manifest_bytes,
            "本地程序信任确认不属于此插件或文件版本"
        );
        ensure!(
            trust
                .grants
                .iter()
                .all(|p| package.manifest.permissions.contains(p)),
            "授权与当前 manifest 不匹配，请重新确认"
        );
        package.unchanged()?;
        ensure!(
            !project.id.is_nil() && project.generation != 0,
            "项目身份无效"
        );
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project,
        };
        let (transport, panel_state, incoming_sequence) = if package.manifest.engine == "wasm" {
            let module =
                wasm::WasmModule::parse(package.wasm_bytes.as_deref().context("Wasm 字节缺失")?)?;
            let snapshot = wasm::initial_wasm_panel(&package.manifest, &module, &trust.grants)?;
            let state = PanelState::new(snapshot).map_err(anyhow::Error::msg)?;
            (WorkerTransport::Wasm { module }, state, 1)
        } else {
            let mut command = Command::new(&package.executable);
            command
                .current_dir(&package.work_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            // Native code remains trusted and unsandboxed; do not hand it host secret environment values.
            command.env_clear();
            #[cfg(windows)]
            for key in ["SystemRoot", "WINDIR", "SYSTEMDRIVE", "TEMP", "TMP"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.as_std_mut().process_group(0);
            }
            let mut child = command.spawn().context("无法启动可信 native worker")?;
            #[cfg(windows)]
            let job = WindowsJob::create_and_assign(&child).ok();
            let mut stdin = child.stdin.take().context("缺少插件 stdin")?;
            let mut stdout = child.stdout.take().context("缺少插件 stdout")?;
            let stderr = child.stderr.take().context("缺少插件 stderr")?;
            let hello = Envelope::new(
                identity,
                1,
                0,
                envelope::Body::Hello(protocol::Hello {
                    max_frame_bytes: protocol::MAX_FRAME_BYTES as u32,
                    max_operation_ms: MAX_OPERATION.as_millis() as u32,
                    granted_permissions: trust.grants.iter().map(|p| *p as i32).collect(),
                }),
                MAX_OPERATION,
            );
            let handshake = async {
                stdin.write_all(&protocol::encode_frame(&hello)?).await?;
                stdin.flush().await?;
                let registration = read_async_frame(&mut stdout).await?;
                registration
                    .check_channel(identity, 0)
                    .map_err(anyhow::Error::msg)?;
                ensure!(registration.in_reply_to == 1, "插件注册未关联 Hello");
                let Some(envelope::Body::Register(register)) = &registration.body else {
                    bail!("插件没有返回 Register");
                };
                ensure!(
                    register.plugin_id == package.manifest.id
                        && register.plugin_version == package.manifest.version,
                    "注册身份与已确认 manifest 不符"
                );
                ensure!(
                    register
                        .required_components
                        .iter()
                        .all(|c| protocol::Component::try_from(*c).is_ok()),
                    "插件要求不支持的原生组件"
                );
                let state = if register.panel.is_none()
                    && !package.manifest.contributions.iter().any(|c| {
                        matches!(
                            c.as_str(),
                            "suite_panel" | "message_editor" | "message_response_editor"
                        )
                    }) {
                    PanelState::new(PanelSnapshot {
                        revision: registration.revision.max(1),
                        title: package.manifest.id.clone(),
                        root: Some(panel::Node::new(
                            1,
                            "插件状态",
                            panel::node::Kind::Text(panel::Text {
                                text: format!(
                                    "{} ({}) Hook 插件运行中",
                                    package.manifest.id, package.manifest.version
                                ),
                                tone: 0,
                            }),
                        )),
                    })
                    .map_err(anyhow::Error::msg)?
                } else {
                    let panel = register.panel.clone().context("插件注册没有完整面板")?;
                    ensure!(
                        registration.revision == panel.revision,
                        "注册 revision 不匹配"
                    );
                    PanelState::new(panel).map_err(anyhow::Error::msg)?
                };
                let mut ready = Envelope::new(
                    identity,
                    2,
                    state.snapshot().revision,
                    envelope::Body::Ready(panel::Empty {}),
                    MAX_OPERATION,
                );
                ready.in_reply_to = registration.request_id;
                stdin.write_all(&protocol::encode_frame(&ready)?).await?;
                stdin.flush().await?;
                Ok::<_, anyhow::Error>((stdin, stdout, state, registration.request_id))
            };
            match timeout(MAX_OPERATION, handshake).await {
                Ok(Ok((stdin, stdout, state, seq))) => (
                    WorkerTransport::Native {
                        child,
                        #[cfg(windows)]
                        job,
                        stdin,
                        stdout,
                        stderr,
                    },
                    state,
                    seq,
                ),
                result => {
                    #[cfg(windows)]
                    if let Some(ref j) = job {
                        let _ = j.terminate();
                    }
                    let terminated = kill_worker_group(&mut child);
                    timeout(REAP_DEADLINE, child.wait())
                        .await
                        .context("插件握手失败，已发出终止但回收进程超时")?
                        .context("插件握手失败，回收进程失败")?;
                    terminated?;
                    match result {
                        Ok(Err(error)) => return Err(error),
                        _ => bail!("插件握手超时；已终止并回收新实例"),
                    }
                }
            }
        };
        let initial_panel = Arc::new(panel_state.snapshot().clone());
        let (control, commands) = mpsc::channel(QUEUE_CAPACITY);
        let (events, event_receiver) = mpsc::channel(QUEUE_CAPACITY);
        let cancel = CancellationToken::new();
        let handle = PluginHandle {
            identity,
            control,
            cancel: cancel.clone(),
        };
        let manifest = package.manifest;
        let runtime = Running {
            identity,
            plugin_id: manifest.id.clone(),
            panel: panel_state,
            broker: PermissionBroker::new(identity, trust.grants),
            services,
            events,
            cancel,
            outgoing_sequence: 2,
            incoming_sequence,
            active_resource: None,
            opening_request: None,
            rate_epoch: Instant::now(),
            frame_count: 0,
            requests: HashMap::new(),
            service_cancels: HashMap::new(),
            revoked_resources: HashMap::new(),
            service_tasks: JoinSet::new(),
        };
        let task = tokio::spawn(runtime.run(transport, commands));
        Ok(Self {
            identity,
            manifest,
            initial_panel,
            events: event_receiver,
            handle,
            task: Some(task),
            terminal_error: None,
        })
    }

    pub fn handle(&self) -> PluginHandle {
        self.handle.clone()
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.handle.cancel();
        if let Some(error) = &self.terminal_error {
            bail!(error.clone());
        }
        let Some(task) = self.task.as_mut() else {
            return Ok(());
        };
        let result = task
            .await
            .context("插件 supervisor 异常退出")
            .and_then(|result| result);
        self.task = None;
        if let Err(error) = &result {
            self.terminal_error = Some(format!("{error:#}"));
        }
        result
    }

    /// A failed candidate leaves the current worker and its contributions untouched.
    pub async fn reload(
        &mut self,
        package: VerifiedPackage,
        trust: NativeTrust,
        services: mpsc::Sender<ServiceRequest>,
    ) -> Result<()> {
        let mut next = Self::start(package, self.identity.project, trust, services).await?;
        if let Err(error) = self.stop().await {
            let _ = next.stop().await;
            return Err(error);
        }
        *self = next;
        Ok(())
    }
}
impl Drop for PluginSupervisor {
    fn drop(&mut self) {
        self.handle.cancel();
    }
}
