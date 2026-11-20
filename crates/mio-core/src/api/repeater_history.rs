use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeaterHistorySource {
    pub source_id: Uuid,
    pub parent_item_id: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeaterHistoryEntryId {
    Archive(u64),
    Live(FlowId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeaterHistoryCursor {
    pub entry: RepeaterHistoryEntryId,
    pub draft_revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeaterHistory {
    pub source: Option<RepeaterHistorySource>,
    pub cursor: Option<RepeaterHistoryCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepeaterHistoryEntry {
    pub id: RepeaterHistoryEntryId,
    pub position: u64,
    pub method: Option<String>,
    pub target: Option<String>,
    pub status: Option<u16>,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepeaterHistoryPage {
    pub total: u64,
    pub start: u64,
    pub current_position: Option<u64>,
    pub rows: Vec<RepeaterHistoryEntry>,
    pub next_offset: Option<u64>,
}
