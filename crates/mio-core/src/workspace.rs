use std::collections::HashSet;

use anyhow::{Context, Result, ensure};

use crate::{
    AdvancedScopeRule, Draft, HostnameResolutionRule, HttpRequest, HttpVersion, ScopeProtocol,
    ScopeRule, SocksProxySettings, TabGroup, TargetScope, UpstreamAuthType, UpstreamProxyProtocol,
    UpstreamProxyRule, UpstreamProxySettings, Uuid, WorkTab, Workspace,
    http::MAX_BODY, intruder::MAX_PAYLOADS, request_to_raw,
};

mod scope;
mod validation;
pub use validation::{
    validate_destination_glob_pattern, validate_listener_custom_pem,
    validate_listener_specific_hostname, validate_redirect_host,
};

#[cfg(test)]
mod intruder_tests;
#[cfg(test)]
mod tests;
