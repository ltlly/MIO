use super::*;

impl State {
    pub(super) async fn open_ext_repeater(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        source_id: Uuid,
        item_id: Uuid,
        message_id: Uuid,
        workspace_revision: u64,
    ) {
        let result = (|| {
            self.check_project(project)?;
            ensure!(
                !self.cancel.is_cancelled(),
                "应用正在退出，已取消载入归档请求"
            );
            ensure!(!self.quiesced, "正在保存并切换项目；未载入归档请求");
            let workspace = self.store.metadata().workspace()?;
            ensure!(
                !self.cancel.is_cancelled(),
                "应用正在退出，已取消载入归档请求"
            );
            ensure!(
                workspace.revision == workspace_revision,
                "工作区 revision 已变化，请重新载入归档请求"
            );
            self.store
                .ext_archive()
                .repeater_tab(source_id, item_id, message_id, &workspace, &self.cancel)
        })()
        .map_err(|error| error.to_string());
        self.emit(Event::ExtRepeaterOpened {
            project,
            operation_id,
            source_id,
            item_id,
            message_id,
            workspace_revision,
            result,
        })
        .await;
    }
}

#[cfg(test)]
mod tests;
