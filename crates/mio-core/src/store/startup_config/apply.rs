use std::collections::BTreeSet;

use anyhow::{Result, bail, ensure};
use tokio_util::sync::CancellationToken;

use super::StartupProjectConfig;
use crate::{
    IntruderAutoThrottle, IntruderResourcePool, MAX_INTRUDER_RESOURCE_POOLS, RepeaterSettings,
    StartupDefaultConfig,
    store::{ProjectStore, intruder, metadata, resource_pools, write_workspace},
};

impl crate::store::MetadataRepo<&mut ProjectStore> {
    pub(crate) fn apply_startup_options(
        &mut self,
        project_name: Option<String>,
        config_mode: &StartupDefaultConfig,
        loaded_config: Option<&StartupProjectConfig>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        ensure!(!cancel.is_cancelled(), "项目启动配置应用已取消");
        let transaction = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            metadata::project_id(&transaction)? == self.0.id,
            "项目 ID 在应用启动配置时已改变"
        );
        let mut workspace = metadata::workspace(&transaction)?;
        let mut workspace_changed = false;
        if let Some(name) = project_name
            && workspace.project_name.as_ref() != Some(&name)
        {
            workspace.project_name = Some(name);
            workspace_changed = true;
        }
        match config_mode {
            StartupDefaultConfig::Project => {}
            StartupDefaultConfig::Defaults => {
                if !workspace.proxy_listeners.is_empty() {
                    workspace.proxy_listeners.clear();
                    workspace_changed = true;
                }
                if !workspace.client_certificates.is_empty() {
                    workspace.client_certificates.clear();
                    workspace_changed = true;
                }
                let defaults = RepeaterSettings::default();
                if workspace.repeater_settings != defaults {
                    workspace.repeater_settings = defaults;
                    workspace_changed = true;
                }
                if !workspace.network_http.is_default() {
                    workspace.network_http = crate::NetworkHttpSettings::default();
                    workspace_changed = true;
                }
                let default_upstream = crate::UpstreamProxySettings::default();
                if workspace.upstream_proxy != default_upstream {
                    workspace.upstream_proxy = default_upstream;
                    workspace_changed = true;
                }
                let default_upstream_tls = crate::UpstreamTlsSettings::default();
                if workspace.upstream_tls != default_upstream_tls {
                    workspace.upstream_tls = default_upstream_tls;
                    workspace_changed = true;
                }
                if !workspace.scope.is_empty() {
                    workspace.scope = crate::TargetScope::default();
                    workspace_changed = true;
                }
                if !workspace.match_and_replace_rules.is_empty() {
                    workspace.match_and_replace_rules.clear();
                    workspace_changed = true;
                }
                if !workspace.ssl_pass_through_rules.is_empty() {
                    workspace.ssl_pass_through_rules.clear();
                    workspace_changed = true;
                }
                if !workspace.ws_match_and_replace_rules.is_empty() {
                    workspace.ws_match_and_replace_rules.clear();
                    workspace_changed = true;
                }
                if !workspace.proxy_interception.is_default() {
                    workspace.proxy_interception = crate::ProxyInterceptionSettings::default();
                    workspace_changed = true;
                }
                if !workspace.proxy_messages.is_default() {
                    workspace.proxy_messages = crate::ProxyMessageSettings::default();
                    workspace_changed = true;
                }
                if !workspace.proxy_websocket_interception.is_default() {
                    workspace.proxy_websocket_interception =
                        crate::ProxyWebSocketInterceptionSettings::default();
                    workspace_changed = true;
                }
                if !workspace.sessions.is_default() {
                    workspace.sessions = crate::SessionSettings::default();
                    workspace_changed = true;
                }
                let current_pools = resource_pools::read(&transaction)?;
                let mut reset_pools = Vec::with_capacity(current_pools.pools.len());
                for pool in &current_pools.pools {
                    let reset = if current_pools.default_pool_id == Some(pool.id) {
                        IntruderResourcePool {
                            name: pool.name.clone(),
                            ..resource_pools::default_pool(pool.id)
                        }
                    } else {
                        IntruderResourcePool {
                            id: pool.id,
                            name: pool.name.clone(),
                            max_concurrent_requests: None,
                            delay: None,
                            automatic_throttling: IntruderAutoThrottle::default(),
                            retained_inputs: None,
                        }
                    };
                    reset_pools.push(reset);
                }
                if reset_pools != current_pools.pools {
                    resource_pools::write(
                        &transaction,
                        &current_pools,
                        reset_pools,
                        current_pools.default_pool_id,
                        cancel,
                    )?;
                }
            }
            StartupDefaultConfig::ConfigFile { .. } => {
                let Some(config) = loaded_config else {
                    bail!("缺少已加载的启动配置文件内容");
                };
                if let Some(repeater) = &config.repeater {
                    if let Some(unpack) = repeater.unpack_compressed_responses
                        && workspace.repeater_settings.unpack_compressed_responses != unpack
                    {
                        workspace.repeater_settings.unpack_compressed_responses = unpack;
                        workspace_changed = true;
                    }
                    if let Some(update_cl) = repeater.update_content_length
                        && workspace.repeater_settings.update_content_length != update_cl
                    {
                        workspace.repeater_settings.update_content_length = update_cl;
                        workspace_changed = true;
                    }
                    if let Some(normalize_line_endings) = repeater.normalize_http1_line_endings
                        && workspace.repeater_settings.normalize_http1_line_endings
                            != normalize_line_endings
                    {
                        workspace.repeater_settings.normalize_http1_line_endings =
                            normalize_line_endings;
                        workspace_changed = true;
                    }
                    if let Some(follow_redirects) = repeater.follow_redirects
                        && workspace.repeater_settings.follow_redirects != follow_redirects
                    {
                        workspace.repeater_settings.follow_redirects = follow_redirects;
                        workspace_changed = true;
                    }
                    if let Some(request_timeout_secs) = repeater.request_timeout_secs
                        && workspace.repeater_settings.request_timeout_secs != request_timeout_secs
                    {
                        workspace.repeater_settings.request_timeout_secs = request_timeout_secs;
                        workspace_changed = true;
                    }
                }
                if let Some(upstream_proxy) = &config.upstream_proxy
                    && &workspace.upstream_proxy != upstream_proxy
                {
                    workspace.upstream_proxy = upstream_proxy.clone();
                    workspace_changed = true;
                }
                if let Some(upstream_tls) = &config.upstream_tls
                    && &workspace.upstream_tls != upstream_tls
                {
                    workspace.upstream_tls = upstream_tls.clone();
                    workspace_changed = true;
                }
                if let Some(scope) = &config.scope
                    && &workspace.scope != scope
                {
                    workspace.scope = scope.clone();
                    workspace_changed = true;
                }
                if let Some(rules) = &config.match_and_replace_rules
                    && &workspace.match_and_replace_rules != rules
                {
                    workspace.match_and_replace_rules = rules.clone();
                    workspace.validate()?;
                    workspace_changed = true;
                }
                if let Some(interception) = &config.proxy_interception
                    && &workspace.proxy_interception != interception
                {
                    workspace.proxy_interception = interception.clone();
                    workspace_changed = true;
                }
                if let Some(messages) = config.proxy_messages
                    && workspace.proxy_messages != messages
                {
                    workspace.proxy_messages = messages;
                    workspace_changed = true;
                }
                if let Some(ssl_rules) = &config.ssl_pass_through_rules
                    && &workspace.ssl_pass_through_rules != ssl_rules
                {
                    workspace.ssl_pass_through_rules = ssl_rules.clone();
                    workspace.validate()?;
                    workspace_changed = true;
                }
                if let Some(ws_rules) = &config.ws_match_and_replace_rules
                    && &workspace.ws_match_and_replace_rules != ws_rules
                {
                    workspace.ws_match_and_replace_rules = ws_rules.clone();
                    workspace.validate()?;
                    workspace_changed = true;
                }
                if let Some(resp_mod) = config.response_modification
                    && workspace.proxy_messages.response_modification != resp_mod
                {
                    workspace.proxy_messages.response_modification = resp_mod;
                    workspace_changed = true;
                }
                if let Some(websocket_interception) = config.proxy_websocket_interception
                    && workspace.proxy_websocket_interception != websocket_interception
                {
                    workspace.proxy_websocket_interception = websocket_interception;
                    workspace_changed = true;
                }
                if let Some(sessions) = &config.sessions
                    && &workspace.sessions != sessions
                {
                    workspace.sessions = sessions.clone();
                    workspace_changed = true;
                }
                if let Some(pools_patch) = &config.resource_pools {
                    let current = resource_pools::read(&transaction)?;
                    let removed: BTreeSet<_> = current
                        .pools
                        .iter()
                        .map(|pool| pool.id)
                        .filter(|id| !pools_patch.pools.iter().any(|pool| pool.id == *id))
                        .collect();
                    if !removed.is_empty() {
                        intruder::ensure_resource_pools_unreferenced(
                            &transaction,
                            &self.0.intruder_sessions,
                            &removed,
                            cancel,
                        )?;
                    }
                    let mut pools = pools_patch.pools.clone();
                    let default_pool_id = match (
                        current.default_pool_id,
                        pools_patch.default_pool_id,
                    ) {
                        (Some(existing), Some(configured)) if existing != configured => {
                            bail!(
                                "启动配置文件的 default_pool_id 与项目既有默认资源池冲突；未覆盖"
                            );
                        }
                        (Some(existing), None) => {
                            if !pools.iter().any(|pool| pool.id == existing) {
                                ensure!(
                                    pools.len() < MAX_INTRUDER_RESOURCE_POOLS,
                                    "Intruder 资源池超过 64 个限额"
                                );
                                if let Some(default_pool) = current.find(existing) {
                                    pools.insert(0, default_pool.clone());
                                }
                            }
                            Some(existing)
                        }
                        (Some(existing), Some(_)) => Some(existing),
                        (None, configured) => configured,
                    };
                    if pools != current.pools || default_pool_id != current.default_pool_id {
                        resource_pools::write(
                            &transaction,
                            &current,
                            pools,
                            default_pool_id,
                            cancel,
                        )?;
                    }
                }
            }
        }
        if workspace_changed {
            workspace.revision += 1;
            write_workspace(&transaction, &workspace)?;
        }
        ensure!(!cancel.is_cancelled(), "项目启动配置应用已取消");
        transaction.commit()?;
        Ok(())
    }
}
