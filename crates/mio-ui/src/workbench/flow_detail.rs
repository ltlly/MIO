use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_flow_detail_error(
        &mut self,
        project: ProjectKey,
        selection_revision: u64,
        flow_id: FlowId,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project) {
            return;
        }
        if self.target_pane.pending_reads.get(&selection_revision) == Some(&target::TargetRead::Live(flow_id)) {
            self.target_pane.pending_reads.remove(&selection_revision);
            if self.target_pane.selected_id == Some(HistoryEntryId::Live(flow_id)) {
                self.target_pane.site_map_error = Some(message.to_owned());
            }
            return;
        }
        if self.extra_tools.logger_pending_read == Some((project, selection_revision, flow_id)) {
            self.extra_tools.logger_pending_read = None;
            self.clear_logger_pending_batch();
            self.fail(format!("无法读取 Logger 消息：{message}"), cx);
            return;
        }
        if self.history_detail_revision == selection_revision
            && self.selected_flow == Some(flow_id)
            && self.detail.is_none()
        {
            self.detail_error = Some(message.to_owned());
            self.fail(format!("无法读取消息：{message}"), cx);
            return;
        }
        let Some(tab_id) = self.repeater_reads.get(&selection_revision).copied() else {
            return;
        };
        if !self
            .workspace
            .tabs
            .iter()
            .any(|tab| tab.id == tab_id && tab.last_flow == Some(flow_id))
        {
            return;
        }
        self.repeater_reads.remove(&selection_revision);
        if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
            editors.result = Some(format!("无法读取已保存的响应：{message}"));
        }
        self.fail(format!("无法读取 Repeater 已保存的响应：{message}"), cx);
    }
}

#[cfg(test)]
mod tests;
