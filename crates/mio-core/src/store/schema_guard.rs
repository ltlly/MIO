mod foreign_keys;
#[cfg(test)]
mod tests;

mod tables;

pub(super) use foreign_keys::{validate_intruder_discard, validate_update_dependencies};
pub(super) use tables::validate;

const TABLES: &[(i32, &str, &[&str])] = &[
    (26, "site_map_entries", &["id", "owner", "revision", "row", "detail"]),
    (1, "metadata", &["key", "value"]),
    (
        1,
        "flows",
        &[
            "id", "revision", "started", "url", "method", "state", "summary",
        ],
    ),
    (1, "messages", &["flow", "side", "head", "body"]),
    (
        2,
        "websocket_messages",
        &[
            "ordinal",
            "id",
            "revision",
            "flow",
            "sequence",
            "direction",
            "kind",
            "payload_bytes",
            "observed",
        ],
    ),
    (2, "websocket_payloads", &["message", "body"]),
    (
        19,
        "websocket_connections",
        &["id", "flow", "listener_port"],
    ),
    (20, "websocket_notes", &["message", "revision", "text"]),
    (22, "ext_http_notes", &["item", "revision", "text"]),
    (23, "ext_http_highlights", &["item", "revision", "color"]),
    (24, "websocket_originals", &["message", "body"]),
    (
        21,
        "repeater_history",
        &[
            "tab",
            "archive_boundary",
            "ordinal",
            "flow",
            "draft_meta",
            "raw",
        ],
    ),
    (
        3,
        "ext_sources",
        &[
            "id",
            "source_bytes",
            "written_bytes",
            "next_chunk",
            "complete",
            "report",
        ],
    ),
    (
        3,
        "ext_source_chunks",
        &["source", "ordinal", "raw_bytes", "codec", "data"],
    ),
    (
        3,
        "ext_items",
        &[
            "id",
            "source",
            "collection",
            "item_index",
            "object_address",
            "state",
            "metadata",
        ],
    ),
    (3, "ext_messages", &["id", "item", "field_id", "summary"]),
    (
        4,
        "intruder_runs",
        &[
            "id",
            "tab",
            "revision",
            "spec_revision",
            "total",
            "dispatched",
            "completed",
            "state",
            "error",
            "started",
            "spec_meta",
            "template",
        ],
    ),
    (
        4,
        "intruder_payloads",
        &["run", "set_index", "ordinal", "value"],
    ),
    (4, "intruder_results", &["run", "ordinal", "flow", "error"]),
];
