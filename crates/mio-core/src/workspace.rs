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

#[cfg(test)]
mod intruder_tests;
#[cfg(test)]
mod tests;
