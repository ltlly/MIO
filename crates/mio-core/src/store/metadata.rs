mod queries;
#[cfg(test)]
mod tests;

pub(super) use queries::{project_id, workspace, workspace_for_update};

// Compatibility ceiling for the current canonical JSON encoding, not an RSS guarantee:
// <=64 MiB raw draft bytes become <=256 MiB of numeric arrays; target/title escaping can
// add <=216 MiB, leaving room for IDs and structure. Workspace::validate still enforces
// the smaller decoded field/count limits. Tests inject a tiny ceiling instead of a huge blob.
pub(in crate::store) const MAX_WORKSPACE_BYTES: usize = 512 * 1024 * 1024;

const WORKSPACE_QUERY: &str =
    "SELECT typeof(value),CASE WHEN typeof(value)='blob' THEN length(value) END,
        CASE WHEN typeof(value)='blob' THEN
            CASE WHEN length(value)<=?1 THEN value END END
     FROM main.metadata WHERE key='workspace'";
