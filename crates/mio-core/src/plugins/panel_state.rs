use std::{
    collections::{BTreeSet, HashMap, HashSet},
    ops::ControlFlow,
};

use mio_plugin_api::panel::*;

fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn coordinate(value: f32) -> bool {
    value.is_finite() && value.abs() <= 16_384.0
}

fn label(value: &str, maximum: usize) -> Result<(), String> {
    check(
        !value.is_empty() && value.len() <= maximum,
        "标签为空或超过限额",
    )
}

fn preserve_table_revisions_with(
    prev_root: &Node,
    next_root: &Node,
    allow_vp: &mut impl FnMut(u64, &VirtualTable, &VirtualTable) -> bool,
) -> Result<(), String> {
    match prev_root.try_visit(&mut |node| {
        if let Some(node::Kind::VirtualTable(previous)) = &node.kind
            && let Some(node::Kind::VirtualTable(next)) =
                next_root.find(node.id).and_then(|n| n.kind.as_ref())
            && let Err(err) = check(
                allow_vp(node.id, previous, next),
                "表格 viewport revision 已过期；需要重新同步",
            )
        {
            return ControlFlow::Break(err);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(err) => Err(err),
        ControlFlow::Continue(()) => Ok(()),
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum OptimisticFieldKey {
    Viewport(u64),
    TabSelected(u64),
    SplitResized(u64),
    Toggled(u64),
    SelectedOption(u64),
}

#[derive(Clone, Debug, PartialEq)]
enum OptimisticFieldValue {
    Viewport {
        revision: u64,
        offset: u64,
        limit: u32,
    },
    TabSelected {
        id: u64,
        title: String,
    },
    SplitResized {
        axis: i32,
        fraction: f32,
    },
    Toggled {
        label: String,
        checked: bool,
    },
    SelectedOption {
        label: String,
        index: u32,
        option: String,
    },
}

impl OptimisticFieldValue {
    fn viewport(&self) -> Option<u64> {
        match self {
            Self::Viewport { revision, .. } => Some(*revision),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct OptimisticFieldEntry {
    request_id: u64,
    value: OptimisticFieldValue,
    uncertain: bool,
}

#[derive(Clone, Debug)]
struct OptimisticFieldLedger {
    base: OptimisticFieldValue,
    pending: Vec<OptimisticFieldEntry>,
}

#[derive(Clone, Debug)]
struct DetachedViewportEntry {
    request_id: u64,
    revision: u64,
    offset: u64,
    limit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PrunedViewportEntry {
    revision: u64,
    offset: u64,
    clamped_at_revision: bool,
}

const MAX_OPTIMISTIC_PENDING: usize = 128;

/// Apply only on the service thread. A failed patch leaves the last valid view intact.
#[derive(Clone, Debug)]
pub struct PanelState {
    snapshot: PanelSnapshot,
    captured: Option<(u64, u64)>,
    optimistic_fields: HashMap<OptimisticFieldKey, OptimisticFieldLedger>,
    detached_viewports: HashMap<u64, Vec<DetachedViewportEntry>>,
    uncertain_optimistic_ids: BTreeSet<u64>,
    confirmed_viewport_requests: BTreeSet<u64>,
    acked_viewports: HashMap<u64, Vec<(u64, u64)>>,
    pruned_viewports: HashMap<u64, Vec<PrunedViewportEntry>>,
}

impl PanelState {
    pub fn new(snapshot: PanelSnapshot) -> Result<Self, String> {
        snapshot.validate()?;
        Ok(Self {
            snapshot,
            captured: None,
            optimistic_fields: HashMap::new(),
            detached_viewports: HashMap::new(),
            uncertain_optimistic_ids: BTreeSet::new(),
            confirmed_viewport_requests: BTreeSet::new(),
            acked_viewports: HashMap::new(),
            pruned_viewports: HashMap::new(),
        })
    }

    pub fn snapshot(&self) -> &PanelSnapshot {
        &self.snapshot
    }

    pub fn captured(&self) -> Option<(u64, u64)> {
        self.captured
    }

    pub fn clear_capture(&mut self) {
        self.captured = None;
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

    fn table_watermark_for_request(
        &self,
        table_id: u64,
        current_vp: u64,
        completed_request_id: u64,
        unclaimed_request_ids: &HashSet<u64>,
    ) -> u64 {
        match self
            .optimistic_fields
            .get(&OptimisticFieldKey::Viewport(table_id))
        {
            Some(ledger) if completed_request_id == 0 => {
                ledger.base.viewport().unwrap_or(current_vp)
            }
            Some(ledger) => ledger
                .pending
                .iter()
                .rev()
                .find(|e| {
                    !e.uncertain
                        && !unclaimed_request_ids.contains(&e.request_id)
                        && e.request_id == completed_request_id
                })
                .map(|e| &e.value)
                .unwrap_or(&ledger.base)
                .viewport()
                .unwrap_or(current_vp),
            None => current_vp,
        }
    }

    fn has_unclamped_pruned_viewport(&self, table_id: u64, viewport_revision: u64) -> bool {
        self.pruned_viewports.get(&table_id).is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.revision == viewport_revision && !entry.clamped_at_revision)
        })
    }

    fn is_pruned_viewport_rejected(
        &self,
        table_id: u64,
        previous_offset: u64,
        candidate_vp: u64,
        candidate_offset: u64,
        allow_clamped_resync: bool,
    ) -> bool {
        self.pruned_viewports.get(&table_id).is_some_and(|entries| {
            entries.iter().any(|entry| {
                if entry.revision != candidate_vp {
                    return false;
                }
                if allow_clamped_resync {
                    candidate_offset == entry.offset
                        || (entry.clamped_at_revision && candidate_offset != previous_offset)
                } else {
                    !entry.clamped_at_revision
                        || candidate_offset == entry.offset
                        || candidate_offset != previous_offset
                }
            })
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn table_watermark_allowed_for_request(
        &self,
        table_id: u64,
        current_vp: u64,
        previous_offset: u64,
        completed_request_id: u64,
        unclaimed_request_ids: &HashSet<u64>,
        candidate_vp: u64,
        candidate_offset: u64,
    ) -> bool {
        if self.is_pruned_viewport_rejected(
            table_id,
            previous_offset,
            candidate_vp,
            candidate_offset,
            false,
        ) {
            return false;
        }
        let floor = self.table_watermark_for_request(
            table_id,
            current_vp,
            completed_request_id,
            unclaimed_request_ids,
        );
        if candidate_vp < floor {
            return false;
        }
        if candidate_vp == floor {
            return true;
        }
        if self
            .optimistic_fields
            .get(&OptimisticFieldKey::Viewport(table_id))
            .is_some_and(|ledger| {
                ledger.pending.iter().any(|e| {
                    !unclaimed_request_ids.contains(&e.request_id)
                        && e.value.viewport() == Some(candidate_vp)
                })
            })
        {
            return true;
        }
        if self
            .detached_viewports
            .get(&table_id)
            .is_some_and(|entries| {
                entries.iter().any(|e| {
                    !unclaimed_request_ids.contains(&e.request_id) && e.revision == candidate_vp
                })
            })
        {
            return true;
        }
        self.acked_viewports
            .get(&table_id)
            .is_some_and(|entries| entries.iter().any(|&(rev, _)| rev == candidate_vp))
    }

    fn preserve_native_revisions_for_request(
        &self,
        next: &PanelSnapshot,
        completed_request_id: u64,
        unclaimed_request_ids: &HashSet<u64>,
        allow_pruned_viewports: bool,
    ) -> Result<(), String> {
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
            preserve_table_revisions_with(
                previous,
                next,
                &mut |table_id, previous, next| {
                    let floor = self.table_watermark_for_request(
                        table_id,
                        previous.viewport_revision,
                        completed_request_id,
                        unclaimed_request_ids,
                    );
                    next.viewport_revision >= floor
                        && !self.is_pruned_viewport_rejected(
                            table_id,
                            previous.offset,
                            next.viewport_revision,
                            next.offset,
                            allow_pruned_viewports,
                        )
                },
            )?;
        }
        Ok(())
    }

    pub fn replace(&mut self, next: PanelSnapshot) -> Result<(), String> {
        self.replace_for_request(next, 0).map(|_| ())
    }

    pub fn replace_for_request(
        &mut self,
        next: PanelSnapshot,
        completed_request_id: u64,
    ) -> Result<Vec<u64>, String> {
        self.replace_for_request_with_unclaimed(next, completed_request_id, &HashSet::new())
    }

    pub fn replace_for_request_with_unclaimed(
        &mut self,
        next: PanelSnapshot,
        completed_request_id: u64,
        unclaimed_request_ids: &HashSet<u64>,
    ) -> Result<Vec<u64>, String> {
        self.replace_for_request_inner(next, completed_request_id, unclaimed_request_ids, false)
    }

    pub fn replace_for_resync_with_unclaimed(
        &mut self,
        next: PanelSnapshot,
        completed_request_id: u64,
        unclaimed_request_ids: &HashSet<u64>,
    ) -> Result<Vec<u64>, String> {
        self.replace_for_request_inner(next, completed_request_id, unclaimed_request_ids, true)
    }

    fn replace_for_request_inner(
        &mut self,
        next: PanelSnapshot,
        completed_request_id: u64,
        unclaimed_request_ids: &HashSet<u64>,
        allow_pruned_viewports: bool,
    ) -> Result<Vec<u64>, String> {
        check(
            next.revision > self.snapshot.revision,
            "Panel snapshot 已过期",
        )?;
        next.validate()?;
        self.preserve_native_revisions_for_request(
            &next,
            completed_request_id,
            unclaimed_request_ids,
            allow_pruned_viewports,
        )?;
        self.snapshot = next;
        let mut invalidated = Vec::new();
        if completed_request_id > 0 {
            let mut settled_uncertain_ids = BTreeSet::new();
            self.uncertain_optimistic_ids.retain(|&id| {
                let remove = id <= completed_request_id;
                if remove && allow_pruned_viewports {
                    settled_uncertain_ids.insert(id);
                }
                !remove
            });
            if !settled_uncertain_ids.is_empty() {
                self.confirmed_viewport_requests
                    .retain(|id| !settled_uncertain_ids.contains(id));
                self.detached_viewports.retain(|_, entries| {
                    entries.retain(|e| !settled_uncertain_ids.contains(&e.request_id));
                    !entries.is_empty()
                });
            }
            self.confirmed_viewport_requests
                .remove(&completed_request_id);
        }
        let root_ref = self.snapshot.root.as_ref();
        prune_pruned_viewports(&mut self.pruned_viewports, root_ref);
        prune_acked_viewports(
            &mut self.acked_viewports,
            &mut self.pruned_viewports,
            root_ref,
        );
        prune_detached_viewports(
            &mut self.detached_viewports,
            completed_request_id,
            unclaimed_request_ids,
            root_ref,
            None,
            &mut self.uncertain_optimistic_ids,
            &mut self.confirmed_viewport_requests,
            &mut self.acked_viewports,
            &mut self.pruned_viewports,
            &mut invalidated,
        );
        let mut surviving = HashMap::new();
        let mut re_apply = Vec::new();
        for (key, ledger) in self.optimistic_fields.drain() {
            rebase_optimistic_ledger(
                key,
                ledger,
                completed_request_id,
                unclaimed_request_ids,
                root_ref,
                &mut self.uncertain_optimistic_ids,
                &mut self.confirmed_viewport_requests,
                &mut self.pruned_viewports,
                &mut invalidated,
                &mut re_apply,
                &mut surviving,
                &mut self.detached_viewports,
            );
        }
        self.optimistic_fields = surviving;
        if let Some(root) = self.snapshot.root.as_mut() {
            for (key, value) in re_apply {
                let _ = apply_optimistic_value(root, key, &value);
            }
        }
        invalidated.sort_unstable();
        invalidated.dedup();
        if let Some((node, hit)) = self.captured
            && !self.node(node).is_some_and(|n| matches!(&n.kind, Some(node::Kind::Canvas(c)) if c.hit_regions.iter().any(|h| h.id == hit)))
        {
            self.captured = None;
        }
        Ok(invalidated)
    }

    pub fn apply(&mut self, patch: &PanelPatch) -> Result<(), String> {
        self.apply_for_request(patch, 0).map(|_| ())
    }

    pub fn apply_for_request(
        &mut self,
        patch: &PanelPatch,
        completed_request_id: u64,
    ) -> Result<Vec<u64>, String> {
        self.apply_for_request_with_unclaimed(patch, completed_request_id, &HashSet::new())
    }

    pub fn apply_for_request_with_unclaimed(
        &mut self,
        patch: &PanelPatch,
        completed_request_id: u64,
        unclaimed_request_ids: &HashSet<u64>,
    ) -> Result<Vec<u64>, String> {
        check(
            patch.base_revision == self.snapshot.revision && patch.revision > patch.base_revision,
            "Panel patch revision 已过期；需要重新同步",
        )?;
        check(
            !patch.operations.is_empty() && patch.operations.len() <= MAX_NODES,
            "Panel patch 大小无效",
        )?;
        let mut next = self.snapshot.clone();
        let mut replaced_ids = HashSet::new();
        let mut patched_tables = HashSet::new();
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
                    node.collect_ids(&mut replaced_ids);
                    replacement.collect_ids(&mut replaced_ids);
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
                    let allowed = if replaced_ids.contains(&operation.node_id)
                        || patched_tables.contains(&operation.node_id)
                    {
                        table.viewport_revision == previous.viewport_revision
                    } else {
                        self.table_watermark_allowed_for_request(
                            operation.node_id,
                            previous.viewport_revision,
                            previous.offset,
                            completed_request_id,
                            unclaimed_request_ids,
                            table.viewport_revision,
                            table.offset,
                        )
                    };
                    check(allowed, "表格 viewport revision 已过期；需要重新同步")?;
                    patched_tables.insert(operation.node_id);
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
        self.preserve_native_revisions_for_request(
            &next,
            completed_request_id,
            unclaimed_request_ids,
            false,
        )?;
        self.snapshot = next;
        let root_ref = self.snapshot.root.as_ref();
        let mut invalidated = Vec::new();
        if completed_request_id > 0 {
            self.confirmed_viewport_requests
                .remove(&completed_request_id);
        }
        prune_pruned_viewports(&mut self.pruned_viewports, root_ref);
        prune_acked_viewports(
            &mut self.acked_viewports,
            &mut self.pruned_viewports,
            root_ref,
        );
        prune_detached_viewports(
            &mut self.detached_viewports,
            completed_request_id,
            unclaimed_request_ids,
            root_ref,
            Some((&replaced_ids, &patched_tables)),
            &mut self.uncertain_optimistic_ids,
            &mut self.confirmed_viewport_requests,
            &mut self.acked_viewports,
            &mut self.pruned_viewports,
            &mut invalidated,
        );
        let mut surviving = HashMap::new();
        let mut re_apply = Vec::new();
        for (key, ledger) in self.optimistic_fields.drain() {
            let touched = match key {
                OptimisticFieldKey::Viewport(id) => {
                    replaced_ids.contains(&id) || patched_tables.contains(&id)
                }
                OptimisticFieldKey::TabSelected(id)
                | OptimisticFieldKey::SplitResized(id)
                | OptimisticFieldKey::Toggled(id)
                | OptimisticFieldKey::SelectedOption(id) => replaced_ids.contains(&id),
            };
            if !touched {
                surviving.insert(key, ledger);
                continue;
            }
            rebase_optimistic_ledger(
                key,
                ledger,
                completed_request_id,
                unclaimed_request_ids,
                root_ref,
                &mut self.uncertain_optimistic_ids,
                &mut self.confirmed_viewport_requests,
                &mut self.pruned_viewports,
                &mut invalidated,
                &mut re_apply,
                &mut surviving,
                &mut self.detached_viewports,
            );
        }
        self.optimistic_fields = surviving;
        if let Some(root) = self.snapshot.root.as_mut() {
            for (key, value) in re_apply {
                let _ = apply_optimistic_value(root, key, &value);
            }
        }
        invalidated.sort_unstable();
        invalidated.dedup();
        if let Some((node, hit)) = self.captured
            && !self.node(node).is_some_and(|n| matches!(&n.kind, Some(node::Kind::Canvas(c)) if c.hit_regions.iter().any(|h| h.id == hit)))
        {
            self.captured = None;
        }
        Ok(invalidated)
    }

    /// Record authoritative native input before dispatch; read-only and stale editor events fail here.
    pub fn observe_event(&mut self, event: &UiEvent) -> Result<(), String> {
        self.observe_event_for_request(event, 0)
    }

    pub fn observe_event_for_request(
        &mut self,
        event: &UiEvent,
        request_id: u64,
    ) -> Result<(), String> {
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
                    next.observe_one(
                        &UiEvent {
                            node_id,
                            event: Some(ui_event::Event::EditorChanged(snapshot.clone())),
                            editors: vec![],
                        },
                        0,
                    )?;
                }
            }
        } else {
            check(event.editors.is_empty(), "此事件没有申请编辑器提交快照")?;
        }
        next.observe_one(event, request_id)?;
        *self = next;
        Ok(())
    }

    fn observe_one(&mut self, event: &UiEvent, request_id: u64) -> Result<(), String> {
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
                        && !self.has_unclamped_pruned_viewport(event.node_id, v.revision)
                        && v.limit > 0
                        && v.limit as usize <= MAX_TABLE_ROWS
                        && (v.offset == 0 || v.offset < t.total_rows),
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
        let mut recorded: Option<(OptimisticFieldKey, OptimisticFieldValue, OptimisticFieldValue)> =
            None;
        match (&event.event, &mut node.kind) {
            (Some(ui_event::Event::EditorChanged(s)), Some(node::Kind::Editor(e))) => {
                e.revision = s.revision;
                e.contents.clone_from(&s.contents);
            }
            (Some(ui_event::Event::Viewport(v)), Some(node::Kind::VirtualTable(t))) => {
                if request_id != 0 {
                    recorded = Some((
                        OptimisticFieldKey::Viewport(event.node_id),
                        OptimisticFieldValue::Viewport {
                            revision: t.viewport_revision,
                            offset: t.offset,
                            limit: t.rows.len().clamp(1, MAX_TABLE_ROWS) as u32,
                        },
                        OptimisticFieldValue::Viewport {
                            revision: v.revision,
                            offset: v.offset,
                            limit: v.limit,
                        },
                    ));
                }
                t.viewport_revision = v.revision;
            }
            (Some(ui_event::Event::TabSelected(id)), Some(node::Kind::Tabs(t))) => {
                if request_id != 0 {
                    let old_title = t
                        .tabs
                        .iter()
                        .find_map(|tab| {
                            tab.content
                                .as_ref()
                                .is_some_and(|c| c.id == t.active)
                                .then(|| tab.title.clone())
                        })
                        .unwrap_or_default();
                    let new_title = t
                        .tabs
                        .iter()
                        .find_map(|tab| {
                            tab.content
                                .as_ref()
                                .is_some_and(|c| c.id == *id)
                                .then(|| tab.title.clone())
                        })
                        .unwrap_or_default();
                    recorded = Some((
                        OptimisticFieldKey::TabSelected(event.node_id),
                        OptimisticFieldValue::TabSelected {
                            id: t.active,
                            title: old_title,
                        },
                        OptimisticFieldValue::TabSelected {
                            id: *id,
                            title: new_title,
                        },
                    ));
                }
                t.active = *id;
            }
            (Some(ui_event::Event::SplitResized(f)), Some(node::Kind::Split(s))) => {
                if request_id != 0 {
                    recorded = Some((
                        OptimisticFieldKey::SplitResized(event.node_id),
                        OptimisticFieldValue::SplitResized {
                            axis: s.axis,
                            fraction: s.fraction,
                        },
                        OptimisticFieldValue::SplitResized {
                            axis: s.axis,
                            fraction: *f,
                        },
                    ));
                }
                s.fraction = *f;
            }
            (Some(ui_event::Event::Toggled(checked)), Some(node::Kind::Checkbox(c))) => {
                if request_id != 0 {
                    recorded = Some((
                        OptimisticFieldKey::Toggled(event.node_id),
                        OptimisticFieldValue::Toggled {
                            label: c.label.clone(),
                            checked: c.checked,
                        },
                        OptimisticFieldValue::Toggled {
                            label: c.label.clone(),
                            checked: *checked,
                        },
                    ));
                }
                c.checked = *checked;
            }
            (Some(ui_event::Event::SelectedOption(idx)), Some(node::Kind::Select(s))) => {
                if request_id != 0 {
                    let old_option = s
                        .options
                        .get(s.selected_index as usize)
                        .cloned()
                        .unwrap_or_default();
                    let new_option = s.options[*idx as usize].clone();
                    recorded = Some((
                        OptimisticFieldKey::SelectedOption(event.node_id),
                        OptimisticFieldValue::SelectedOption {
                            label: s.label.clone(),
                            index: s.selected_index,
                            option: old_option,
                        },
                        OptimisticFieldValue::SelectedOption {
                            label: s.label.clone(),
                            index: *idx,
                            option: new_option,
                        },
                    ));
                }
                s.selected_index = *idx;
            }
            _ => {}
        }
        if let Some((key, old_val, new_val)) = recorded {
            let entry = self
                .optimistic_fields
                .entry(key)
                .or_insert_with(|| OptimisticFieldLedger {
                    base: old_val,
                    pending: Vec::new(),
                });
            if entry.pending.len() >= MAX_OPTIMISTIC_PENDING {
                let promoted = entry.pending.remove(0);
                entry.base = promoted.value;
            }
            entry.pending.push(OptimisticFieldEntry {
                request_id,
                value: new_val,
                uncertain: false,
            });
        }
        self.captured = capture;
        Ok(())
    }

    pub fn retract_optimistic_request(&mut self, request_id: u64) -> bool {
        if request_id == 0 {
            return false;
        }
        self.uncertain_optimistic_ids.remove(&request_id);
        self.confirmed_viewport_requests.remove(&request_id);
        self.detached_viewports.retain(|_, entries| {
            entries.retain(|e| e.request_id != request_id);
            !entries.is_empty()
        });
        let mut reverted_any = false;
        for (key, ledger) in &mut self.optimistic_fields {
            let before_len = ledger.pending.len();
            ledger.pending.retain(|e| e.request_id != request_id);
            if ledger.pending.len() != before_len {
                let effective = ledger
                    .pending
                    .last()
                    .map(|e| e.value.clone())
                    .unwrap_or_else(|| ledger.base.clone());
                if let Some(root) = self.snapshot.root.as_mut()
                    && apply_optimistic_value(root, *key, &effective)
                {
                    reverted_any = true;
                }
            }
        }
        self.optimistic_fields
            .retain(|_, ledger| !ledger.pending.is_empty());
        reverted_any
    }

    pub fn has_uncertain_optimistic_requests(&self) -> bool {
        !self.uncertain_optimistic_ids.is_empty()
    }

    pub fn patch_confirms_detached_viewport(&self, patch: &PanelPatch, request_id: u64) -> bool {
        if request_id == 0 || self.detached_viewports.is_empty() {
            return false;
        }
        for op in &patch.operations {
            match &op.change {
                Some(patch_operation::Change::Table(table)) => {
                    if let Some(entries) = self.detached_viewports.get(&op.node_id)
                        && entries
                            .iter()
                            .any(|e| e.request_id == request_id && table.viewport_revision >= e.revision)
                    {
                        return true;
                    }
                }
                Some(patch_operation::Change::Replace(node)) => {
                    for (&table_id, entries) in &self.detached_viewports {
                        if let Some(entry) = entries.iter().find(|e| e.request_id == request_id)
                            && let Some(found) = node.find(table_id)
                            && matches!(
                                &found.kind,
                                Some(node::Kind::VirtualTable(table))
                                    if table.viewport_revision >= entry.revision
                            )
                        {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    pub fn mark_optimistic_request_uncertain(&mut self, request_id: u64) -> bool {
        if request_id == 0 || self.confirmed_viewport_requests.contains(&request_id) {
            return false;
        }
        if self.uncertain_optimistic_ids.insert(request_id)
            && self.uncertain_optimistic_ids.len() > MAX_OPTIMISTIC_PENDING
        {
            self.uncertain_optimistic_ids.pop_first();
        }
        for ledger in self.optimistic_fields.values_mut() {
            for entry in &mut ledger.pending {
                if entry.request_id == request_id {
                    entry.uncertain = true;
                }
            }
        }
        true
    }

    pub fn retract_uncertain_optimistic_requests(&mut self) -> bool {
        let uncertain_ids = std::mem::take(&mut self.uncertain_optimistic_ids);
        let had_recorded = !uncertain_ids.is_empty();
        if had_recorded {
            self.confirmed_viewport_requests
                .retain(|id| !uncertain_ids.contains(id));
            self.detached_viewports.retain(|_, entries| {
                entries.retain(|e| !uncertain_ids.contains(&e.request_id));
                !entries.is_empty()
            });
        }
        let mut had_uncertain = false;
        for (key, ledger) in &mut self.optimistic_fields {
            let before_len = ledger.pending.len();
            ledger.pending.retain(|e| !e.uncertain);
            if ledger.pending.len() != before_len {
                had_uncertain = true;
                let effective = ledger
                    .pending
                    .last()
                    .map(|e| e.value.clone())
                    .unwrap_or_else(|| ledger.base.clone());
                if let Some(root) = self.snapshot.root.as_mut() {
                    let _ = apply_optimistic_value(root, *key, &effective);
                }
            }
        }
        self.optimistic_fields
            .retain(|_, ledger| !ledger.pending.is_empty());
        had_uncertain || had_recorded
    }

    pub fn settle_uncertain_optimistic_requests_through(&mut self, max_request_id: u64) -> bool {
        self.settle_uncertain_optimistic_requests_through_with_promoted(max_request_id)
            .0
    }

    pub fn settle_uncertain_optimistic_requests_through_with_promoted(
        &mut self,
        max_request_id: u64,
    ) -> (bool, BTreeSet<u64>) {
        if max_request_id == 0 {
            return (false, BTreeSet::new());
        }
        let mut settled = false;
        let mut promoted_ids = BTreeSet::new();
        let mut settled_ids = BTreeSet::new();
        self.uncertain_optimistic_ids.retain(|&id| {
            let remove = id <= max_request_id;
            if remove {
                settled_ids.insert(id);
                settled = true;
            }
            !remove
        });
        if !settled_ids.is_empty() {
            self.confirmed_viewport_requests
                .retain(|id| !settled_ids.contains(id));
            self.detached_viewports.retain(|_, entries| {
                entries.retain(|e| !settled_ids.contains(&e.request_id));
                !entries.is_empty()
            });
        }
        self.optimistic_fields.retain(|_, ledger| {
            let prefix = ledger
                .pending
                .iter()
                .take_while(|e| e.uncertain && e.request_id <= max_request_id)
                .count();
            if prefix > 0 {
                promoted_ids.extend(ledger.pending[..prefix].iter().map(|e| e.request_id));
                ledger.base = ledger.pending[prefix - 1].value.clone();
                ledger.pending.drain(..prefix);
                settled = true;
            }
            !ledger.pending.is_empty()
        });
        (settled, promoted_ids)
    }

    pub fn snapshot_with_confirmed_viewports(
        &self,
        active_request_ids: &HashSet<u64>,
    ) -> PanelSnapshot {
        let mut snapshot = self.snapshot.clone();
        let Some(root) = snapshot.root.as_mut() else {
            return snapshot;
        };
        for (key, ledger) in &self.optimistic_fields {
            let OptimisticFieldKey::Viewport(table_id) = *key else {
                continue;
            };
            let has_unconfirmed = ledger
                .pending
                .iter()
                .any(|e| e.uncertain || active_request_ids.contains(&e.request_id));
            if !has_unconfirmed {
                continue;
            }
            if let Some(confirmed_vp) = ledger
                .pending
                .iter()
                .rev()
                .find(|e| !e.uncertain && !active_request_ids.contains(&e.request_id))
                .map(|e| &e.value)
                .unwrap_or(&ledger.base)
                .viewport()
                && let Some(node) = root.find_mut(table_id)
                && let Some(node::Kind::VirtualTable(table)) = &mut node.kind
            {
                table.viewport_revision = confirmed_vp;
            }
        }
        snapshot
    }

    pub fn has_optimistic_request(&self, request_id: u64) -> bool {
        request_id != 0
            && self
                .optimistic_fields
                .values()
                .any(|ledger| ledger.pending.iter().any(|e| e.request_id == request_id))
    }

    pub fn has_confirmed_viewport_request(&self, request_id: u64) -> bool {
        request_id != 0 && self.confirmed_viewport_requests.contains(&request_id)
    }

    pub fn has_detached_viewport(&self, request_id: u64) -> bool {
        request_id != 0
            && self
                .detached_viewports
                .values()
                .any(|entries| entries.iter().any(|e| e.request_id == request_id))
    }

    pub fn has_tracked_optimistic_request(&self, request_id: u64) -> bool {
        request_id != 0
            && (self.has_optimistic_request(request_id)
                || self
                    .detached_viewports
                    .values()
                    .any(|entries| entries.iter().any(|e| e.request_id == request_id))
                || self.uncertain_optimistic_ids.contains(&request_id)
                || self.confirmed_viewport_requests.contains(&request_id))
    }

    pub fn settle_optimistic_request(&mut self, request_id: u64) -> bool {
        self.settle_optimistic_request_inner(request_id, true)
    }

    pub fn settle_cancelled_optimistic_request(&mut self, request_id: u64) -> bool {
        self.settle_optimistic_request_inner(request_id, false)
    }

    fn settle_optimistic_request_inner(
        &mut self,
        request_id: u64,
        record_acked_viewport: bool,
    ) -> bool {
        if request_id == 0 {
            return false;
        }
        let mut settled = self.confirmed_viewport_requests.remove(&request_id);
        self.uncertain_optimistic_ids.remove(&request_id);
        let mut acked_viewports = Vec::new();
        self.detached_viewports.retain(|&table_id, entries| {
            let before = entries.len();
            entries.retain(|e| {
                let matched = e.request_id == request_id;
                if matched {
                    acked_viewports.push((table_id, e.revision, e.offset));
                }
                !matched
            });
            if entries.len() != before {
                settled = true;
            }
            !entries.is_empty()
        });
        let mut rollback_viewports = Vec::new();
        self.optimistic_fields.retain(|key, ledger| {
            if let Some(pos) = ledger
                .pending
                .iter()
                .position(|e| e.request_id == request_id)
            {
                if let OptimisticFieldKey::Viewport(table_id) = key {
                    // Bare Ack means the Worker acknowledged the event without publishing a newer table
                    // viewport; drop the optimistic watermark instead of advancing the host floor, while
                    // preserving an async patch ceiling if the Worker later pushes that viewport via Patch.
                    let removed = ledger.pending.remove(pos);
                    if let OptimisticFieldValue::Viewport {
                        revision, offset, ..
                    } = removed.value
                    {
                        acked_viewports.push((*table_id, revision, offset));
                    }
                    let effective = ledger
                        .pending
                        .last()
                        .map(|e| e.value.clone())
                        .unwrap_or_else(|| ledger.base.clone());
                    rollback_viewports.push((*key, effective));
                } else {
                    ledger.base = ledger.pending[pos].value.clone();
                    ledger.pending.drain(..=pos);
                }
                settled = true;
            }
            !ledger.pending.is_empty()
        });
        if let Some(root) = self.snapshot.root.as_mut() {
            for (key, value) in rollback_viewports {
                let _ = apply_optimistic_value(root, key, &value);
            }
        }
        if record_acked_viewport && !acked_viewports.is_empty() {
            let root_ref = self.snapshot.root.as_ref();
            for (table_id, rev, offset) in acked_viewports {
                if let Some(node::Kind::VirtualTable(table)) = root_ref
                    .and_then(|r| r.find(table_id))
                    .and_then(|n| n.kind.as_ref())
                {
                    let confirmed_vp = self
                        .optimistic_fields
                        .get(&OptimisticFieldKey::Viewport(table_id))
                        .and_then(|ledger| ledger.base.viewport())
                        .unwrap_or(table.viewport_revision);
                    if confirmed_vp < rev {
                        if offset == 0 || offset < table.total_rows {
                            push_acked_viewport(&mut self.acked_viewports, table_id, rev, offset);
                        } else {
                            record_pruned_viewport(
                                &mut self.pruned_viewports,
                                table_id,
                                rev,
                                offset,
                            );
                        }
                    }
                }
            }
        }
        settled
    }
}

fn push_acked_viewport(
    acked_viewports: &mut HashMap<u64, Vec<(u64, u64)>>,
    table_id: u64,
    revision: u64,
    offset: u64,
) {
    let entries = acked_viewports.entry(table_id).or_default();
    if entries.iter().any(|&(r, o)| r == revision && o == offset) {
        return;
    }
    if entries.len() >= MAX_OPTIMISTIC_PENDING {
        entries.remove(0);
    }
    entries.push((revision, offset));
}

fn record_pruned_viewport(
    pruned_viewports: &mut HashMap<u64, Vec<PrunedViewportEntry>>,
    table_id: u64,
    revision: u64,
    offset: u64,
) {
    let entries = pruned_viewports.entry(table_id).or_default();
    if entries
        .iter()
        .any(|e| e.revision == revision && e.offset == offset)
    {
        return;
    }
    if entries.len() >= MAX_OPTIMISTIC_PENDING {
        entries.remove(0);
    }
    entries.push(PrunedViewportEntry {
        revision,
        offset,
        clamped_at_revision: false,
    });
}

fn prune_pruned_viewports(
    pruned_viewports: &mut HashMap<u64, Vec<PrunedViewportEntry>>,
    root_ref: Option<&Node>,
) {
    pruned_viewports.retain(|&table_id, entries| {
        let Some(node::Kind::VirtualTable(table)) = root_ref
            .and_then(|r| r.find(table_id))
            .and_then(|n| n.kind.as_ref())
        else {
            return false;
        };
        entries.retain_mut(|entry| {
            if table.viewport_revision > entry.revision {
                return false;
            }
            if table.viewport_revision == entry.revision {
                entry.clamped_at_revision = true;
            }
            true
        });
        !entries.is_empty()
    });
}

fn prune_acked_viewports(
    acked_viewports: &mut HashMap<u64, Vec<(u64, u64)>>,
    pruned_viewports: &mut HashMap<u64, Vec<PrunedViewportEntry>>,
    root_ref: Option<&Node>,
) {
    acked_viewports.retain(|&table_id, entries| {
        let Some(node::Kind::VirtualTable(table)) = root_ref
            .and_then(|r| r.find(table_id))
            .and_then(|n| n.kind.as_ref())
        else {
            return false;
        };
        entries.retain(|&(rev, offset)| {
            let keep =
                rev > table.viewport_revision && (offset == 0 || offset < table.total_rows);
            if !keep && rev > table.viewport_revision {
                record_pruned_viewport(pruned_viewports, table_id, rev, offset);
            }
            keep
        });
        !entries.is_empty()
    });
}

fn record_confirmed_viewport_request(confirmed: &mut BTreeSet<u64>, request_id: u64) {
    if request_id != 0
        && confirmed.insert(request_id)
        && confirmed.len() > MAX_OPTIMISTIC_PENDING
    {
        confirmed.pop_first();
    }
}

fn viewport_is_confirmed_by_table(table: &VirtualTable, revision: u64, offset: u64) -> bool {
    table.viewport_revision > revision
        || (table.viewport_revision == revision
            && (table.offset == offset || (offset > 0 && offset >= table.total_rows)))
}

fn viewport_is_pending_on_table(
    table: &VirtualTable,
    revision: u64,
    offset: u64,
    limit: u32,
) -> bool {
    limit > 0
        && (limit as usize) <= MAX_TABLE_ROWS
        && (offset == 0 || offset < table.total_rows)
        && (revision > table.viewport_revision
            || (revision == table.viewport_revision && table.offset != offset))
}

fn push_detached_viewport(
    detached_viewports: &mut HashMap<u64, Vec<DetachedViewportEntry>>,
    table_id: u64,
    entry: DetachedViewportEntry,
) {
    let entries = detached_viewports.entry(table_id).or_default();
    if entries.len() >= MAX_OPTIMISTIC_PENDING {
        entries.remove(0);
    }
    entries.push(entry);
}

#[allow(clippy::too_many_arguments)]
fn prune_detached_viewports(
    detached_viewports: &mut HashMap<u64, Vec<DetachedViewportEntry>>,
    completed_request_id: u64,
    unclaimed_request_ids: &HashSet<u64>,
    root_ref: Option<&Node>,
    touched_tables: Option<(&HashSet<u64>, &HashSet<u64>)>,
    uncertain_optimistic_ids: &mut BTreeSet<u64>,
    confirmed_viewport_requests: &mut BTreeSet<u64>,
    acked_viewports: &mut HashMap<u64, Vec<(u64, u64)>>,
    pruned_viewports: &mut HashMap<u64, Vec<PrunedViewportEntry>>,
    invalidated: &mut Vec<u64>,
) {
    detached_viewports.retain(|&table_id, entries| {
        if let Some((replaced_ids, patched_tables)) = touched_tables
            && !replaced_ids.contains(&table_id)
            && !patched_tables.contains(&table_id)
        {
            return true;
        }
        let table = root_ref
            .and_then(|r| r.find(table_id))
            .and_then(|n| match &n.kind {
                Some(node::Kind::VirtualTable(t)) => Some(t),
                _ => None,
            });
        if completed_request_id > 0 {
            entries.retain(|e| {
                if e.request_id == completed_request_id {
                    if let Some(table) = table
                        && e.revision > table.viewport_revision
                    {
                        if e.offset == 0 || e.offset < table.total_rows {
                            push_acked_viewport(acked_viewports, table_id, e.revision, e.offset);
                        } else {
                            record_pruned_viewport(pruned_viewports, table_id, e.revision, e.offset);
                        }
                    }
                    false
                } else {
                    true
                }
            });
        }
        if entries.is_empty() {
            return false;
        }
        let Some(table) = table else {
            invalidated.extend(entries.drain(..).map(|e| e.request_id));
            return false;
        };
        entries.retain(|e| {
            if !unclaimed_request_ids.contains(&e.request_id)
                && (completed_request_id == 0
                    || e.request_id < completed_request_id
                    || table.viewport_revision == e.revision)
            {
                if viewport_is_confirmed_by_table(table, e.revision, e.offset) {
                    uncertain_optimistic_ids.remove(&e.request_id);
                    record_confirmed_viewport_request(confirmed_viewport_requests, e.request_id);
                    return false;
                }
                if viewport_is_pending_on_table(table, e.revision, e.offset, e.limit) {
                    return true;
                }
                if e.revision > table.viewport_revision {
                    record_pruned_viewport(pruned_viewports, table_id, e.revision, e.offset);
                }
                invalidated.push(e.request_id);
                return false;
            }
            let valid = e.revision > table.viewport_revision
                && e.limit > 0
                && (e.limit as usize) <= MAX_TABLE_ROWS
                && (e.offset == 0 || e.offset < table.total_rows);
            if !valid {
                if !unclaimed_request_ids.contains(&e.request_id)
                    && e.revision > table.viewport_revision
                {
                    record_pruned_viewport(pruned_viewports, table_id, e.revision, e.offset);
                }
                invalidated.push(e.request_id);
            }
            valid
        });
        !entries.is_empty()
    });
}

#[allow(clippy::too_many_arguments)]
fn rebase_optimistic_ledger(
    key: OptimisticFieldKey,
    mut ledger: OptimisticFieldLedger,
    completed_request_id: u64,
    unclaimed_request_ids: &HashSet<u64>,
    root_ref: Option<&Node>,
    uncertain_optimistic_ids: &mut BTreeSet<u64>,
    confirmed_viewport_requests: &mut BTreeSet<u64>,
    pruned_viewports: &mut HashMap<u64, Vec<PrunedViewportEntry>>,
    invalidated: &mut Vec<u64>,
    re_apply: &mut Vec<(OptimisticFieldKey, OptimisticFieldValue)>,
    surviving: &mut HashMap<OptimisticFieldKey, OptimisticFieldLedger>,
    detached_viewports: &mut HashMap<u64, Vec<DetachedViewportEntry>>,
) {
    if completed_request_id == 0 {
        if let (OptimisticFieldKey::Viewport(table_id), Some(root)) = (key, root_ref)
            && let Some(node::Kind::VirtualTable(table)) =
                root.find(table_id).and_then(|n| n.kind.as_ref())
        {
            for entry in &ledger.pending {
                if entry.uncertain
                    && !unclaimed_request_ids.contains(&entry.request_id)
                    && let OptimisticFieldValue::Viewport {
                        revision,
                        offset,
                        limit,
                    } = entry.value
                {
                    if viewport_is_confirmed_by_table(table, revision, offset) {
                        uncertain_optimistic_ids.remove(&entry.request_id);
                        record_confirmed_viewport_request(
                            confirmed_viewport_requests,
                            entry.request_id,
                        );
                    } else if viewport_is_pending_on_table(table, revision, offset, limit) {
                        push_detached_viewport(
                            detached_viewports,
                            table_id,
                            DetachedViewportEntry {
                                request_id: entry.request_id,
                                revision,
                                offset,
                                limit,
                            },
                        );
                    } else if revision > table.viewport_revision {
                        record_pruned_viewport(pruned_viewports, table_id, revision, offset);
                    }
                }
            }
        }
        ledger.pending.retain(|e| !e.uncertain);
    } else {
        if let (OptimisticFieldKey::Viewport(table_id), Some(root)) = (key, root_ref)
            && let Some(node::Kind::VirtualTable(table)) =
                root.find(table_id).and_then(|n| n.kind.as_ref())
        {
            for entry in &ledger.pending {
                if entry.request_id < completed_request_id
                    && !unclaimed_request_ids.contains(&entry.request_id)
                    && let OptimisticFieldValue::Viewport {
                        revision,
                        offset,
                        limit,
                    } = entry.value
                {
                    if viewport_is_confirmed_by_table(table, revision, offset) {
                        uncertain_optimistic_ids.remove(&entry.request_id);
                        record_confirmed_viewport_request(
                            confirmed_viewport_requests,
                            entry.request_id,
                        );
                    } else if !entry.uncertain
                        && viewport_is_pending_on_table(table, revision, offset, limit)
                    {
                        push_detached_viewport(
                            detached_viewports,
                            table_id,
                            DetachedViewportEntry {
                                request_id: entry.request_id,
                                revision,
                                offset,
                                limit,
                            },
                        );
                    } else {
                        if !viewport_is_pending_on_table(table, revision, offset, limit)
                            && revision > table.viewport_revision
                        {
                            record_pruned_viewport(pruned_viewports, table_id, revision, offset);
                        }
                        if !entry.uncertain {
                            invalidated.push(entry.request_id);
                        }
                    }
                }
            }
        } else if let OptimisticFieldKey::Viewport(_) = key {
            for entry in &ledger.pending {
                if entry.request_id < completed_request_id
                    && !entry.uncertain
                    && !unclaimed_request_ids.contains(&entry.request_id)
                {
                    invalidated.push(entry.request_id);
                }
            }
        }
        ledger.pending.retain(|e| e.request_id > completed_request_id);
    }
    if ledger.pending.is_empty() {
        return;
    }
    let Some(new_base) = root_ref.and_then(|root| read_valid_optimistic_base(root, key)) else {
        invalidated.extend(ledger.pending.into_iter().map(|e| e.request_id));
        return;
    };
    let root = root_ref.unwrap();
    let mut valid_pending = Vec::new();
    for entry in ledger.pending {
        if !unclaimed_request_ids.contains(&entry.request_id)
            && let (
                OptimisticFieldKey::Viewport(table_id),
                OptimisticFieldValue::Viewport {
                    revision,
                    offset,
                    limit,
                },
            ) = (key, &entry.value)
            && let Some(node::Kind::VirtualTable(table)) =
                root.find(table_id).and_then(|n| n.kind.as_ref())
            && (completed_request_id == 0 || table.viewport_revision == *revision)
        {
            if viewport_is_confirmed_by_table(table, *revision, *offset) {
                uncertain_optimistic_ids.remove(&entry.request_id);
                record_confirmed_viewport_request(confirmed_viewport_requests, entry.request_id);
            } else if viewport_is_pending_on_table(table, *revision, *offset, *limit) {
                push_detached_viewport(
                    detached_viewports,
                    table_id,
                    DetachedViewportEntry {
                        request_id: entry.request_id,
                        revision: *revision,
                        offset: *offset,
                        limit: *limit,
                    },
                );
            } else {
                if *revision > table.viewport_revision {
                    record_pruned_viewport(pruned_viewports, table_id, *revision, *offset);
                }
                invalidated.push(entry.request_id);
            }
            continue;
        }
        if !is_optimistic_value_valid(root, key, &entry.value) {
            if !unclaimed_request_ids.contains(&entry.request_id)
                && let (
                    OptimisticFieldKey::Viewport(table_id),
                    OptimisticFieldValue::Viewport {
                        revision, offset, ..
                    },
                ) = (key, &entry.value)
                && let Some(node::Kind::VirtualTable(table)) =
                    root.find(table_id).and_then(|n| n.kind.as_ref())
                && *revision > table.viewport_revision
            {
                record_pruned_viewport(pruned_viewports, table_id, *revision, *offset);
            }
            invalidated.push(entry.request_id);
        } else if completed_request_id > 0 || unclaimed_request_ids.contains(&entry.request_id) {
            valid_pending.push(entry);
        } else if let (
            OptimisticFieldKey::Viewport(table_id),
            OptimisticFieldValue::Viewport {
                revision,
                offset,
                limit,
            },
        ) = (key, &entry.value)
        {
            push_detached_viewport(
                detached_viewports,
                table_id,
                DetachedViewportEntry {
                    request_id: entry.request_id,
                    revision: *revision,
                    offset: *offset,
                    limit: *limit,
                },
            );
        }
    }
    if let Some(latest) = valid_pending.last() {
        re_apply.push((key, latest.value.clone()));
        ledger.base = new_base;
        ledger.pending = valid_pending;
        surviving.insert(key, ledger);
    }
}

fn optimistic_key_node_id(key: OptimisticFieldKey) -> u64 {
    match key {
        OptimisticFieldKey::Viewport(id)
        | OptimisticFieldKey::TabSelected(id)
        | OptimisticFieldKey::SplitResized(id)
        | OptimisticFieldKey::Toggled(id)
        | OptimisticFieldKey::SelectedOption(id) => id,
    }
}

fn read_valid_optimistic_base(
    root: &Node,
    key: OptimisticFieldKey,
) -> Option<OptimisticFieldValue> {
    let node = root.find(optimistic_key_node_id(key))?;
    match (key, &node.kind) {
        (OptimisticFieldKey::Viewport(_), Some(node::Kind::VirtualTable(t))) => {
            Some(OptimisticFieldValue::Viewport {
                revision: t.viewport_revision,
                offset: t.offset,
                limit: t.rows.len().clamp(1, MAX_TABLE_ROWS) as u32,
            })
        }
        (OptimisticFieldKey::TabSelected(_), Some(node::Kind::Tabs(t))) => {
            let title = t
                .tabs
                .iter()
                .find_map(|tab| {
                    tab.content
                        .as_ref()
                        .is_some_and(|c| c.id == t.active)
                        .then(|| tab.title.clone())
                })
                .unwrap_or_default();
            Some(OptimisticFieldValue::TabSelected {
                id: t.active,
                title,
            })
        }
        (OptimisticFieldKey::SplitResized(_), Some(node::Kind::Split(s))) => {
            Some(OptimisticFieldValue::SplitResized {
                axis: s.axis,
                fraction: s.fraction,
            })
        }
        (OptimisticFieldKey::Toggled(_), Some(node::Kind::Checkbox(c))) => {
            c.enabled.then(|| OptimisticFieldValue::Toggled {
                label: c.label.clone(),
                checked: c.checked,
            })
        }
        (OptimisticFieldKey::SelectedOption(_), Some(node::Kind::Select(s))) => {
            s.enabled.then(|| OptimisticFieldValue::SelectedOption {
                label: s.label.clone(),
                index: s.selected_index,
                option: s
                    .options
                    .get(s.selected_index as usize)
                    .cloned()
                    .unwrap_or_default(),
            })
        }
        _ => None,
    }
}

fn is_optimistic_value_valid(
    root: &Node,
    key: OptimisticFieldKey,
    value: &OptimisticFieldValue,
) -> bool {
    let Some(node) = root.find(optimistic_key_node_id(key)) else {
        return false;
    };
    match (key, value, &node.kind) {
        (
            OptimisticFieldKey::Viewport(_),
            OptimisticFieldValue::Viewport {
                revision,
                offset,
                limit,
            },
            Some(node::Kind::VirtualTable(t)),
        ) => {
            *revision > t.viewport_revision
                && *limit > 0
                && (*limit as usize) <= MAX_TABLE_ROWS
                && (*offset == 0 || *offset < t.total_rows)
        }
        (
            OptimisticFieldKey::TabSelected(_),
            OptimisticFieldValue::TabSelected { id: tab_id, title },
            Some(node::Kind::Tabs(t)),
        ) => t
            .tabs
            .iter()
            .any(|tab| tab.content.as_ref().is_some_and(|c| c.id == *tab_id) && tab.title == *title),
        (
            OptimisticFieldKey::SplitResized(_),
            OptimisticFieldValue::SplitResized { axis, fraction },
            Some(node::Kind::Split(s)),
        ) => s.axis == *axis && fraction.is_finite() && (0.05..=0.95).contains(fraction),
        (
            OptimisticFieldKey::Toggled(_),
            OptimisticFieldValue::Toggled { label, .. },
            Some(node::Kind::Checkbox(c)),
        ) => c.enabled && c.label == *label,
        (
            OptimisticFieldKey::SelectedOption(_),
            OptimisticFieldValue::SelectedOption {
                label,
                index,
                option,
            },
            Some(node::Kind::Select(s)),
        ) => {
            s.enabled
                && s.label == *label
                && s.options
                    .get(*index as usize)
                    .is_some_and(|current| current == option)
        }
        _ => false,
    }
}

fn apply_optimistic_value(
    root: &mut Node,
    key: OptimisticFieldKey,
    value: &OptimisticFieldValue,
) -> bool {
    let Some(node) = root.find_mut(optimistic_key_node_id(key)) else {
        return false;
    };
    match (value, &mut node.kind) {
        (
            OptimisticFieldValue::Viewport { revision, .. },
            Some(node::Kind::VirtualTable(t)),
        ) => {
            let changed = t.viewport_revision != *revision;
            t.viewport_revision = *revision;
            changed
        }
        (OptimisticFieldValue::TabSelected { id: active, .. }, Some(node::Kind::Tabs(t))) => {
            let changed = t.active != *active;
            t.active = *active;
            changed
        }
        (OptimisticFieldValue::SplitResized { fraction, .. }, Some(node::Kind::Split(s))) => {
            let changed = s.fraction.to_bits() != fraction.to_bits();
            s.fraction = *fraction;
            changed
        }
        (OptimisticFieldValue::Toggled { checked, .. }, Some(node::Kind::Checkbox(c))) => {
            let changed = c.checked != *checked;
            c.checked = *checked;
            changed
        }
        (OptimisticFieldValue::SelectedOption { index: idx, .. }, Some(node::Kind::Select(s))) => {
            let changed = s.selected_index != *idx;
            s.selected_index = *idx;
            changed
        }
        _ => false,
    }
}


#[cfg(test)]
mod tests;
