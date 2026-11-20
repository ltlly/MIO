use crate::{IntruderResourcePool, Uuid};

mod apply;
mod parse;
mod parse_proxy;

#[cfg(test)]
pub(crate) use parse::parse_config_bytes;
pub(crate) use parse::{load_config_file, normalize_project_name};

const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;
const MAX_PROJECT_NAME_BYTES: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupRepeaterPatch {
    pub unpack_compressed_responses: Option<bool>,
    pub update_content_length: Option<bool>,
    pub normalize_http1_line_endings: Option<bool>,
    pub follow_redirects: Option<crate::RepeaterRedirectMode>,
    pub request_timeout_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupResourcePoolsPatch {
    pub pools: Vec<IntruderResourcePool>,
    pub default_pool_id: Option<Uuid>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupProjectConfig {
    pub repeater: Option<StartupRepeaterPatch>,
    pub upstream_proxy: Option<crate::UpstreamProxySettings>,
    pub upstream_tls: Option<crate::UpstreamTlsSettings>,
    pub scope: Option<crate::TargetScope>,
    pub match_and_replace_rules: Option<Vec<crate::MatchAndReplaceRule>>,
    pub proxy_interception: Option<crate::ProxyInterceptionSettings>,
    pub proxy_messages: Option<crate::ProxyMessageSettings>,
    pub proxy_websocket_interception: Option<crate::ProxyWebSocketInterceptionSettings>,
    pub ssl_pass_through_rules: Option<Vec<crate::SslPassThroughRule>>,
    pub ws_match_and_replace_rules: Option<Vec<crate::WsMatchAndReplaceRule>>,
    pub response_modification: Option<crate::ProxyResponseModificationSettings>,
    pub sessions: Option<crate::SessionSettings>,
    pub resource_pools: Option<StartupResourcePoolsPatch>,
}

#[cfg(test)]
mod tests;
