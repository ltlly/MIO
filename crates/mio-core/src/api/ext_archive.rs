use std::path::PathBuf;

use anyhow::{Result, ensure};
use mio_plugin_api::{HttpVersion, Uuid, WebSocketDirection, WebSocketKind};
use serde::{Deserialize, Serialize};

pub use crate::ext_archive::import::extract_embedded_ext_project_config;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtCategory {
    ProxyHttpHistory,
    ProxyWebSocketHistory,
    TargetSiteMapAndScope,
    RepeaterWorkspaceAndHistory,
    EmbeddedProjectConfig,
    UnmappedSlots,
}

impl ExtCategory {
    pub const ALL: [Self; 6] = [
        Self::ProxyHttpHistory,
        Self::ProxyWebSocketHistory,
        Self::TargetSiteMapAndScope,
        Self::RepeaterWorkspaceAndHistory,
        Self::EmbeddedProjectConfig,
        Self::UnmappedSlots,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ProxyHttpHistory => "Proxy HTTP History",
            Self::ProxyWebSocketHistory => "Proxy WebSocket History",
            Self::TargetSiteMapAndScope => "Target Site Map & Scope",
            Self::RepeaterWorkspaceAndHistory => "Repeater Workspace & History",
            Self::EmbeddedProjectConfig => "Embedded Project Config",
            Self::UnmappedSlots => "Unmapped Slots",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtObjectTypeCount {
    pub object_type: u8,
    pub subtype: u8,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtCategoryCoverage {
    pub category: ExtCategory,
    pub label: String,
    pub collection_paths: Vec<String>,
    pub total_slots: Option<u64>,
    pub non_null_slots: u64,
    pub recovered_items: u64,
    pub recovered_messages: u64,
    pub unmapped_slots: u64,
    pub observed_types: Vec<ExtObjectTypeCount>,
    pub mapped_fields: Vec<String>,
    pub unmapped_fields: Vec<String>,
    pub state: String,
    pub unmapped_reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtCoverageReport {
    pub schema_floor: u16,
    pub schema_revision: u16,
    pub source_bytes: u64,
    pub complete_archive: bool,
    pub categories: Vec<ExtCategoryCoverage>,
}

impl ExtCoverageReport {
    pub fn category(&self, category: ExtCategory) -> Option<&ExtCategoryCoverage> {
        self.categories
            .iter()
            .find(|entry| entry.category == category)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtRepeaterSettingsPatch {
    pub unpack_compressed_responses: Option<bool>,
    pub update_content_length: Option<bool>,
    pub normalize_http1_line_endings: Option<bool>,
    pub follow_redirects: Option<crate::RepeaterRedirectMode>,
    pub request_timeout_secs: Option<u64>,
}

impl ExtRepeaterSettingsPatch {
    pub fn is_empty(&self) -> bool {
        self.unpack_compressed_responses.is_none()
            && self.update_content_length.is_none()
            && self.normalize_http1_line_endings.is_none()
            && self.follow_redirects.is_none()
            && self.request_timeout_secs.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtEmbeddedProjectConfig {
    pub scope: Option<crate::TargetScope>,
    pub match_and_replace_rules: Option<Vec<crate::MatchAndReplaceRule>>,
    pub repeater: Option<ExtRepeaterSettingsPatch>,
    pub sessions: Option<crate::SessionSettings>,
}

impl ExtEmbeddedProjectConfig {
    pub fn is_empty(&self) -> bool {
        self.scope.is_none()
            && self.match_and_replace_rules.is_none()
            && self
                .repeater
                .as_ref()
                .is_none_or(ExtRepeaterSettingsPatch::is_empty)
            && self.sessions.is_none()
    }

    pub fn apply_to_workspace(&self, workspace: &mut crate::Workspace) -> Result<bool> {
        let mut changed = false;
        if let Some(repeater) = &self.repeater {
            if let Some(timeout) = repeater.request_timeout_secs {
                ensure!(
                    (1..=3600).contains(&timeout),
                    "Repeater 请求超时必须在 1 到 3600 秒之间"
                );
                if workspace.repeater_settings.request_timeout_secs != timeout {
                    workspace.repeater_settings.request_timeout_secs = timeout;
                    changed = true;
                }
            }
            if let Some(unpack) = repeater.unpack_compressed_responses
                && workspace.repeater_settings.unpack_compressed_responses != unpack
            {
                workspace.repeater_settings.unpack_compressed_responses = unpack;
                changed = true;
            }
            if let Some(update_cl) = repeater.update_content_length
                && workspace.repeater_settings.update_content_length != update_cl
            {
                workspace.repeater_settings.update_content_length = update_cl;
                changed = true;
            }
            if let Some(normalize) = repeater.normalize_http1_line_endings
                && workspace.repeater_settings.normalize_http1_line_endings != normalize
            {
                workspace.repeater_settings.normalize_http1_line_endings = normalize;
                changed = true;
            }
            if let Some(redirects) = repeater.follow_redirects
                && workspace.repeater_settings.follow_redirects != redirects
            {
                workspace.repeater_settings.follow_redirects = redirects;
                changed = true;
            }
        }
        if let Some(scope) = &self.scope {
            scope.validate()?;
            if &workspace.scope != scope {
                workspace.scope = scope.clone();
                changed = true;
            }
        }
        if let Some(rules) = &self.match_and_replace_rules
            && &workspace.match_and_replace_rules != rules
        {
            workspace.match_and_replace_rules = rules.clone();
            changed = true;
        }
        if let Some(sessions) = &self.sessions {
            sessions.validate()?;
            if &workspace.sessions != sessions {
                workspace.sessions = sessions.clone();
                changed = true;
            }
        }
        if changed {
            workspace.validate()?;
        }
        Ok(changed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtEmbeddedConfigStatus {
    Absent,
    Null,
    Extracted,
    Invalid { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtEmbeddedConfigReport {
    pub status: ExtEmbeddedConfigStatus,
    pub config: ExtEmbeddedProjectConfig,
    pub skipped_unsafe_sections: Vec<String>,
    pub unmapped_keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtResponseRef {
    pub source_id: Uuid,
    pub item_id: Uuid,
    pub message_id: Uuid,
    pub draft_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtCollectionReport {
    pub path: String,
    pub logical_items: Option<u64>,
    pub indexed_items: u64,
    pub message_count: u64,
    /// Missing/null/unavailable/failed are not interchangeable with an empty collection.
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtImportReport {
    pub schema_floor: u16,
    pub schema_revision: u16,
    pub source_bytes: u64,
    pub indexed_items: u64,
    pub indexed_messages: u64,
    pub collections: Vec<ExtCollectionReport>,
    /// Zero-based source Repeater tab slot, when the schema-specific field is verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeater_active_tab: Option<u64>,
    pub gaps: Vec<String>,
    /// The whole source is retained, not a claim that every category is semantically mapped.
    pub complete_archive: bool,
}

impl ExtImportReport {
    pub fn category_coverage(&self) -> Vec<ExtCategoryCoverage> {
        crate::ext_archive::import::category_coverage(self)
    }

    pub fn coverage_report(&self) -> ExtCoverageReport {
        ExtCoverageReport {
            schema_floor: self.schema_floor,
            schema_revision: self.schema_revision,
            source_bytes: self.source_bytes,
            complete_archive: self.complete_archive,
            categories: self.category_coverage(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtSourceInfo {
    pub id: Uuid,
    pub report: ExtImportReport,
}

#[derive(Clone, Debug)]
pub struct ExtInspection {
    pub id: Uuid,
    pub source_name: String,
    pub report: ExtImportReport,
}

impl ExtInspection {
    pub fn category_coverage(&self) -> Vec<ExtCategoryCoverage> {
        self.report.category_coverage()
    }

    pub fn coverage_report(&self) -> ExtCoverageReport {
        self.report.coverage_report()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtImportPhase {
    Snapshot,
    Archive,
    Index,
    Publish,
}

#[derive(Clone, Debug)]
pub enum ExtJobOutcome {
    Inspected(ExtInspection),
    Imported {
        path: PathBuf,
        report: ExtImportReport,
        total_bytes: u64,
    },
    Released {
        inspection_id: Uuid,
    },
    Cancelled,
    Failed {
        message: String,
        /// Atomic publication may succeed before a directory-sync or reopen error.
        created_path: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtItem {
    pub id: Uuid,
    pub source_id: Uuid,
    pub collection: String,
    pub index: u64,
    pub object_address: Option<u64>,
    pub state: String,
    /// Descriptors and individual field observations; never a guessed object serialization.
    pub metadata_json: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtItemSummary {
    pub id: Uuid,
    pub source_id: Uuid,
    pub collection: String,
    pub index: u64,
    pub object_address: Option<u64>,
    pub state: String,
    pub message_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtItemPage {
    pub total: usize,
    /// Includes empty, null and failed slots; metadata and raw messages are loaded on selection.
    pub rows: Vec<ExtItemSummary>,
    pub next_offset: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtItemDetail {
    pub item: ExtItem,
    /// A bounded page from this item only; current drafts are never filled from history.
    pub messages: ExtMessagePage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtMessageSummary {
    pub id: Uuid,
    pub source_id: Uuid,
    pub item_id: Uuid,
    pub collection: String,
    pub item_index: u64,
    pub role: String,
    pub field_id: u8,
    /// Absolute byte offset in the archived source, not an address in a live external file.
    pub raw_offset: u64,
    pub raw_bytes: u64,
    /// Relative to raw_offset; absent if a complete valid HTTP head was not decoded.
    pub body_offset: Option<u64>,
    /// The serialized start-line declaration, not evidence of negotiated transport protocol.
    pub declared_version: Option<HttpVersion>,
    pub method: Option<String>,
    pub request_target: Option<String>,
    pub status: Option<u16>,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtMessagePage {
    pub total: usize,
    /// Complete summaries within the byte budget; a page can be shorter than the requested limit.
    pub rows: Vec<ExtMessageSummary>,
    /// The first unread row, never an assumed fixed page size.
    pub next_offset: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtMessageFragment {
    pub summary: ExtMessageSummary,
    /// Relative to the raw message; at most 64 KiB is returned per selection/page.
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtWebSocketRow {
    pub item: ExtItemSummary,
    pub payload: Option<ExtMessageSummary>,
    pub edited_payload: Option<ExtMessageSummary>,
    pub connection_id: Option<u64>,
    pub direction: Option<WebSocketDirection>,
    pub kind: Option<WebSocketKind>,
    pub url: Option<String>,
    pub observed_unix_ms: Option<i64>,
    pub notes: Option<String>,
    pub highlight: Option<u8>,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtWebSocketPage {
    pub total: usize,
    pub rows: Vec<ExtWebSocketRow>,
    pub next_offset: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtWebSocketFragment {
    pub row: ExtWebSocketRow,
    pub summary: ExtMessageSummary,
    pub edited: bool,
    pub offset: u64,
    pub bytes: Vec<u8>,
}
