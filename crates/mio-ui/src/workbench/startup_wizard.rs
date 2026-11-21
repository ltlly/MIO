use std::collections::HashSet;

use gpui_kit::{WeakEntity, component::table::Column};
use mio_core::{RecentProject, StartupDefaultConfig, Uuid};

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WizardStep {
    Step1,
    Step2,
    Starting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProjectMode {
    Temporary,
    NewOnDisk,
    OpenExisting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfigChoice {
    Defaults,
    Project,
    ConfigFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecentSortColumn {
    Name,
    File,
    LastAccessed,
}

pub(super) struct PendingRecentSave {
    pub(super) operation_id: Uuid,
    pub(super) expected_revision: u64,
    pub(super) label: &'static str,
    pub(super) removed_path: Option<PathBuf>,
    pub(super) previous_paths: Vec<PathBuf>,
}

pub(super) struct RecentProjectsDelegate {
    owner: Option<WeakEntity<Workbench>>,
    columns: [Column; 3],
    rows: Vec<RecentProject>,
    missing_paths: HashSet<PathBuf>,
    selected_path: Option<PathBuf>,
    sort_column: RecentSortColumn,
    ascending: bool,
}

mod actions;
mod lifecycle;
mod pickers;
mod render_controls;
mod render_shell;
mod render_steps;
mod state;
#[cfg(test)]
mod tests_basic;
#[cfg(test)]
mod tests_edge;

use render_controls::*;
use state::*;

pub(super) struct StartupWizard {
    pub(super) active: bool,
    pub(super) step: WizardStep,
    pub(super) mode: ProjectMode,
    pub(super) new_name_input: Entity<InputState>,
    pub(super) new_file_input: Entity<InputState>,
    pub(super) open_file_input: Entity<InputState>,
    pub(super) config_file_input: Entity<InputState>,
    pub(super) recent_table: Entity<TableState<RecentProjectsDelegate>>,
    pub(super) config_list_scroll: ScrollHandle,
    pub(super) trust_project_file: bool,
    pub(super) pause_automated_tasks: bool,
    pub(super) config_choice: ConfigChoice,
    pub(super) config_choice_customized: bool,
    pub(super) default_in_future: bool,
    pub(super) disable_extensions: bool,
    pub(super) disable_extensions_customized: bool,
    pub(super) cancel_confirm_open: bool,
    pub(super) new_file_custom_dir: Option<PathBuf>,
    pub(super) new_file_customized: bool,
    pub(super) syncing_inputs: bool,
    pub(super) picker: Option<Uuid>,
    pub(super) pending_preference_save: Option<(Uuid, u64, StartupDefaultConfig, bool)>,
    pub(super) pending_recent_save: Option<PendingRecentSave>,
    pub(super) awaiting_start: bool,
    pub(super) initial_project_ready: bool,
    pub(super) cli_initial_path: Option<PathBuf>,
    pub(super) cli_retry_path: Option<PathBuf>,
    pub(super) last_start_failed: bool,
    pub(super) error: Option<String>,
    _subscriptions: Vec<Subscription>,
}
