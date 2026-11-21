use super::*;

impl Lab {
    pub(crate) fn publish<R: io::Read, W: io::Write>(
        &mut self,
        session: &mut WorkerSession<R, W>,
        reply: u64,
    ) -> io::Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("面板 revision 耗尽"))?;
        let panel = self.snapshot();
        panel.validate().map_err(io::Error::other)?;
        session.send_on_panel(
            self.panel_id,
            envelope::Body::Snapshot(panel),
            self.revision,
            reply,
        )?;
        Ok(())
    }

    pub(crate) fn call<R: io::Read, W: io::Write>(
        &mut self,
        session: &mut WorkerSession<R, W>,
        operation: host_call::Operation,
        pending: PendingService,
    ) -> io::Result<()> {
        let id = session.send_on_panel(
            self.panel_id,
            envelope::Body::HostCall(HostCall {
                operation: Some(operation),
                parent_request_id: 0,
            }),
            self.revision,
            0,
        )?;
        self.pending.insert(id, pending);
        Ok(())
    }

    pub(crate) fn publish_canvas<R: io::Read, W: io::Write>(
        &mut self,
        session: &mut WorkerSession<R, W>,
        reply: u64,
    ) -> io::Result<()> {
        let base_revision = self.revision;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("面板 revision 耗尽"))?;
        let patch = PanelPatch {
            base_revision,
            revision: self.revision,
            operations: vec![
                PatchOperation {
                    node_id: CANVAS_NODE,
                    change: Some(patch_operation::Change::Canvas(self.timeline())),
                },
                PatchOperation {
                    node_id: 4,
                    change: Some(patch_operation::Change::Replace(Node::new(
                        4,
                        "任务状态",
                        node::Kind::Text(Text {
                            text: self.status.clone(),
                            tone: if self.error {
                                Tone::Error
                            } else {
                                Tone::Foreground
                            } as i32,
                        }),
                    ))),
                },
            ],
        };
        session.send_on_panel(
            self.panel_id,
            envelope::Body::Patch(patch),
            self.revision,
            reply,
        )?;
        Ok(())
    }

    pub(crate) fn query<R: io::Read, W: io::Write>(
        &mut self,
        session: &mut WorkerSession<R, W>,
        offset: u64,
        limit: u32,
    ) -> io::Result<()> {
        self.call(
            session,
            host_call::Operation::QueryHistory(HistoryQuery {
                offset,
                limit,
                filter: self.filter.clone(),
            }),
            PendingService::History {
                viewport_revision: self.viewport_revision,
            },
        )
    }

    pub(crate) fn click<R: io::Read, W: io::Write>(
        &mut self,
        session: &mut WorkerSession<R, W>,
        event: &UiEvent,
    ) -> Result<Option<(host_call::Operation, PendingService)>, String> {
        let changed: Vec<_> = event
            .editors
            .iter()
            .filter_map(|snapshot| {
                let current = match snapshot.document_id {
                    RAW_DOCUMENT => self.raw.revision,
                    DECODED_DOCUMENT => self.decoded.revision,
                    _ => return None,
                };
                (snapshot.revision > current).then_some(snapshot.document_id)
            })
            .collect();
        if changed.len() == 1 {
            self.active_document = changed[0];
        }
        if changed.len() > 1 && !changed.contains(&self.active_document) {
            return Err("多个编辑器同时改变，请先选择要使用的编辑器".into());
        }
        for snapshot in &event.editors {
            self.sync_editor(snapshot)?;
        }
        match event.node_id {
            DECODE => {
                let decoded = self
                    .decode_b64(&self.raw.contents)
                    .map_err(|_| "Raw 不是有效 Base64；未覆盖 Decoded")?;
                Self::set_editor(&mut self.decoded, decoded)?;
                self.active_document = DECODED_DOCUMENT;
                self.status = "已解码到 Decoded；尚未发送或回写。".into();
            }
            ENCODE => {
                let encoded = self.encode_b64(&self.decoded.contents).into_bytes();
                Self::set_editor(&mut self.raw, encoded)?;
                self.active_document = RAW_DOCUMENT;
                self.status = "已编码并回写 Raw 编辑器；尚未发送。".into();
            }
            SEND | OPEN_REPEATER | OPEN_INTRUDER | WRITE_BACK => {
                if event.node_id == SEND && self.sending {
                    return Err("已有在途请求；不会重复发送".into());
                }
                let request = self.request(self.active_document)?;
                if event.node_id == SEND {
                    self.sending = true;
                }
                self.status = if event.node_id == SEND {
                    "正在发送实际请求…"
                } else if event.node_id == WRITE_BACK {
                    "正在回写草稿…"
                } else if event.node_id == OPEN_INTRUDER {
                    "正在发送到 Intruder…"
                } else {
                    "正在打开 Repeater；不会自动发送…"
                }
                .into();
                return Ok(Some(match event.node_id {
                    SEND => (
                        host_call::Operation::NetworkSend(request.into()),
                        PendingService::Send {
                            context_generation: self.context_generation,
                        },
                    ),
                    OPEN_REPEATER => (
                        host_call::Operation::OpenRepeater(request.into()),
                        PendingService::OpenRepeater,
                    ),
                    OPEN_INTRUDER => (
                        host_call::Operation::OpenIntruder(request.into()),
                        PendingService::OpenIntruder,
                    ),
                    _ => {
                        let context = self.context.as_ref().ok_or("尚无草稿上下文")?;
                        if !context.is_draft || context.read_only {
                            return Err("当前上下文不是可写草稿".into());
                        }
                        (
                            host_call::Operation::UpdateDraft(DraftUpdate {
                                resource_id: context.resource.as_bytes().to_vec(),
                                expected_revision: context.revision,
                                request: Some(request.clone().into()),
                            }),
                            PendingService::WriteBack {
                                request,
                                context_generation: self.context_generation,
                            },
                        )
                    }
                }));
            }
            TOGGLE_HOOKS => {
                self.auto_tag_hooks = !self.auto_tag_hooks;
                self.status = if self.auto_tag_hooks {
                    "已开启自动流量与会话 Hook 标记。".into()
                } else {
                    "已关闭自动流量与会话 Hook 标记。".into()
                };
            }
            SAVE_KV => {
                let limit = self.raw.contents.len().min(4096);
                let value = self.raw.contents[..limit].to_vec();
                self.status = "正在将 Raw 内容存入插件 KV…".into();
                return Ok(Some((
                    host_call::Operation::StorageSet(StorageEntry {
                        key: "scratch".into(),
                        value,
                    }),
                    PendingService::SaveKv,
                )));
            }
            LOAD_KV => {
                if self.decoded.read_only {
                    return Err("只读编辑器拒绝读取 KV 覆盖".into());
                }
                self.status = "正在从插件 KV 读取内容…".into();
                return Ok(Some((
                    host_call::Operation::StorageGet("scratch".into()),
                    PendingService::LoadKv,
                )));
            }
            SAVE_PREF => {
                let limit = self.raw.contents.len().min(4096);
                let value = self.raw.contents[..limit].to_vec();
                self.status = "正在将 Raw 内容存入跨项目偏好…".into();
                return Ok(Some((
                    host_call::Operation::PreferencesSet(StorageEntry {
                        key: "scratch_pref".into(),
                        value,
                    }),
                    PendingService::SavePref,
                )));
            }
            LOAD_PREF => {
                if self.decoded.read_only {
                    return Err("只读编辑器拒绝读取偏好覆盖".into());
                }
                self.status = "正在从跨项目偏好读取内容…".into();
                return Ok(Some((
                    host_call::Operation::PreferencesGet("scratch_pref".into()),
                    PendingService::LoadPref,
                )));
            }
            CANCEL => {
                let ids: Vec<_> = self
                    .pending
                    .iter()
                    .filter_map(|(id, p)| matches!(p, PendingService::Send { .. }).then_some(*id))
                    .collect();
                if ids.is_empty() {
                    return Err("没有在途发送任务".into());
                }
                for id in ids {
                    session
                        .send_on_panel(
                            self.panel_id,
                            envelope::Body::Cancel(Cancel { request_id: id }),
                            self.revision,
                            0,
                        )
                        .map_err(|e| e.to_string())?;
                }
                self.status = "已请求取消；等待宿主回收网络任务。".into();
            }
            _ => return Err("未知操作按钮".into()),
        }
        Ok(None)
    }

    pub(crate) fn event<R: io::Read, W: io::Write>(
        &mut self,
        session: &mut WorkerSession<R, W>,
        event: UiEvent,
        reply: u64,
    ) -> io::Result<()> {
        let mut service = None;
        let mut query = None;
        let result: Result<(), String> = (|| {
            match event.event.as_ref().ok_or("缺少 UI 事件")? {
                ui_event::Event::Click(_) if event.node_id == REFRESH => {
                    self.viewport_revision = self
                        .viewport_revision
                        .checked_add(1)
                        .ok_or("视口 revision 耗尽")?;
                    query = Some((self.history.offset, 64));
                    self.status = "正在刷新实际项目流量…".into();
                }
                ui_event::Event::Click(_) => service = self.click(session, &event)?,
                ui_event::Event::EditorChanged(editor) => {
                    self.sync_editor(editor)?;
                    self.active_document = editor.document_id;
                    self.status = "编辑内容尚未发送或回写。".into();
                }
                ui_event::Event::Focus(true) => {
                    self.active_document = match event.node_id {
                        RAW_NODE => RAW_DOCUMENT,
                        DECODED_NODE => DECODED_DOCUMENT,
                        RESPONSE_NODE => RESPONSE_DOCUMENT,
                        _ => self.active_document,
                    };
                }
                ui_event::Event::TabSelected(id) => {
                    self.active_document = match *id {
                        RAW_NODE => RAW_DOCUMENT,
                        DECODED_NODE => DECODED_DOCUMENT,
                        RESPONSE_NODE => RESPONSE_DOCUMENT,
                        _ => return Err("未知编辑 tab".into()),
                    }
                }
                ui_event::Event::Viewport(viewport) => {
                    self.viewport_revision = viewport.revision;
                    query = Some((viewport.offset, viewport.limit));
                    self.status = "正在读取项目流量页…".into();
                }
                ui_event::Event::Filter(filter) => {
                    self.filter = filter.clone();
                    self.viewport_revision += 1;
                    query = Some((0, 64));
                    self.status = "正在筛选实际流量…".into();
                }
                ui_event::Event::TableSelection(selection) => {
                    self.selection_revision = self
                        .selection_revision
                        .checked_add(1)
                        .ok_or("选择 revision 耗尽")?;
                    if let Some(id) = selection.row_ids.first() {
                        let id = Uuid::parse_str(id).map_err(|_| "流量 ID 无效")?;
                        service = Some((
                            host_call::Operation::ReadFlow(id.as_bytes().to_vec()),
                            PendingService::Detail {
                                selection: self.selection_revision,
                            },
                        ));
                        self.status = "正在读取选中请求…".into();
                    }
                }
                ui_event::Event::Pointer(pointer) => {
                    let id = self
                        .hit_ids
                        .iter()
                        .find_map(|(id, hit)| (*hit == pointer.hit_id).then_some(id.clone()))
                        .ok_or("时间线命中区域已过期")?;
                    let index = self
                        .history
                        .rows
                        .iter()
                        .position(|r| protocol::uuid(&r.id).is_ok_and(|row| row.to_string() == id))
                        .ok_or("时间线流量已改变")?;
                    match PointerPhase::try_from(pointer.phase).map_err(|_| "未知指针阶段")? {
                        PointerPhase::Down => {
                            let position = self.positions.get(&id).cloned().unwrap_or(Point {
                                x: 12.0,
                                y: 20.0 + index as f32 * 24.0,
                            });
                            self.dragging = Some((
                                pointer.hit_id,
                                Point {
                                    x: pointer.x - position.x,
                                    y: pointer.y - position.y,
                                },
                            ));
                        }
                        PointerPhase::Move => {
                            if let Some((hit, offset)) = &self.dragging
                                && *hit == pointer.hit_id
                            {
                                self.positions.insert(
                                    id,
                                    Point {
                                        x: (pointer.x - offset.x).clamp(0.0, 590.0),
                                        y: (pointer.y - offset.y)
                                            .clamp(0.0, self.timeline().height - 24.0),
                                    },
                                );
                            }
                        }
                        PointerPhase::Up | PointerPhase::Cancel => {
                            self.dragging = None;
                            self.status = "已调整当前页的时间线布局；没有改写流量。".into();
                        }
                    }
                }
                ui_event::Event::Key(key) if key.key == "escape" => {
                    self.dragging = None;
                }
                ui_event::Event::SplitResized(fraction) => self.split_fraction = *fraction,
                ui_event::Event::Toggled(checked) if event.node_id == AUTO_HOOK_CHECKBOX => {
                    self.auto_tag_hooks = *checked;
                    self.status = if *checked {
                        "已开启自动流量与会话 Hook 标记。".into()
                    } else {
                        "已关闭自动流量与会话 Hook 标记。".into()
                    };
                }
                ui_event::Event::SelectedOption(index) if event.node_id == CODEC_SELECT => {
                    self.url_safe_base64 = *index == 1;
                    self.status = if self.url_safe_base64 {
                        "已切换编码变体：URL-Safe Base64。".into()
                    } else {
                        "已切换编码变体：Standard Base64。".into()
                    };
                }
                ui_event::Event::Toggled(_) | ui_event::Event::SelectedOption(_) => {
                    return Err("未知交互控件".into());
                }
                ui_event::Event::Focus(false) | ui_event::Event::Key(_) => {}
            }
            Ok(())
        })();
        self.error = result.is_err();
        if let Err(error) = result {
            self.status = error;
        }
        if !self.error
            && matches!(
                event.event,
                Some(
                    ui_event::Event::Focus(_)
                        | ui_event::Event::Key(_)
                        | ui_event::Event::SplitResized(_)
                        | ui_event::Event::TabSelected(_)
                )
            )
        {
            session.send_on_panel(
                self.panel_id,
                envelope::Body::Ack(Empty {}),
                self.revision,
                reply,
            )?;
        } else if matches!(event.event, Some(ui_event::Event::Pointer(_))) {
            self.publish_canvas(session, reply)?;
        } else {
            self.publish(session, reply)?;
        }
        if let Some((operation, pending)) = service {
            self.call(session, operation, pending)?;
        }
        if let Some((offset, limit)) = query {
            self.query(session, offset, limit)?;
        }
        Ok(())
    }
}
