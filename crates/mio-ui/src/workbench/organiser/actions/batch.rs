use std::collections::HashSet;
use std::fmt::Write;

use super::super::table::format_local_time;
use super::super::*;

impl Workbench {
    pub(in crate::workbench) fn handle_organiser_row_mouse_down(
        &mut self,
        id: mio_core::Uuid,
        button: MouseButton,
        shift: bool,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row_ids: Vec<mio_core::Uuid> = self
            .organiser
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .map(|entry| entry.id)
            .collect();
        if !row_ids.contains(&id) {
            return;
        }
        if button == MouseButton::Right && self.organiser.selected_ids.contains(&id) {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.sync_organiser_table(window, cx);
            self.fail(err, cx);
            return;
        }
        let mut selected = HashSet::new();
        let mut anchor = self
            .organiser
            .selection_anchor
            .or(self.organiser.selected_id);
        if button == MouseButton::Left && shift {
            let start = anchor
                .and_then(|aid| row_ids.iter().position(|&rid| rid == aid))
                .unwrap_or(0);
            let end = row_ids.iter().position(|&rid| rid == id).unwrap_or(0);
            selected.extend(row_ids[start.min(end)..=start.max(end)].iter().copied());
            anchor = row_ids.get(start).copied();
        } else if button == MouseButton::Left && toggle {
            selected = self.organiser.selected_ids.clone();
            if selected.is_empty()
                && let Some(prev) = self.organiser.selected_id
            {
                selected.insert(prev);
            }
            if !selected.remove(&id) {
                selected.insert(id);
            }
            anchor = Some(id);
        } else {
            selected.insert(id);
            if button == MouseButton::Left {
                anchor = Some(id);
            }
        }
        let primary = selected
            .contains(&id)
            .then_some(id)
            .or_else(|| row_ids.iter().copied().find(|rid| selected.contains(rid)));
        let prev_primary = self.organiser.selected_id;
        self.organiser.selected_id = primary;
        self.organiser.selected_ids = selected.clone();
        self.organiser.selection_anchor = anchor;
        self.organiser.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.selected_id = primary;
            delegate.selected_ids = selected;
            table.refresh(cx);
        });
        if primary != prev_primary {
            self.reload_selected_organiser_detail(window, cx);
        } else {
            cx.notify();
        }
    }

    pub(in crate::workbench) fn select_all_visible_organiser_items(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let row_ids: Vec<mio_core::Uuid> = self
            .organiser
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .map(|entry| entry.id)
            .collect();
        let selected: HashSet<mio_core::Uuid> = row_ids.iter().copied().collect();
        let primary = self
            .organiser
            .selected_id
            .filter(|id| selected.contains(id))
            .or_else(|| row_ids.first().copied());
        let prev_primary = self.organiser.selected_id;
        self.organiser.selected_id = primary;
        self.organiser.selected_ids = selected.clone();
        self.organiser.selection_anchor = row_ids.first().copied();
        self.organiser.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.selected_id = primary;
            delegate.selected_ids = selected;
            table.refresh(cx);
        });
        if primary != prev_primary {
            self.reload_selected_organiser_detail(window, cx);
        } else {
            cx.notify();
        }
    }

    pub(in crate::workbench) fn set_batch_organiser_status(
        &mut self,
        status: OrganiserStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let ids = self.organiser.selected_ids.clone();
        let mut updated = 0usize;
        for item in &mut self.workspace.organiser_items {
            if ids.contains(&item.id) && item.workflow_status != status {
                item.workflow_status = status;
                updated += 1;
            }
        }
        if updated == 0 {
            return;
        }
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        self.error = None;
        self.status = format!(
            "已批量将 {updated} 条 Organiser 状态更新为「{}」。",
            status.label()
        );
        cx.notify();
    }

    pub(in crate::workbench) fn set_batch_organiser_highlight(
        &mut self,
        highlight: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() || highlight.is_some_and(|c| !(1..=9).contains(&c)) {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let ids = self.organiser.selected_ids.clone();
        let mut updated = 0usize;
        for item in &mut self.workspace.organiser_items {
            if ids.contains(&item.id) && item.highlight != highlight {
                item.highlight = highlight;
                updated += 1;
            }
        }
        if updated == 0 {
            return;
        }
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        self.error = None;
        cx.notify();
    }

    pub(in crate::workbench) fn delete_selected_organiser_batch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.organiser.selected_ids.len() <= 1 {
            self.delete_selected_organiser_item(window, cx);
            return;
        }
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let ids = self.organiser.selected_ids.clone();
        let before = self.workspace.organiser_items.len();
        self.workspace
            .organiser_items
            .retain(|item| !ids.contains(&item.id));
        let removed = before - self.workspace.organiser_items.len();
        if removed == 0 {
            return;
        }
        self.organiser.selected_ids.clear();
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        self.error = None;
        self.status = format!("已从 Organiser 批量删除 {removed} 条记录。");
        cx.notify();
    }

    pub(in crate::workbench) fn export_organiser_tsv(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let multi = self.organiser.selected_ids.len() > 1;
        let selected_ids = &self.organiser.selected_ids;
        let rows: Vec<_> = self
            .organiser
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .filter(|row| !multi || selected_ids.contains(&row.id))
            .cloned()
            .collect();
        let clean = |s: &str| -> String {
            s.chars()
                .map(|ch| {
                    if matches!(ch, '\t' | '\r' | '\n') {
                        ' '
                    } else {
                        ch
                    }
                })
                .collect()
        };
        let mut tsv = String::from("#\t时间\t来源\t方法\tURL\t状态码\t长度\t处理状态\t备注摘要\n");
        for entry in &rows {
            let status = entry
                .status_code
                .map_or_else(String::new, |c| c.to_string());
            let _ = writeln!(
                tsv,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                entry.number,
                clean(&format_local_time(entry.added_unix_ms)),
                clean(&entry.source_tool),
                clean(&entry.method),
                clean(&entry.url),
                status,
                entry.response_len,
                entry.workflow_status.label(),
                clean(&entry.notes_preview),
            );
        }
        let count = rows.len();
        cx.write_to_clipboard(ClipboardItem::new_string(tsv));
        self.error = None;
        self.status = format!("已复制 {count} 条 Organiser 列表 (TSV) 到剪贴板。");
        cx.notify();
    }
}
