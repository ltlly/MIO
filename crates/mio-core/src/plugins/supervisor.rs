use super::*;

pub struct PluginSupervisor {
    pub identity: ChannelIdentity,
    pub manifest: PluginManifest,
    pub initial_panel: Arc<PanelSnapshot>,
    pub supports_panel_multiplexing: bool,
    pub events: mpsc::Receiver<PluginEvent>,
    handle: PluginHandle,
    task: Option<JoinHandle<Result<()>>>,
    terminal_error: Option<String>,
}
#[cfg(test)]
pub(crate) fn spawn_test_multiplex_supervisor(
    identity: ChannelIdentity,
    manifest: PluginManifest,
    panel: Arc<PanelSnapshot>,
) -> (
    PluginSupervisor,
    mpsc::Sender<PluginEvent>,
    Arc<std::sync::Mutex<Vec<(String, u64, Uuid)>>>,
) {
    let (control, mut commands) = mpsc::channel::<SequencedControl>(QUEUE_CAPACITY);
    let (urgent_control, mut urgent_commands) = mpsc::unbounded_channel::<SequencedControl>();
    let control_order = Arc::new(std::sync::Mutex::new(ControlOrderState::default()));
    let (events, receiver) = mpsc::channel(QUEUE_CAPACITY);
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log_task = log.clone();
    let events_task = events.clone();
    let initial_rev = panel.revision.max(1);
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let task = tokio::spawn(async move {
        let mut revisions: HashMap<u64, u64> = HashMap::from([(protocol::PANEL_ID, initial_rev)]);
        let mut urgent_open = true;
        loop {
            let cmd = tokio::select! {
                biased;
                _ = cancel_task.cancelled() => break,
                next = urgent_commands.recv(), if urgent_open => match next {
                    Some(cmd) => cmd.command,
                    None => {
                        urgent_open = false;
                        continue;
                    }
                },
                next = commands.recv() => match next {
                    Some(cmd) => cmd.command,
                    None => break,
                },
            };
            match cmd {
                Control::Open {
                    panel_id,
                    id,
                    request,
                    reply,
                    ..
                } => {
                    log_task
                        .lock()
                        .unwrap()
                        .push(("open".into(), panel_id, id));
                    let rev = revisions.entry(panel_id).or_insert(0);
                    *rev += 1;
                    let snap = Arc::new(PanelSnapshot {
                        revision: *rev,
                        title: format!("panel-{panel_id}"),
                        root: Some(panel::Node::new(
                            1,
                            "editor",
                            panel::node::Kind::Editor(panel::Editor {
                                document_id: 1,
                                revision: 1,
                                contents: format!("panel-{panel_id}:{}", request.url).into_bytes(),
                                format: panel::EditorFormat::Utf8 as i32,
                                read_only: false,
                            }),
                        )),
                    });
                    let _ = reply.send(Ok(snap));
                }
                Control::OpenResponse {
                    panel_id,
                    id,
                    response,
                    reply,
                    ..
                } => {
                    log_task
                        .lock()
                        .unwrap()
                        .push(("open_response".into(), panel_id, id));
                    let rev = revisions.entry(panel_id).or_insert(0);
                    *rev += 1;
                    let snap = Arc::new(PanelSnapshot {
                        revision: *rev,
                        title: format!("resp-panel-{panel_id}"),
                        root: Some(panel::Node::new(
                            1,
                            "editor",
                            panel::node::Kind::Editor(panel::Editor {
                                document_id: 1,
                                revision: 1,
                                contents: response.body,
                                format: panel::EditorFormat::Utf8 as i32,
                                read_only: false,
                            }),
                        )),
                    });
                    let _ = reply.send(Ok(snap));
                }
                Control::Event { panel_id, event, .. } => {
                    log_task
                        .lock()
                        .unwrap()
                        .push(("event".into(), panel_id, Uuid::nil()));
                    let rev = revisions.entry(panel_id).or_insert(0);
                    *rev += 1;
                    let snap = Arc::new(PanelSnapshot {
                        revision: *rev,
                        title: format!("panel-{panel_id}-ev-{}", event.node_id),
                        root: None,
                    });
                    let _ = events_task
                        .send(PluginEvent::Panel {
                            identity,
                            panel_id,
                            snapshot: snap,
                            editor_bases: vec![],
                            synced_ui_seq: 0,
                            rejected_ui_seqs: vec![],
                        })
                        .await;
                }
                Control::Commit {
                    panel_id,
                    op_id: _,
                    request,
                    cancelled: _,
                    expected_snapshot: _,
                    reply,
                } => {
                    let res_id = protocol::uuid(&request.resource_id).unwrap_or(Uuid::nil());
                    log_task
                        .lock()
                        .unwrap()
                        .push(("commit".into(), panel_id, res_id));
                    let mut req: HttpRequest = request.request.unwrap().try_into().unwrap();
                    req.headers.push(crate::Header {
                        name: "x-panel-id".into(),
                        value: panel_id.to_string().into_bytes(),
                    });
                    let _ = reply.send(Ok(req));
                }
                Control::CommitResponse {
                    panel_id,
                    op_id: _,
                    request,
                    cancelled: _,
                    expected_snapshot: _,
                    reply,
                } => {
                    let res_id = protocol::uuid(&request.resource_id).unwrap_or(Uuid::nil());
                    log_task
                        .lock()
                        .unwrap()
                        .push(("commit_response".into(), panel_id, res_id));
                    let mut resp: HttpResponse = request.response.unwrap().try_into().unwrap();
                    resp.headers.push(crate::Header {
                        name: "x-panel-id".into(),
                        value: panel_id.to_string().into_bytes(),
                    });
                    let _ = reply.send(Ok(resp));
                }
                Control::Revoke {
                    panel_id,
                    id,
                    remove_panel,
                    reply,
                } => {
                    log_task
                        .lock()
                        .unwrap()
                        .push((format!("revoke:{remove_panel}"), panel_id, id));
                    if remove_panel {
                        revisions.remove(&panel_id);
                    }
                    let _ = reply.send(Ok(()));
                }
                Control::RevokeBatch { panel_id, items } => {
                    for (id, reply) in items {
                        log_task
                            .lock()
                            .unwrap()
                            .push(("revoke:false".into(), panel_id, id));
                        let _ = reply.send(Ok(()));
                    }
                }
                Control::CancelPanelOp { .. } => {}
                Control::Hook { reply, .. } => {
                    let _ = reply.send(Err(anyhow::anyhow!("unused in mock")));
                }
            }
        }
        Ok(())
    });
    (
        PluginSupervisor {
            identity,
            manifest,
            initial_panel: panel,
            supports_panel_multiplexing: true,
            events: receiver,
            handle: PluginHandle {
                identity,
                control,
                urgent_control,
                control_order,
                cancel,
            },
            task: Some(task),
            terminal_error: None,
        },
        events,
        log,
    )
}

#[cfg(test)]
pub(crate) fn test_supervisor(
    identity: ChannelIdentity,
    manifest: PluginManifest,
    panel: Arc<PanelSnapshot>,
) -> (PluginSupervisor, mpsc::Sender<PluginEvent>) {
    let (control, _commands) = mpsc::channel(QUEUE_CAPACITY);
    let (urgent_control, _urgent_commands) = mpsc::unbounded_channel();
    let (events, receiver) = mpsc::channel(QUEUE_CAPACITY);
    (
        PluginSupervisor {
            identity,
            manifest,
            initial_panel: panel,
            supports_panel_multiplexing: false,
            events: receiver,
            handle: PluginHandle {
                identity,
                control,
                urgent_control,
                control_order: Arc::new(std::sync::Mutex::new(ControlOrderState::default())),
                cancel: CancellationToken::new(),
            },
            task: None,
            terminal_error: None,
        },
        events,
    )
}
pub(super) async fn handshake_native<W, R>(
    stdin: &mut W,
    stdout: &mut R,
    identity: ChannelIdentity,
    manifest: &PluginManifest,
    grants: &BTreeSet<Permission>,
) -> Result<(PanelState, u64, bool, u32)>
where
    W: tokio::io::AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let hello = Envelope::new(
        identity,
        1,
        0,
        envelope::Body::Hello(protocol::Hello {
            max_frame_bytes: protocol::MAX_FRAME_BYTES as u32,
            max_operation_ms: MAX_OPERATION.as_millis() as u32,
            granted_permissions: grants.iter().map(|p| *p as i32).collect(),
            host_minor: protocol::PROTOCOL_MINOR,
        }),
        MAX_OPERATION,
    )
    .with_minor(protocol::LEGACY_HANDSHAKE_MINOR);
    stdin.write_all(&protocol::encode_frame(&hello)?).await?;
    stdin.flush().await?;
    let registration = read_async_frame(stdout).await?;
    registration
        .check_channel(identity, 0)
        .map_err(anyhow::Error::msg)?;
    ensure!(registration.in_reply_to == 1, "插件注册未关联 Hello");
    let Some(envelope::Body::Register(register)) = &registration.body else {
        bail!("插件没有返回 Register");
    };
    ensure!(
        register.plugin_id == manifest.id && register.plugin_version == manifest.version,
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
        && !manifest.contributions.iter().any(|c| {
            matches!(
                c.as_str(),
                "suite_panel" | "message_editor" | "message_response_editor"
            )
        }) {
        PanelState::new(PanelSnapshot {
            revision: registration.revision.max(1),
            title: manifest.id.clone(),
            root: Some(panel::Node::new(
                1,
                "插件状态",
                panel::node::Kind::Text(panel::Text {
                    text: format!("{} ({}) Hook 插件运行中", manifest.id, manifest.version),
                    tone: 0,
                }),
            )),
        })
        .map_err(anyhow::Error::msg)?
    } else {
        let panel = register.panel.clone().context("插件注册没有完整面板")?;
        ensure!(registration.revision == panel.revision, "注册 revision 不匹配");
        PanelState::new(panel).map_err(anyhow::Error::msg)?
    };
    let session_minor = registration.minor.min(protocol::PROTOCOL_MINOR);
    let mut ready = Envelope::new(
        identity,
        2,
        state.snapshot().revision,
        envelope::Body::Ready(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_minor(session_minor);
    ready.in_reply_to = registration.request_id;
    stdin.write_all(&protocol::encode_frame(&ready)?).await?;
    stdin.flush().await?;
    let supports_panel_multiplexing =
        register.supports_panel_multiplexing && session_minor >= protocol::PROTOCOL_MINOR;
    Ok((
        state,
        registration.request_id,
        supports_panel_multiplexing,
        session_minor,
    ))
}

#[cfg(test)]
pub(crate) async fn spawn_test_duplex_supervisor(
    identity: ChannelIdentity,
    manifest: PluginManifest,
    grants: BTreeSet<Permission>,
    mut stdin: tokio::io::DuplexStream,
    mut stdout: tokio::io::DuplexStream,
) -> Result<(PluginSupervisor, mpsc::Receiver<ServiceRequest>)> {
    let (panel_state, incoming_sequence, supports_panel_multiplexing, protocol_minor) =
        timeout(
            MAX_OPERATION,
            handshake_native(&mut stdin, &mut stdout, identity, &manifest, &grants),
        )
        .await
        .context("测试插件握手超时")??;
    let initial_panel = Arc::new(panel_state.snapshot().clone());
    let (services, service_receiver) = mpsc::channel(QUEUE_CAPACITY);
    let (control, commands) = mpsc::channel(QUEUE_CAPACITY);
    let (urgent_control, urgent_commands) = mpsc::unbounded_channel();
    let (events, event_receiver) = mpsc::channel(QUEUE_CAPACITY);
    let cancel = CancellationToken::new();
    let handle = PluginHandle {
        identity,
        control,
        urgent_control,
        control_order: Arc::new(std::sync::Mutex::new(ControlOrderState::default())),
        cancel: cancel.clone(),
    };
    let runtime = Running {
        identity,
        plugin_id: manifest.id.clone(),
        protocol_minor,
        supports_panel_multiplexing,
        panel: panel_state,
        panels: HashMap::new(),
        broker: PermissionBroker::new(identity, grants),
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
        cancelled_requests: std::collections::VecDeque::new(),
        cancel_notifications: std::collections::VecDeque::new(),
        v14_degraded_resync_ids: std::collections::VecDeque::new(),
        main_cancelled_state_requests: std::collections::VecDeque::new(),
        main_cancelled_resync_requests: std::collections::VecDeque::new(),
        main_resync_incompatible: false,
        main_deferred_commands: std::collections::VecDeque::new(),
        outbound_queue: std::collections::VecDeque::new(),
        outbound_claims: None,
        pending_revokes: std::collections::VecDeque::new(),
        main_resync_needed_seq: 0,
        main_resync_attempts: 0,
        main_resync_semantic_drift: false,
        main_resync_snapshot_confirmed: false,
        main_last_snapshot_rev: 0,
        main_pending_pointer_cancels: std::collections::VecDeque::new(),
        next_resync_seq: 0,
        main_processed_ui_seq: 0,
        pending_panel_syncs: std::collections::BTreeMap::new(),
        defer_panel_sync_for_publish: false,
        child_headroom_blocked: false,
        urgent_commands: None,
        panel_destroy_control_seq: HashMap::new(),
        panel_destroy_seq_floor: 0,
        resource_revoke_control_seq: HashMap::new(),
        resource_revoke_seq_floor: 0,
        cancelled_op_control_seq: HashMap::new(),
        cancelled_op_seq_floor: 0,
        panel_resource_open_seq: HashMap::new(),
        panel_revoke_snapshots: HashMap::new(),
        panel_revoke_snapshot_floor: HashMap::new(),
        service_cancels: HashMap::new(),
        revoked_resources: HashMap::new(),
        panel_revoked_resources: HashMap::new(),
        service_tasks: JoinSet::new(),
    };
    let task = tokio::spawn(runtime.run(
        WorkerTransport::Duplex { stdin, stdout },
        commands,
        urgent_commands,
    ));
    Ok((
        PluginSupervisor {
            identity,
            manifest,
            initial_panel,
            supports_panel_multiplexing,
            events: event_receiver,
            handle,
            task: Some(task),
            terminal_error: None,
        },
        service_receiver,
    ))
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
        let (
            transport,
            panel_state,
            incoming_sequence,
            supports_panel_multiplexing,
            protocol_minor,
        ) = {
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
            let handshake = handshake_native(
                &mut stdin,
                &mut stdout,
                identity,
                &package.manifest,
                &trust.grants,
            );
            match timeout(MAX_OPERATION, handshake).await {
                Ok(Ok((state, seq, multiplexing, minor))) => (
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
                    multiplexing,
                    minor,
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
        let (urgent_control, urgent_commands) = mpsc::unbounded_channel();
        let (events, event_receiver) = mpsc::channel(QUEUE_CAPACITY);
        let cancel = CancellationToken::new();
        let handle = PluginHandle {
            identity,
            control,
            urgent_control,
            control_order: Arc::new(std::sync::Mutex::new(ControlOrderState::default())),
            cancel: cancel.clone(),
        };
        let manifest = package.manifest;
        let runtime = Running {
            identity,
            plugin_id: manifest.id.clone(),
            protocol_minor,
            supports_panel_multiplexing,
            panel: panel_state,
            panels: HashMap::new(),
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
            cancelled_requests: std::collections::VecDeque::new(),
            cancel_notifications: std::collections::VecDeque::new(),
            v14_degraded_resync_ids: std::collections::VecDeque::new(),
            main_cancelled_state_requests: std::collections::VecDeque::new(),
            main_cancelled_resync_requests: std::collections::VecDeque::new(),
            main_resync_incompatible: false,
            main_deferred_commands: std::collections::VecDeque::new(),
            outbound_queue: std::collections::VecDeque::new(),
            outbound_claims: None,
            pending_revokes: std::collections::VecDeque::new(),
            main_resync_needed_seq: 0,
            main_resync_attempts: 0,
            main_resync_semantic_drift: false,
            main_resync_snapshot_confirmed: false,
            main_last_snapshot_rev: 0,
            main_pending_pointer_cancels: std::collections::VecDeque::new(),
            next_resync_seq: 0,
            main_processed_ui_seq: 0,
            pending_panel_syncs: std::collections::BTreeMap::new(),
            defer_panel_sync_for_publish: false,
            child_headroom_blocked: false,
            urgent_commands: None,
            panel_destroy_control_seq: HashMap::new(),
            panel_destroy_seq_floor: 0,
            resource_revoke_control_seq: HashMap::new(),
            resource_revoke_seq_floor: 0,
            cancelled_op_control_seq: HashMap::new(),
            cancelled_op_seq_floor: 0,
            panel_resource_open_seq: HashMap::new(),
            panel_revoke_snapshots: HashMap::new(),
            panel_revoke_snapshot_floor: HashMap::new(),
            service_cancels: HashMap::new(),
            revoked_resources: HashMap::new(),
            panel_revoked_resources: HashMap::new(),
            service_tasks: JoinSet::new(),
        };
        let task = tokio::spawn(runtime.run(transport, commands, urgent_commands));
        Ok(Self {
            identity,
            manifest,
            initial_panel,
            supports_panel_multiplexing,
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
