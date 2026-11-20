use std::time::{SystemTime, UNIX_EPOCH};

pub mod cookie_jar;
pub mod macros;
pub mod rules;
pub mod runtime;
mod validation;

pub use cookie_jar::{
    apply_cookie_jar_to_request, ingest_set_cookie_headers, matching_cookie_entries,
    set_or_replace_request_cookie, set_request_cookie,
};
pub use macros::{ExtractedParam, execute_macro};
pub use rules::{
    apply_rule_sync_action, replace_request_text, rewrite_url_query_param, rule_matches_request,
    set_request_header,
};
pub use runtime::SessionRuntime;
pub use validation::{
    normalize_cookie_domain, valid_cookie_name, valid_cookie_value, valid_macro_step_url,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionExecError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for SessionExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("会话规则执行已取消"),
            Self::Failed(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for SessionExecError {}

pub(crate) fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests;
