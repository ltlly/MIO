mod builtins;
mod engine;
mod eval;
mod highlight;
#[cfg(test)]
mod tests;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::MAX_BODY;
use crate::WebSocketDirection;

pub const DEFAULT_REQUEST_SCRIPT_TEMPLATE: &str =
    "request.headers[\"X-Forwarded-For\"] = \"127.0.0.1\";\nrequest";
pub const DEFAULT_RESPONSE_SCRIPT_TEMPLATE: &str =
    "response.headers[\"X-Frame-Options\"] = \"DENY\";\nresponse";
pub const DEFAULT_HTTP_FILTER_SCRIPT_TEMPLATE: &str =
    "request.method == \"POST\" && response.status == 200";
pub const DEFAULT_WS_FILTER_SCRIPT_TEMPLATE: &str =
    "message.direction == \"c2s\" && message.payload.contains(\"token\")";

pub use engine::validate_filter_script;
pub use eval::{
    CustomActionOutcome, FilterScriptInput, HighlightFilterState, WsFilterScriptInput,
    convert_http_filter_to_script, convert_ws_filter_to_script, eval_custom_action_script,
    eval_custom_action_with_mutation, eval_custom_scan_check_script, eval_http_filter_script_input,
    eval_ws_filter_script_input, execute_http_match_replace_script,
};
pub(super) use eval::version_from_script;
pub use highlight::script_keyword_ranges;

/// Compatibility wrapper around [`eval_http_filter_script_input`] with default scope.
pub fn eval_http_filter_script(
    script: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    length: u64,
    request_raw: Option<&[u8]>,
    response_raw: Option<&[u8]>,
    notes: &str,
    highlighted: bool,
    listener_port: Option<u16>,
) -> Result<bool> {
    eval_http_filter_script_input(
        script,
        &FilterScriptInput {
            method,
            url,
            status,
            length,
            request_raw,
            response_raw,
            notes,
            highlight: HighlightFilterState::Boolean(highlighted),
            listener_port,
            in_scope: false,
            cancel: None,
        },
    )
}

/// Compatibility wrapper around [`eval_http_filter_script_input`] with boolean highlight and cancel token.
#[cfg(test)]
pub(crate) fn eval_http_filter_script_with_scope_and_cancel(
    script: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    length: u64,
    request_raw: Option<&[u8]>,
    response_raw: Option<&[u8]>,
    notes: &str,
    highlighted: bool,
    listener_port: Option<u16>,
    in_scope: bool,
    cancel: &CancellationToken,
) -> Result<bool> {
    eval_http_filter_script_input(
        script,
        &FilterScriptInput {
            method,
            url,
            status,
            length,
            request_raw,
            response_raw,
            notes,
            highlight: HighlightFilterState::Boolean(highlighted),
            listener_port,
            in_scope,
            cancel: Some(cancel),
        },
    )
}

/// Compatibility wrapper around [`eval_http_filter_script_input`] with highlight color code and cancel token.
pub(crate) fn eval_http_filter_script_with_highlight_color_and_cancel(
    script: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    length: u64,
    request_raw: Option<&[u8]>,
    response_raw: Option<&[u8]>,
    notes: &str,
    highlight_color: u8,
    listener_port: Option<u16>,
    in_scope: bool,
    cancel: &CancellationToken,
) -> Result<bool> {
    eval_http_filter_script_input(
        script,
        &FilterScriptInput {
            method,
            url,
            status,
            length,
            request_raw,
            response_raw,
            notes,
            highlight: HighlightFilterState::ColorCode(highlight_color),
            listener_port,
            in_scope,
            cancel: Some(cancel),
        },
    )
}

/// Compatibility wrapper around [`eval_ws_filter_script_input`] with default scope.
pub fn eval_ws_filter_script(
    script: &str,
    url: &str,
    direction: WebSocketDirection,
    payload: &[u8],
    edited: bool,
    notes: &str,
    listener_port: Option<u16>,
) -> Result<bool> {
    eval_ws_filter_script_input(
        script,
        &WsFilterScriptInput {
            url,
            direction,
            payload,
            edited,
            notes,
            listener_port,
            in_scope: false,
        },
    )
}

/// Compatibility wrapper around [`eval_ws_filter_script_input`] with explicit `in_scope`.
pub(crate) fn eval_ws_filter_script_with_scope(
    script: &str,
    url: &str,
    direction: WebSocketDirection,
    payload: &[u8],
    edited: bool,
    notes: &str,
    listener_port: Option<u16>,
    in_scope: bool,
) -> Result<bool> {
    eval_ws_filter_script_input(
        script,
        &WsFilterScriptInput {
            url,
            direction,
            payload,
            edited,
            notes,
            listener_port,
            in_scope,
        },
    )
}
