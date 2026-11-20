use super::*;

impl Lab {
    pub(crate) fn button(&self, id: u64, label: &str, enabled: bool, commit_editors: bool) -> Node {
        Node::new(
            id,
            label,
            node::Kind::Button(Button {
                label: label.into(),
                enabled,
                commit_editors,
            }),
        )
    }

    pub(crate) fn snapshot(&self) -> PanelSnapshot {
        let has_request = self.context.is_some();
        let sending = self.sending;
        let toolbar = Node::new(
            2,
            "操作",
            node::Kind::Row(Container {
                children: vec![
                    self.button(DECODE, "Base64 解码", !self.decoded.read_only, true),
                    self.button(ENCODE, "Base64 编码", !self.raw.read_only, true),
                    self.button(
                        SEND,
                        "Send",
                        has_request && !sending && self.grants.contains(&Permission::NetworkSend),
                        true,
                    ),
                    self.button(
                        OPEN_REPEATER,
                        "发送到 Repeater",
                        has_request && self.grants.contains(&Permission::RepeaterOpen),
                        true,
                    ),
                    self.button(
                        WRITE_BACK,
                        "回写草稿",
                        self.context
                            .as_ref()
                            .is_some_and(|c| c.is_draft && !c.read_only)
                            && self.grants.contains(&Permission::DraftWrite),
                        true,
                    ),
                    self.button(CANCEL, "取消发送", sending, false),
                    self.button(
                        REFRESH,
                        "刷新流量",
                        self.grants.contains(&Permission::TrafficRead),
                        false,
                    ),
                    self.button(
                        TOGGLE_HOOKS,
                        if self.auto_tag_hooks {
                            "流量 Hook: 开"
                        } else {
                            "流量 Hook: 关"
                        },
                        self.grants.contains(&Permission::TrafficModify)
                            || self.grants.contains(&Permission::SessionHook),
                        false,
                    ),
                    self.button(
                        OPEN_INTRUDER,
                        "送到 Intruder",
                        has_request && self.grants.contains(&Permission::RepeaterOpen),
                        true,
                    ),
                    self.button(
                        SAVE_KV,
                        "存入 KV",
                        self.grants.contains(&Permission::Storage),
                        true,
                    ),
                    self.button(
                        LOAD_KV,
                        "读取 KV",
                        !self.decoded.read_only && self.grants.contains(&Permission::Storage),
                        false,
                    ),
                    self.button(
                        SAVE_PREF,
                        "存入偏好",
                        self.grants.contains(&Permission::Storage),
                        true,
                    ),
                    self.button(
                        LOAD_PREF,
                        "读取偏好",
                        !self.decoded.read_only && self.grants.contains(&Permission::Storage),
                        false,
                    ),
                    Node::new(
                        AUTO_HOOK_CHECKBOX,
                        "自动标记流量 Hook",
                        node::Kind::Checkbox(CheckboxNode {
                            label: "自动标记 Hook".into(),
                            checked: self.auto_tag_hooks,
                            enabled: self.grants.contains(&Permission::TrafficModify)
                                || self.grants.contains(&Permission::SessionHook),
                        }),
                    ),
                    Node::new(
                        CODEC_SELECT,
                        "Base64 变体选择",
                        node::Kind::Select(SelectNode {
                            label: "编码".into(),
                            options: vec!["Standard Base64".into(), "URL-Safe Base64".into()],
                            selected_index: u32::from(self.url_safe_base64),
                            enabled: true,
                        }),
                    ),
                ],
            }),
        );
        let status_row = Node::new(
            STATUS_ROW,
            "状态栏",
            node::Kind::Row(Container {
                children: vec![
                    Node::new(
                        STATUS_BADGE,
                        "运行状态徽章",
                        node::Kind::Badge(BadgeNode {
                            text: if self.error {
                                "ERROR".into()
                            } else if sending {
                                "SENDING".into()
                            } else if has_request {
                                "BOUND".into()
                            } else {
                                "IDLE".into()
                            },
                            tone: if self.error {
                                Tone::Error
                            } else if sending || has_request {
                                Tone::Accent
                            } else {
                                Tone::Muted
                            } as i32,
                        }),
                    ),
                    Node::new(
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
                    ),
                    Node::new(
                        STATUS_PROGRESS,
                        "发送进度",
                        node::Kind::Progress(ProgressNode {
                            label: if sending {
                                "发送中".into()
                            } else {
                                "就绪".into()
                            },
                            percent: if sending { 50 } else { 100 },
                        }),
                    ),
                ],
            }),
        );
        let table = Node::new(
            TABLE_NODE,
            "项目流量",
            node::Kind::VirtualTable(VirtualTable {
                columns: vec![
                    TableColumn {
                        id: "method".into(),
                        title: "Method".into(),
                        width: 72.0,
                    },
                    TableColumn {
                        id: "url".into(),
                        title: "URL".into(),
                        width: 260.0,
                    },
                    TableColumn {
                        id: "status".into(),
                        title: "Status".into(),
                        width: 64.0,
                    },
                ],
                total_rows: self.history.total,
                offset: self.history.offset,
                viewport_revision: self.viewport_revision,
                rows: self
                    .history
                    .rows
                    .iter()
                    .map(|r| TableRow {
                        id: protocol::uuid(&r.id)
                            .map(|id| id.to_string())
                            .unwrap_or_default(),
                        cells: vec![
                            r.method.clone(),
                            r.url.clone(),
                            r.status.map_or_else(String::new, |s| s.to_string()),
                        ],
                    })
                    .collect(),
                multi_select: false,
            }),
        );
        let mut tabs = vec![
            Tab {
                title: "Raw".into(),
                content: Some(Box::new(Node::new(
                    RAW_NODE,
                    "Raw Base64 正文",
                    node::Kind::Editor(self.raw.clone()),
                ))),
            },
            Tab {
                title: "Decoded".into(),
                content: Some(Box::new(Node::new(
                    DECODED_NODE,
                    "Decoded 正文",
                    node::Kind::Editor(self.decoded.clone()),
                ))),
            },
        ];
        if let Some(response) = &self.response {
            tabs.push(Tab {
                title: "Response".into(),
                content: Some(Box::new(Node::new(
                    RESPONSE_NODE,
                    "实际响应正文",
                    node::Kind::Editor(response.clone()),
                ))),
            });
        }
        let editors = Node::new(
            TABS_NODE,
            "消息编辑器",
            node::Kind::Tabs(Tabs {
                axis: Axis::Horizontal as i32,
                active: if self.active_document == DECODED_DOCUMENT {
                    DECODED_NODE
                } else if self.active_document == RESPONSE_DOCUMENT && self.response.is_some() {
                    RESPONSE_NODE
                } else {
                    RAW_NODE
                },
                tabs,
            }),
        );
        let workspace = Node::new(
            3,
            "协议工作区",
            node::Kind::Split(Split {
                axis: Axis::Horizontal as i32,
                fraction: self.split_fraction,
                first: Some(Box::new(table)),
                second: Some(Box::new(editors)),
            }),
        );
        PanelSnapshot {
            revision: self.revision,
            title: "协议实验室".into(),
            root: Some(Node::new(
                1,
                "协议实验室",
                node::Kind::Column(Container {
                    children: vec![
                        toolbar,
                        status_row,
                        workspace,
                        Node::new(
                            CANVAS_NODE,
                            "可拖拽请求时间线",
                            node::Kind::Canvas(self.timeline()),
                        ),
                    ],
                }),
            )),
        }
    }

    pub(crate) fn timeline(&self) -> Canvas {
        let mut canvas = Canvas {
            width: 900.0,
            height: 80.0 + self.history.rows.len() as f32 * 24.0,
            commands: vec![],
            hit_regions: vec![],
        };
        if self.history.rows.is_empty() {
            canvas.commands.push(DrawCommand {
                op: Some(draw_command::Op::Text(DrawText {
                    origin: Some(Point { x: 12.0, y: 20.0 }),
                    text: "当前页没有流量。".into(),
                    tone: Tone::Muted as i32,
                    size: 13.0,
                })),
            });
        }
        for (index, row) in self.history.rows.iter().enumerate() {
            let id = protocol::uuid(&row.id)
                .map(|id| id.to_string())
                .unwrap_or_default();
            let mut position = self.positions.get(&id).cloned().unwrap_or(Point {
                x: 12.0,
                y: 20.0 + index as f32 * 24.0,
            });
            position.y = position.y.clamp(0.0, canvas.height - 24.0);
            let bounds = Rect {
                x: position.x,
                y: position.y,
                width: (row.elapsed_ms as f32).clamp(80.0, 300.0),
                height: 20.0,
            };
            canvas.commands.push(DrawCommand {
                op: Some(draw_command::Op::Rect(DrawRect {
                    bounds: Some(bounds.clone()),
                    tone: Tone::Selection as i32,
                    radius: 3.0,
                })),
            });
            canvas.commands.push(DrawCommand {
                op: Some(draw_command::Op::Text(DrawText {
                    origin: Some(Point {
                        x: bounds.x + 4.0,
                        y: bounds.y + 2.0,
                    }),
                    text: format!("{} · {} ms", row.method, row.elapsed_ms),
                    tone: Tone::Foreground as i32,
                    size: 12.0,
                })),
            });
            canvas.hit_regions.push(HitRegion {
                id: self.hit_ids[&id],
                bounds: Some(bounds),
                accessibility_label: format!(
                    "{} {}，{} 毫秒；可拖动",
                    row.method, row.url, row.elapsed_ms
                ),
                focusable: true,
                capture_pointer: true,
            });
        }
        canvas
    }
}
