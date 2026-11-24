use std::time::Instant;

use mio_core::{
    FinishedAttackClose, RunningAttackClose, UserDisplaySettings, UserMessageEditorSettings,
    UserMiscSettings, UserPreferences, Uuid,
};

use super::*;

mod actions;
mod receive;

#[derive(Clone, Copy)]
pub(in crate::workbench) enum RememberCloseChoice {
    Running(RunningAttackClose),
    Finished(FinishedAttackClose),
}

#[derive(Clone, Copy)]
pub(super) enum SaveOrigin {
    Settings,
    Workbench,
    Remember(RememberCloseChoice),
    Tls,
}

#[derive(Clone)]
pub(super) struct PendingSave {
    operation: Uuid,
    expected_revision: u64,
    running: RunningAttackClose,
    finished: FinishedAttackClose,
    display: UserDisplaySettings,
    message_editor: UserMessageEditorSettings,
    misc: UserMiscSettings,
    certificates: Vec<Vec<u8>>,
    origin: SaveOrigin,
    deadline: (Instant, gpui_kit::BackgroundExecutor),
}

#[derive(Clone, Default)]
pub(in crate::workbench) struct UserPreferencesUi {
    pub(in crate::workbench) confirmed: Option<UserPreferences>,
    pub(in crate::workbench) running: RunningAttackClose,
    pub(in crate::workbench) finished: FinishedAttackClose,
    pub(in crate::workbench) display: UserDisplaySettings,
    pub(in crate::workbench) message_editor: UserMessageEditorSettings,
    pub(in crate::workbench) misc: UserMiscSettings,
    pub(in crate::workbench) load_error: Option<String>,
    pub(in crate::workbench) error: Option<String>,
    pub(in crate::workbench) remember_error: Option<String>,
    pub(in crate::workbench) status: String,
    query: Option<(Uuid, Instant, gpui_kit::BackgroundExecutor)>,
    pending: Option<PendingSave>,
    pub(in crate::workbench) retain_selection: bool,
    pub(in crate::workbench) retain_workbench: bool,
}
