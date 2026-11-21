use std::collections::HashSet;

use super::*;

const MAX_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

impl State {
    pub(super) async fn read_history_transfer(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        workspace_revision: u64,
        entries: &[HistoryTransferEntry],
        cancel: &CancellationToken,
    ) {
        self.store
            .cancel_queries_with_operation(self.cancel.clone(), cancel.clone());
        let result = self
            .history_transfer_proposals(project, operation_id, workspace_revision, entries, cancel)
            .map_err(|error| error.to_string());
        self.store.cancel_queries_with(self.cancel.clone());
        self.emit(Event::HistoryTransferRead {
            project,
            operation_id,
            workspace_revision,
            result,
        })
        .await;
    }

    fn history_transfer_proposals(
        &self,
        project: ProjectKey,
        operation_id: Uuid,
        workspace_revision: u64,
        entries: &[HistoryTransferEntry],
        cancel: &CancellationToken,
    ) -> Result<Vec<(HistoryEntryId, WorkTab)>> {
        self.check_project(project)?;
        ensure!(
            !cancel.is_cancelled() && !self.cancel.is_cancelled(),
            "历史请求转送读取已取消"
        );
        ensure!(!operation_id.is_nil(), "历史请求转送操作 ID 不能为空");
        ensure!(!self.quiesced, "项目正在切换；未读取转送请求");
        ensure!(
            !entries.is_empty() && entries.len() <= 500,
            "历史请求转送需要 1–500 条记录"
        );
        ensure!(
            self.store.metadata().workspace()?.revision == workspace_revision,
            "工作区 revision 已变化；未读取转送请求"
        );
        let mut seen = HashSet::with_capacity(entries.len());
        for entry in entries {
            ensure!(seen.insert(entry.id()), "历史请求转送记录重复；未创建标签");
            match *entry {
                HistoryTransferEntry::SiteMap { entry_id, revision, .. } => {
                    ensure!(!entry_id.is_nil() && revision > 0, "站点地图转送身份无效");
                }
                HistoryTransferEntry::Live {
                    flow_id, revision, ..
                } => {
                    ensure!(
                        !flow_id.is_nil() && revision > 0,
                        "历史请求转送 flow ID 或 revision 无效"
                    );
                }
                HistoryTransferEntry::External {
                    source_id,
                    item_id,
                    message_id,
                } => {
                    ensure!(
                        !source_id.is_nil() && !item_id.is_nil() && !message_id.is_nil(),
                        "历史请求转送归档身份无效"
                    );
                }
            }
        }
        let mut proposals = Vec::with_capacity(entries.len());
        let mut bytes = 0usize;
        let empty_workspace = Workspace::default();
        for entry in entries {
            ensure!(
                !cancel.is_cancelled() && !self.cancel.is_cancelled(),
                "历史请求转送读取已取消"
            );
            let tab = match *entry {
                HistoryTransferEntry::SiteMap { row_id, entry_id, revision } => {
                    let detail = self.store.site_map().read_entry(entry_id, revision)?;
                    ensure!(detail.row_id == row_id, "站点地图转送来源身份不一致");
                    let mut tab = WorkTab::from_request(Some(&detail.record.request), "1".into());
                    tab.draft.raw = detail.request_raw;
                    tab
                }
                HistoryTransferEntry::Live {
                    flow_id,
                    revision,
                    original,
                } => {
                    let record = self.read_flow(flow_id)?;
                    ensure!(
                        record.summary.id == flow_id && record.summary.revision == revision,
                        "所选历史请求 revision 已变化；未创建标签"
                    );
                    let before = if original {
                        self.original_flow(flow_id)?
                    } else {
                        OriginalMessages::default()
                    };
                    let request = before.request.as_ref().unwrap_or(&record.request);
                    WorkTab::from_request(Some(request), "1".into())
                }
                HistoryTransferEntry::External {
                    source_id,
                    item_id,
                    message_id,
                } => {
                    // Read only the immutable summary before allocating the selected raw request.
                    let message = self
                        .store
                        .ext_archive()
                        .message_fragment(message_id, 0, 0)?
                        .summary;
                    ensure!(
                        message.source_id == source_id
                            && message.item_id == item_id
                            && matches!(message.collection.as_str(), "root.f1.f0" | "root.f1.f1")
                            && ((message.field_id == 15 && message.role == "request")
                                || (message.field_id == 17 && message.role == "edited_request")),
                        "所选归档消息不是该 HTTP 历史条目的原始或编辑请求"
                    );
                    let length = usize::try_from(message.raw_bytes).context("历史请求大小溢出")?;
                    ensure!(
                        bytes
                            .checked_add(length)
                            .is_some_and(|total| total <= MAX_TRANSFER_BYTES),
                        "历史转送请求总量超过 64 MiB；未创建标签"
                    );
                    self.store.ext_archive().repeater_tab(
                        source_id,
                        item_id,
                        message_id,
                        &empty_workspace,
                        cancel,
                    )?
                }
            };
            ensure!(
                tab.draft.raw.len() <= crate::http::MAX_BODY + 256 * 1024
                    && tab.draft.target.len() <= 64 * 1024
                    && tab.draft.revision == 1
                    && tab.archive_response.is_none()
                    && tab.last_flow.is_none()
                    && tab.history.is_none()
                    && tab.group.is_none(),
                "历史转送提案超过草稿限额或包含不属于请求的状态"
            );
            bytes = bytes
                .checked_add(tab.draft.raw.len())
                .context("历史请求总量溢出")?;
            ensure!(
                bytes <= MAX_TRANSFER_BYTES,
                "历史转送请求总量超过 64 MiB；未创建标签"
            );
            proposals.push((entry.id(), tab));
        }
        ensure!(
            !cancel.is_cancelled() && !self.cancel.is_cancelled(),
            "历史请求转送读取已取消"
        );
        ensure!(
            self.store.metadata().workspace()?.revision == workspace_revision,
            "工作区 revision 已变化；未应用转送请求"
        );
        Ok(proposals)
    }
}

#[cfg(test)]
mod tests;
