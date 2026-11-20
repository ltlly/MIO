use super::*;

impl State {
    pub(crate) fn read_websocket_notes(
        &self,
        project: ProjectKey,
        id: Uuid,
    ) -> Result<WebSocketNotes> {
        self.check_project(project)?;
        ensure!(!id.is_nil(), "WebSocket 消息 ID 不能为空");
        if self.store.websocket_ordinal(id)?.is_none() {
            if self
                .websocket_recovery
                .iter()
                .any(|capture| capture.record.id == id)
            {
                return Ok(WebSocketNotes::default());
            }
            if let Some(notes) = super::ext_archive::read_ext_websocket_notes(&self.store, id)? {
                return Ok(notes);
            }
        }
        self.store.websocket_notes(project, id)
    }

    pub(crate) fn set_websocket_notes(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        id: Uuid,
        expected: u64,
        text: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<WebSocketNotes, WebSocketNotesError> {
        let ready = self.check_project(project).and_then(|()| {
            ensure!(
                !operation_id.is_nil() && !id.is_nil(),
                "WebSocket 备注操作或消息 ID 不能为空"
            );
            ensure!(!cancel.is_cancelled(), "WebSocket 备注修改已取消");
            self.require_saved()
        });
        if let Err(error) = ready {
            return Err(WebSocketNotesError {
                message: error.to_string(),
                stored: if project == self.project {
                    self.store.websocket_notes(project, id).ok()
                } else {
                    None
                },
            });
        }
        if self.store.websocket_ordinal(id).ok().flatten().is_none()
            && let Ok(Some(notes)) = super::ext_archive::read_ext_websocket_notes(&self.store, id)
        {
            if expected == notes.revision && text == notes.text {
                return Ok(notes);
            }
            return Err(WebSocketNotesError {
                message: "外部归档 WebSocket 源备注为只读；未修改源归档".into(),
                stored: Some(notes),
            });
        }
        self.store
            .set_websocket_notes(project, id, expected, text, cancel)
    }
}
