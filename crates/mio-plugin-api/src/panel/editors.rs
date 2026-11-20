use std::collections::HashSet;

use super::{validators::*, *};

/// Apply only on the service thread. A failed patch leaves the last valid view intact.
#[derive(Clone, Debug)]
pub struct PanelState {
    snapshot: PanelSnapshot,
    captured: Option<(u64, u64)>,
}

impl PanelState {
    pub fn new(snapshot: PanelSnapshot) -> Result<Self, String> {
        snapshot.validate()?;
        Ok(Self {
            snapshot,
            captured: None,
        })
    }

    pub fn snapshot(&self) -> &PanelSnapshot {
        &self.snapshot
    }

    pub fn node(&self, id: u64) -> Option<&Node> {
        self.snapshot.root.as_ref()?.find(id)
    }

    pub fn editors(&self) -> Vec<(u64, &Editor)> {
        let mut editors = Vec::new();
        if let Some(root) = &self.snapshot.root {
            root.editors(&mut editors);
        }
        editors
    }

    pub fn editor(&self, document_id: u64) -> Option<&Editor> {
        self.editors()
            .into_iter()
            .find_map(|(_, e)| (e.document_id == document_id).then_some(e))
    }

    fn preserve_native_revisions(&self, next: &PanelSnapshot) -> Result<(), String> {
        let mut next_editors = Vec::new();
        if let Some(root) = &next.root {
            root.editors(&mut next_editors);
        }
        for (_, previous) in self.editors() {
            if let Some((_, next)) = next_editors
                .iter()
                .find(|(_, e)| e.document_id == previous.document_id)
            {
                check(
                    next.revision >= previous.revision
                        && (next.revision != previous.revision
                            || next.contents == previous.contents),
                    "插件结果会覆盖较新的编辑内容；需要重新同步",
                )?;
            }
        }
        if let (Some(previous), Some(next)) = (&self.snapshot.root, &next.root) {
            previous.preserve_table_revisions(next)?;
        }
        Ok(())
    }

    pub fn replace(&mut self, next: PanelSnapshot) -> Result<(), String> {
        check(
            next.revision > self.snapshot.revision,
            "Panel snapshot 已过期",
        )?;
        next.validate()?;
        self.preserve_native_revisions(&next)?;
        self.snapshot = next;
        if let Some((node, hit)) = self.captured
            && !self.node(node).is_some_and(|n| matches!(&n.kind, Some(node::Kind::Canvas(c)) if c.hit_regions.iter().any(|h| h.id == hit)))
        {
            self.captured = None;
        }
        Ok(())
    }

    pub fn apply(&mut self, patch: &PanelPatch) -> Result<(), String> {
        check(
            patch.base_revision == self.snapshot.revision && patch.revision > patch.base_revision,
            "Panel patch revision 已过期；需要重新同步",
        )?;
        check(
            !patch.operations.is_empty() && patch.operations.len() <= MAX_NODES,
            "Panel patch 大小无效",
        )?;
        let mut next = self.snapshot.clone();
        for operation in &patch.operations {
            let node = next
                .root
                .as_mut()
                .and_then(|n| n.find_mut(operation.node_id))
                .ok_or("Panel patch 节点不存在")?;
            match operation.change.as_ref().ok_or("未知 Panel patch")? {
                patch_operation::Change::Replace(replacement) => {
                    check(
                        replacement.id == operation.node_id,
                        "替换节点必须保留稳定 ID",
                    )?;
                    *node = replacement.clone();
                }
                patch_operation::Change::Editor(update) => {
                    let Some(node::Kind::Editor(editor)) = &mut node.kind else {
                        return Err("目标不是 Editor".into());
                    };
                    let snapshot = update.snapshot.as_ref().ok_or("缺少 EditorSnapshot")?;
                    snapshot.validate()?;
                    check(
                        snapshot.document_id == editor.document_id
                            && update.base_revision == editor.revision
                            && snapshot.revision > editor.revision,
                        "编辑器 patch 身份或 revision 已过期",
                    )?;
                    editor.revision = snapshot.revision;
                    editor.contents.clone_from(&snapshot.contents);
                }
                patch_operation::Change::Table(table) => {
                    let Some(node::Kind::VirtualTable(previous)) = &node.kind else {
                        return Err("目标不是 VirtualTable".into());
                    };
                    check(
                        table.viewport_revision == previous.viewport_revision,
                        "表格 viewport revision 已过期；需要重新同步",
                    )?;
                    node.kind = Some(node::Kind::VirtualTable(table.clone()));
                }
                patch_operation::Change::Canvas(canvas) => {
                    check(
                        matches!(node.kind, Some(node::Kind::Canvas(_))),
                        "目标不是 Canvas",
                    )?;
                    node.kind = Some(node::Kind::Canvas(canvas.clone()));
                }
            }
        }
        next.revision = patch.revision;
        next.validate()?;
        self.preserve_native_revisions(&next)?;
        self.snapshot = next;
        if let Some((node, hit)) = self.captured
            && !self.node(node).is_some_and(|n| matches!(&n.kind, Some(node::Kind::Canvas(c)) if c.hit_regions.iter().any(|h| h.id == hit)))
        {
            self.captured = None;
        }
        Ok(())
    }

    /// Record authoritative native input before dispatch; read-only and stale editor events fail here.
    pub fn observe_event(&mut self, event: &UiEvent) -> Result<(), String> {
        let mut next = self.clone();
        let button = match next.node(event.node_id).and_then(|n| n.kind.as_ref()) {
            Some(node::Kind::Button(button))
                if matches!(event.event, Some(ui_event::Event::Click(_))) =>
            {
                Some(button)
            }
            _ => None,
        };
        if button.is_some_and(|button| button.commit_editors) {
            let editors = next.editors();
            check(
                event.editors.len() == editors.len(),
                "按钮激活缺少最新 EditorSnapshot",
            )?;
            let mut seen = HashSet::new();
            for snapshot in &event.editors {
                snapshot.validate()?;
                check(
                    seen.insert(snapshot.document_id),
                    "按钮激活含重复编辑器快照",
                )?;
                let (_, editor) = editors
                    .iter()
                    .find(|(_, e)| e.document_id == snapshot.document_id)
                    .ok_or("快照不属于此面板")?;
                check(snapshot.revision >= editor.revision, "按钮激活快照已过期")?;
                check(
                    snapshot.revision != editor.revision || snapshot.contents == editor.contents,
                    "相同 revision 的编辑内容不一致",
                )?;
                check(
                    !editor.read_only
                        || (snapshot.revision == editor.revision
                            && snapshot.contents == editor.contents),
                    "只读编辑器拒绝修改",
                )?;
            }
            drop(editors);
            for snapshot in &event.editors {
                let (node_id, editor) = next
                    .editors()
                    .into_iter()
                    .find(|(_, e)| e.document_id == snapshot.document_id)
                    .unwrap();
                if snapshot.revision > editor.revision {
                    next.observe_one(&UiEvent {
                        node_id,
                        event: Some(ui_event::Event::EditorChanged(snapshot.clone())),
                        editors: vec![],
                    })?;
                }
            }
        } else {
            check(event.editors.is_empty(), "此事件没有申请编辑器提交快照")?;
        }
        next.observe_one(event)?;
        *self = next;
        Ok(())
    }

    fn observe_one(&mut self, event: &UiEvent) -> Result<(), String> {
        let node = self.node(event.node_id).ok_or("UI 事件节点不存在")?;
        let kind = node.kind.as_ref().ok_or("UI 事件节点类型不存在")?;
        let mut capture = self.captured;
        match event.event.as_ref().ok_or("未知 UI 事件")? {
            ui_event::Event::Click(_) => check(
                matches!(kind, node::Kind::Button(b) if b.enabled),
                "按钮不可用",
            )?,
            ui_event::Event::EditorChanged(snapshot) => {
                let node::Kind::Editor(editor) = kind else {
                    return Err("目标不是 Editor".into());
                };
                snapshot.validate()?;
                check(!editor.read_only, "只读编辑器拒绝修改")?;
                check(
                    snapshot.document_id == editor.document_id
                        && snapshot.revision > editor.revision,
                    "编辑事件身份或 revision 已过期",
                )?;
                if editor.format == EditorFormat::Utf8 as i32 {
                    let text = std::str::from_utf8(&snapshot.contents)
                        .map_err(|_| "编辑内容不是 UTF-8")?;
                    check(
                        text.is_char_boundary(snapshot.selection_start as usize)
                            && text.is_char_boundary(snapshot.selection_end as usize),
                        "选区不是 UTF-8 字符边界",
                    )?;
                }
            }
            ui_event::Event::Viewport(v) => {
                let node::Kind::VirtualTable(t) = kind else {
                    return Err("目标不是 VirtualTable".into());
                };
                check(
                    v.revision > t.viewport_revision
                        && v.limit > 0
                        && v.limit as usize <= MAX_TABLE_ROWS
                        && v.offset <= t.total_rows,
                    "视口请求无效或过期",
                )?;
            }
            ui_event::Event::TableSelection(s) => {
                let node::Kind::VirtualTable(t) = kind else {
                    return Err("目标不是 VirtualTable".into());
                };
                check(
                    s.row_ids.len() <= MAX_TABLE_ROWS
                        && (t.multi_select || s.row_ids.len() <= 1)
                        && s.row_ids
                            .iter()
                            .all(|id| t.rows.iter().any(|r| &r.id == id)),
                    "表格选择不属于当前页",
                )?;
            }
            ui_event::Event::Pointer(p) => {
                let node::Kind::Canvas(c) = kind else {
                    return Err("目标不是 Canvas".into());
                };
                check(coordinate(p.x) && coordinate(p.y), "指针坐标无效")?;
                let h = c
                    .hit_regions
                    .iter()
                    .find(|h| h.id == p.hit_id)
                    .ok_or("Canvas 命中区域不存在")?;
                match PointerPhase::try_from(p.phase).map_err(|_| "未知指针阶段")? {
                    PointerPhase::Down => {
                        check(
                            h.bounds.as_ref().is_some_and(|r| r.contains(p.x, p.y)),
                            "指针未命中区域",
                        )?;
                        if h.capture_pointer {
                            capture = Some((event.node_id, p.hit_id));
                        }
                    }
                    PointerPhase::Move => check(
                        capture == Some((event.node_id, p.hit_id))
                            || h.bounds.as_ref().is_some_and(|r| r.contains(p.x, p.y)),
                        "指针没有 capture",
                    )?,
                    PointerPhase::Up | PointerPhase::Cancel => {
                        check(
                            capture == Some((event.node_id, p.hit_id))
                                || h.bounds.as_ref().is_some_and(|r| r.contains(p.x, p.y)),
                            "指针没有 capture",
                        )?;
                        capture = None;
                    }
                }
            }
            ui_event::Event::Key(k) => {
                label(&k.key, 64)?;
                check(k.modifiers <= 15, "未知快捷键修饰符")?;
            }
            ui_event::Event::Focus(_) => {}
            ui_event::Event::TabSelected(id) => check(
                matches!(kind, node::Kind::Tabs(t) if t.tabs.iter().any(|t| t.content.as_ref().is_some_and(|n| n.id == *id))),
                "目标 tab 不存在",
            )?,
            ui_event::Event::SplitResized(f) => check(
                matches!(kind, node::Kind::Split(_)) && f.is_finite() && (0.05..=0.95).contains(f),
                "Split 比例无效",
            )?,
            ui_event::Event::Filter(query) => check(
                matches!(kind, node::Kind::VirtualTable(_)) && query.len() <= 1024,
                "筛选条件无效",
            )?,
            ui_event::Event::Toggled(_) => check(
                matches!(kind, node::Kind::Checkbox(c) if c.enabled),
                "复选框不可用或节点类型不匹配",
            )?,
            ui_event::Event::SelectedOption(idx) => check(
                matches!(kind, node::Kind::Select(s) if s.enabled && (*idx as usize) < s.options.len()),
                "下拉选项不可用或索引越界",
            )?,
        }
        let node = self
            .snapshot
            .root
            .as_mut()
            .and_then(|n| n.find_mut(event.node_id))
            .unwrap();
        match (&event.event, &mut node.kind) {
            (Some(ui_event::Event::EditorChanged(s)), Some(node::Kind::Editor(e))) => {
                e.revision = s.revision;
                e.contents.clone_from(&s.contents);
            }
            (Some(ui_event::Event::Viewport(v)), Some(node::Kind::VirtualTable(t))) => {
                t.viewport_revision = v.revision;
            }
            (Some(ui_event::Event::TabSelected(id)), Some(node::Kind::Tabs(t))) => {
                t.active = *id;
            }
            (Some(ui_event::Event::SplitResized(f)), Some(node::Kind::Split(s))) => {
                s.fraction = *f;
            }
            (Some(ui_event::Event::Toggled(checked)), Some(node::Kind::Checkbox(c))) => {
                c.checked = *checked;
            }
            (Some(ui_event::Event::SelectedOption(idx)), Some(node::Kind::Select(s))) => {
                s.selected_index = *idx;
            }
            _ => {}
        }
        self.captured = capture;
        Ok(())
    }
}
