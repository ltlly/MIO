use mio_plugin_sdk::ui;

use super::*;

impl Lab {
    pub(crate) fn snapshot(&self) -> PanelSnapshot {
        let has_request = self.context.is_some();
        let sending = self.sending;
        let toolbar = ui::row(
            2,
            "操作",
            vec![
                ui::button(DECODE, "Base64 解码", !self.decoded.read_only, true),
                ui::button(ENCODE, "Base64 编码", !self.raw.read_only, true),
                ui::button(
                    SEND,
                    "Send",
                    has_request && !sending && self.grants.contains(&Permission::NetworkSend),
                    true,
                ),
                ui::button(
                    OPEN_REPEATER,
                    "发送到 Repeater",
                    has_request && self.grants.contains(&Permission::RepeaterOpen),
                    true,
                ),
                ui::button(
                    WRITE_BACK,
                    "回写草稿",
                    self.context
                        .as_ref()
                        .is_some_and(|c| c.is_draft && !c.read_only)
                        && self.grants.contains(&Permission::DraftWrite),
                    true,
                ),
                ui::button(CANCEL, "取消发送", sending, false),
                ui::button(
                    REFRESH,
                    "刷新流量",
                    self.grants.contains(&Permission::TrafficRead),
                    false,
                ),
                ui::button(
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
                ui::button(
                    OPEN_INTRUDER,
                    "送到 Intruder",
                    has_request && self.grants.contains(&Permission::RepeaterOpen),
                    true,
                ),
                ui::button(
                    SAVE_KV,
                    "存入 KV",
                    self.grants.contains(&Permission::Storage),
                    true,
                ),
                ui::button(
                    LOAD_KV,
                    "读取 KV",
                    !self.decoded.read_only && self.grants.contains(&Permission::Storage),
                    false,
                ),
                ui::button(
                    SAVE_PREF,
                    "存入偏好",
                    self.grants.contains(&Permission::Storage),
                    true,
                ),
                ui::button(
                    LOAD_PREF,
                    "读取偏好",
                    !self.decoded.read_only && self.grants.contains(&Permission::Storage),
                    false,
                ),
                ui::checkbox(
                    AUTO_HOOK_CHECKBOX,
                    "自动标记 Hook",
                    self.auto_tag_hooks,
                    self.grants.contains(&Permission::TrafficModify)
                        || self.grants.contains(&Permission::SessionHook),
                ),
                ui::select(
                    CODEC_SELECT,
                    "编码",
                    vec!["Standard Base64".into(), "URL-Safe Base64".into()],
                    u32::from(self.url_safe_base64),
                    true,
                ),
            ],
            0.0,
        );
        let status_row = ui::row(
            STATUS_ROW,
            "状态栏",
            vec![
                ui::badge(
                    STATUS_BADGE,
                    if self.error {
                        "ERROR"
                    } else if sending {
                        "SENDING"
                    } else if has_request {
                        "BOUND"
                    } else {
                        "IDLE"
                    },
                    if self.error {
                        Tone::Error
                    } else if sending || has_request {
                        Tone::Accent
                    } else {
                        Tone::Muted
                    },
                ),
                ui::text(
                    4,
                    "任务状态",
                    &self.status,
                    if self.error {
                        Tone::Error
                    } else {
                        Tone::Foreground
                    },
                ),
                ui::progress(
                    STATUS_PROGRESS,
                    if sending { "发送中" } else { "就绪" },
                    if sending { 50 } else { 100 },
                ),
            ],
            0.0,
        );
        let table = ui::virtual_table(
            TABLE_NODE,
            "项目流量",
            vec![
                ui::table_column("method", "Method", 72.0),
                ui::table_column("url", "URL", 260.0),
                ui::table_column("status", "Status", 64.0),
            ],
            self.history.total,
            self.history.offset,
            self.viewport_revision,
            self.history
                .rows
                .iter()
                .map(|r| {
                    ui::table_row(
                        protocol::uuid(&r.id)
                            .map(|id| id.to_string())
                            .unwrap_or_default(),
                        vec![
                            r.method.clone(),
                            r.url.clone(),
                            r.status.map_or_else(String::new, |s| s.to_string()),
                        ],
                    )
                })
                .collect(),
            false,
        );
        let editor_format = |ed: &Editor| {
            EditorFormat::try_from(ed.format).unwrap_or(EditorFormat::Utf8)
        };
        let mut editor_tabs = vec![
            ui::tab(
                "Raw",
                ui::editor(
                    RAW_NODE,
                    "Raw Base64 正文",
                    self.raw.document_id,
                    self.raw.revision,
                    self.raw.contents.clone(),
                    editor_format(&self.raw),
                    self.raw.read_only,
                ),
            ),
            ui::tab(
                "Decoded",
                ui::editor(
                    DECODED_NODE,
                    "Decoded 正文",
                    self.decoded.document_id,
                    self.decoded.revision,
                    self.decoded.contents.clone(),
                    editor_format(&self.decoded),
                    self.decoded.read_only,
                ),
            ),
        ];
        if let Some(response) = &self.response {
            editor_tabs.push(ui::tab(
                "Response",
                ui::editor(
                    RESPONSE_NODE,
                    "实际响应正文",
                    response.document_id,
                    response.revision,
                    response.contents.clone(),
                    editor_format(response),
                    response.read_only,
                ),
            ));
        }
        let active_tab = if self.active_document == DECODED_DOCUMENT {
            DECODED_NODE
        } else if self.active_document == RESPONSE_DOCUMENT && self.response.is_some() {
            RESPONSE_NODE
        } else {
            RAW_NODE
        };
        let editors = ui::tabs(
            TABS_NODE,
            "消息编辑器",
            Axis::Horizontal,
            active_tab,
            editor_tabs,
        );
        let workspace = ui::split(
            3,
            "协议工作区",
            Axis::Horizontal,
            self.split_fraction,
            table,
            editors,
        );
        let timeline = self.timeline();
        let canvas_node = ui::canvas(
            CANVAS_NODE,
            "可拖拽请求时间线",
            timeline.width,
            timeline.height,
            timeline.commands,
            timeline.hit_regions,
        );
        ui::snapshot(
            self.revision,
            "协议实验室",
            ui::column(
                1,
                "协议实验室",
                vec![toolbar, status_row, workspace, canvas_node],
                0.0,
            ),
        )
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
