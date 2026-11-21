use anyhow::{Context, Result, bail, ensure};
use mio_plugin_api::{FlowRecord, Header, HttpRequest, HttpResponse, HttpVersion};
use regex::{Regex, RegexBuilder};
use url::{Position, Url};

use crate::{
    InspectorData, InspectorItem, InspectorMutation, InspectorSection, MatchAndReplaceRule,
    MatchReplaceTarget,
};

mod compression;
pub mod mio_script;
pub use mio_script::*;
pub use compression::{
    compress_deflate_bytes, compress_gzip_bytes, decompress_brotli_bytes, decompress_deflate_bytes,
    decompress_gzip_bytes,
};
pub(crate) use compression::{UnpackError, unpack_request, unpack_response};
mod editor_replace;
mod extra_conditions;
pub mod html_extract;
pub(crate) use extra_conditions::infer_history_display_mime_type;

pub(crate) const MAX_HEAD: usize = 128 * 1024;
pub(crate) const MAX_BODY: usize = 16 * 1024 * 1024;

mod inspector;
mod intercept;
mod intercept_rules;
mod intercept_rules2;
mod match_replace;
mod parse;
mod parse_message;
mod pretty;
mod proxy_transforms;
pub mod raw_head;
pub mod request_codec;
mod response_len;
mod to_raw;

pub use editor_replace::{expand_search_replace_all, expand_search_replace_one};
pub use inspector::{
    DecodedInspectorSelection, InspectorSelectionCodec, apply_inspector_mutation,
    apply_inspector_response_mutation, decode_inspector_selection, detect_request_line_version,
    encode_inspector_selection, inspect_http_messages, rewrite_request_line_version,
};
use intercept::InterceptParameters;
pub use intercept::build_flow_transform;
pub(crate) use intercept::{
    build_flow_transform_with_scope_and_provenance, build_intercept_filter,
    build_websocket_intercept_filter, validate_proxy_interception,
    validate_proxy_interception_shape,
};
use intercept_rules::intercept_rule_matches;
use intercept_rules2::{
    check_intercept_control, intercept_control_error, intercept_cookie_pairs,
    intercept_pairs_match, intercept_request_parameters, request_has_intercept_parameters,
    validate_intercept_headers,
};
pub use match_replace::{apply_match_and_replace, test_match_and_replace_rule};
#[cfg(test)]
use match_replace::compile_rule_regex;
use match_replace::replace_text_with_rule;
#[cfg(test)]
use match_replace::{append_replacement, apply_header_rule};
use match_replace::{apply_single_rule, valid_header_value_bytes};
pub(crate) use parse::{body, head, is_chunked, token};
pub(crate) use parse_message::{parse_imported_request, parse_imported_response};
pub use parse_message::{
    absolute_request_target, copy_url_for_request, curl_command_for_request,
    curl_command_for_request_with_options, http_history_path_and_query, parse_repeater_request,
    parse_request, parse_response, redirect_location_url, resolve_redirect_request,
    resolve_redirect_request_with_options, validate_target_origin,
};
pub use pretty::{PrettyMessage, format_pretty_message, format_pretty_message_with_line_map};

pub fn decode_http_body(fields: &[Header], raw: &[u8]) -> Result<(Vec<u8>, Vec<Header>)> {
    body(fields, raw)
}

pub fn is_chunked_headers(fields: &[Header]) -> bool {
    is_chunked(fields)
}
#[allow(unused_imports)]
pub use proxy_transforms::{
    build_ssl_pass_through_matcher, build_websocket_transform,
    build_websocket_transform_with_scope, validate_ssl_and_ws_rules,
};
pub use raw_head::{
    HttpHeaderSpan, ParsedHttpResponseHead, extract_content_type_essence_and_charset,
    extract_http_header_charset, filter_http_headers_bytes, find_http_body_offset,
    parse_http_head_info, parse_http_header_spans, parse_http_response_parts,
};
pub use request_codec::{
    BodyEncodingTarget, change_http_body_encoding_bytes, parse_clipboard_url_as_request,
    set_http_request_method_bytes, toggle_http_request_method_bytes,
};
pub use response_len::intruder_response_len;
#[cfg(test)]
pub use response_len::response_raw_len;
pub use to_raw::{request_to_raw, response_to_raw};

#[cfg(test)]
#[path = "http/tests/core_features.rs"]
mod core_features_tests;
#[cfg(test)]
#[path = "http/tests/core_inspector.rs"]
mod core_inspector_tests;
#[cfg(test)]
#[path = "http/tests/core_roundtrip.rs"]
mod core_roundtrip_tests;
#[cfg(test)]
#[path = "http/tests/inspector_structured.rs"]
mod inspector_structured_tests;
#[cfg(test)]
#[path = "http/tests/pretty_xml_html.rs"]
mod pretty_xml_html_tests;
#[cfg(test)]
#[path = "http/tests.rs"]
mod request_target_tests;
