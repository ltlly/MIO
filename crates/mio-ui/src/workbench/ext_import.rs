#[cfg(test)]
use mio_core::ExtImportReport;
use mio_core::{ExtImportPhase, ExtJobOutcome, ExtSourceInfo, Uuid};

use super::*;

mod coverage_view;
mod messages;
mod repeater;

use coverage_view::render_report;

#[derive(Clone, Copy)]
pub(in crate::workbench) enum AfterRelease {
    Dismiss,
    ChooseSource,
    Cancelled,
}

enum JobKind {
    Inspect,
    Convert {
        destination: PathBuf,
    },
    Release {
        inspection_id: Uuid,
        next: AfterRelease,
    },
}

struct Job {
    project: ProjectKey,
    id: Uuid,
    kind: JobKind,
    cancel: Option<CancellationToken>,
    cancelling: bool,
}

#[derive(Default)]
pub(super) struct ExtImport {
    pub(super) visible: bool,
    picker: Option<Uuid>,
    job: Option<Job>,
    source: Option<PathBuf>,
    result: Option<ExtJobOutcome>,
    pending_release: Option<AfterRelease>,
    progress: Option<(ExtImportPhase, u64, Option<u64>)>,
    error: Option<String>,
    report: Option<ExtSourceInfo>,
    report_query: u64,
    report_requested: bool,
    report_loading: bool,
    report_error: Option<String>,
    messages: Option<messages::Messages>,
    pub(super) browser_visible: bool,
    repeater: repeater::OpenState,
}

impl ExtImport {
    pub(super) fn picker_pending(&self) -> bool {
        self.picker.is_some()
    }

    pub(super) fn blocks_transition(&self) -> bool {
        self.job.is_some()
            || self.picker.is_some()
            || matches!(self.result, Some(ExtJobOutcome::Inspected(_)))
    }

    fn created_path(&self) -> Option<&PathBuf> {
        match &self.result {
            Some(ExtJobOutcome::Imported { path, .. }) => Some(path),
            Some(ExtJobOutcome::Failed { created_path, .. }) => created_path.as_ref(),
            _ => None,
        }
    }

    pub(super) fn message_editor(&mut self) -> Option<&mut MessageEditor> {
        self.messages.as_mut().map(|messages| &mut messages.payload)
    }

    pub(super) fn message_editor_ref(&self) -> Option<&MessageEditor> {
        self.messages.as_ref().map(|messages| &messages.payload)
    }

    pub(super) fn selected_fragment_key(&self) -> Option<(u64, Uuid)> {
        let (revision, fragment) = self.messages.as_ref()?.selected_fragment()?;
        Some((revision, fragment.summary.id))
    }

    pub(super) fn refresh_tables(&mut self, soft_wrap: bool, window: &mut Window, cx: &mut App) {
        if let Some(messages) = &mut self.messages {
            messages.refresh_tables(soft_wrap, window, cx);
        }
    }

    pub(super) fn visible_message_editor(&mut self) -> Option<&mut MessageEditor> {
        if !self.browser_visible {
            return None;
        }
        let messages = self.messages.as_mut()?;
        (!messages.items_visible && messages.detail.is_some()).then_some(&mut messages.payload)
    }
}

fn phase_label(phase: ExtImportPhase) -> &'static str {
    match phase {
        ExtImportPhase::Snapshot => "读取 外部归档项目文件",
        ExtImportPhase::Archive => "读取抓包与 Repeater 数据",
        ExtImportPhase::Index => "解析 Proxy 历史与 Repeater",
        ExtImportPhase::Publish => "保存 MIO 项目文件",
    }
}

mod part_1;
mod part_2;

#[cfg(test)]
mod tests;
