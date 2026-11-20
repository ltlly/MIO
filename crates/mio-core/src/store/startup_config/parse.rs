use std::{fs::OpenOptions, io::Read, path::Path};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use super::{
    MAX_CONFIG_FILE_BYTES, MAX_PROJECT_NAME_BYTES, StartupProjectConfig, StartupRepeaterPatch,
    StartupResourcePoolsPatch,
    parse_proxy::{RawProxyConfig, parse_proxy_section},
};
use crate::{IntruderResourcePool, IntruderResourcePools, Uuid};

pub(super) fn deserialize_present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRepeaterPatch {
    #[serde(default, deserialize_with = "deserialize_present")]
    unpack_compressed_responses: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_present")]
    update_content_length: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_present")]
    normalize_http1_line_endings: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_present")]
    follow_redirects: Option<crate::RepeaterRedirectMode>,
    #[serde(default, deserialize_with = "deserialize_present")]
    request_timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResourcePoolsCatalog {
    #[serde(default)]
    #[allow(dead_code)]
    revision: u64,
    pools: Vec<IntruderResourcePool>,
    #[serde(default, deserialize_with = "deserialize_present")]
    default_pool_id: Option<Uuid>,
}

enum RawResourcePools {
    Catalog(RawResourcePoolsCatalog),
    List(Vec<IntruderResourcePool>),
}

impl<'de> Deserialize<'de> for RawResourcePools {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = RawResourcePools;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("Intruder 资源池目录对象或列表")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                RawResourcePoolsCatalog::deserialize(serde::de::value::MapAccessDeserializer::new(
                    map,
                ))
                .map(RawResourcePools::Catalog)
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                Vec::<IntruderResourcePool>::deserialize(
                    serde::de::value::SeqAccessDeserializer::new(seq),
                )
                .map(RawResourcePools::List)
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfigFile {
    #[serde(default, deserialize_with = "deserialize_present")]
    repeater: Option<RawRepeaterPatch>,
    #[serde(default, deserialize_with = "deserialize_present")]
    repeater_settings: Option<RawRepeaterPatch>,
    #[serde(default, deserialize_with = "deserialize_present")]
    upstream_proxy: Option<crate::UpstreamProxySettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    upstream_tls: Option<crate::UpstreamTlsSettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    scope: Option<crate::TargetScope>,
    #[serde(default, deserialize_with = "deserialize_present")]
    match_and_replace_rules: Option<Vec<crate::MatchAndReplaceRule>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    proxy_interception: Option<crate::ProxyInterceptionSettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    proxy_messages: Option<crate::ProxyMessageSettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    proxy_websocket_interception: Option<crate::ProxyWebSocketInterceptionSettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    sessions: Option<crate::SessionSettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    intruder_resource_pools: Option<RawResourcePools>,
    #[serde(default, deserialize_with = "deserialize_present")]
    resource_pools: Option<RawResourcePools>,
    #[serde(default, deserialize_with = "deserialize_present")]
    default_pool_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_present")]
    proxy: Option<RawProxyConfig>,
}

pub(crate) fn normalize_project_name(name: Option<&str>) -> Result<Option<String>> {
    let Some(name) = name else {
        return Ok(None);
    };
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    ensure!(
        trimmed.len() <= MAX_PROJECT_NAME_BYTES && !trimmed.chars().any(char::is_control),
        "项目名称不能包含控制字符或超过 256 字节"
    );
    Ok(Some(trimmed.to_owned()))
}

pub(crate) fn load_config_file(path: &Path) -> Result<StartupProjectConfig> {
    let raw_path = path.as_os_str().as_encoded_bytes();
    ensure!(
        !raw_path.is_empty() && raw_path.len() <= 4096 && !raw_path.contains(&0),
        "启动配置文件路径不能为空、超过上限或包含空字节"
    );
    ensure!(
        path.is_absolute() || raw_path.starts_with(b"/"),
        "启动配置文件路径必须是绝对路径"
    );
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir)),
        "启动配置文件路径不能包含父目录跳转"
    );
    #[cfg(not(unix))]
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "启动配置文件必须是普通文件：{}",
            path.display()
        );
    }
    let mut open_opts = OpenOptions::new();
    open_opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = open_opts
        .open(path)
        .with_context(|| format!("不能安全打开启动配置文件 {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("不能读取启动配置文件元数据 {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "启动配置文件必须是普通文件：{}",
        path.display()
    );
    ensure!(
        metadata.len() <= MAX_CONFIG_FILE_BYTES,
        "启动配置文件超过 1 MiB 限额"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_CONFIG_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("读取启动配置文件失败 {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= MAX_CONFIG_FILE_BYTES,
        "启动配置文件在读取时超过 1 MiB 限额"
    );
    parse_config_bytes(&bytes)
}

pub(crate) fn parse_config_bytes(bytes: &[u8]) -> Result<StartupProjectConfig> {
    let mut unknown = false;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let raw: RawConfigFile = serde_ignored::deserialize(&mut decoder, |_| unknown = true).map_err(|error| {
        anyhow::anyhow!(
            "启动配置文件不是合法的 JSON，或包含未映射、重复、null、非空 proxy.request_listeners 等无效设置项：{error}"
        )
    })?;
    decoder.end().context("启动配置文件 JSON 存在尾随内容")?;
    ensure!(!unknown, "启动配置文件包含未映射的设置项；未应用配置");
    let _ = raw.proxy.as_ref().map(|proxy| proxy.request_listeners);
    ensure!(
        !(raw.proxy_websocket_interception.is_some()
            && raw
                .proxy
                .as_ref()
                .is_some_and(|proxy| proxy.intercept_web_sockets_messages.is_some())),
        "启动配置文件不能同时指定 `proxy_websocket_interception` 与 `proxy.intercept_web_sockets_messages`"
    );
    ensure!(
        !(raw.repeater.is_some() && raw.repeater_settings.is_some()),
        "启动配置文件不能同时指定 `repeater` 与 `repeater_settings`"
    );
    ensure!(
        !(raw.intruder_resource_pools.is_some() && raw.resource_pools.is_some()),
        "启动配置文件不能同时指定 `intruder_resource_pools` 与 `resource_pools`"
    );
    let repeater = raw
        .repeater
        .or(raw.repeater_settings)
        .map(|patch| -> Result<StartupRepeaterPatch> {
            if let Some(timeout) = patch.request_timeout_secs {
                ensure!(
                    (1..=3600).contains(&timeout),
                    "Repeater 请求超时必须在 1 到 3600 秒之间"
                );
            }
            Ok(StartupRepeaterPatch {
                unpack_compressed_responses: patch.unpack_compressed_responses,
                update_content_length: patch.update_content_length,
                normalize_http1_line_endings: patch.normalize_http1_line_endings,
                follow_redirects: patch.follow_redirects,
                request_timeout_secs: patch.request_timeout_secs,
            })
        })
        .transpose()?;
    if let Some(upstream) = &raw.upstream_proxy {
        upstream.validate()?;
    }
    if let Some(tls) = &raw.upstream_tls {
        tls.validate()?;
    }
    if let Some(interception) = &raw.proxy_interception {
        interception.validate()?;
    }
    if let Some(scope) = &raw.scope {
        scope.validate()?;
    }
    if let Some(sessions) = &raw.sessions {
        sessions.validate()?;
    }
    if let Some(rules) = &raw.match_and_replace_rules {
        let workspace = crate::Workspace {
            match_and_replace_rules: rules.clone(),
            ..crate::Workspace::default()
        };
        workspace.validate()?;
    }
    let resource_pools = match raw.intruder_resource_pools.or(raw.resource_pools) {
        Some(RawResourcePools::Catalog(catalog)) => {
            ensure!(
                raw.default_pool_id.is_none() || raw.default_pool_id == catalog.default_pool_id,
                "启动配置文件的 default_pool_id 与资源池目录不一致"
            );
            let default_pool_id = catalog.default_pool_id.or(raw.default_pool_id);
            let validated = IntruderResourcePools {
                revision: 1,
                pools: catalog.pools,
                default_pool_id,
            };
            validated.validate()?;
            Some(StartupResourcePoolsPatch {
                pools: validated.pools,
                default_pool_id: validated.default_pool_id,
            })
        }
        Some(RawResourcePools::List(pools)) => {
            let validated = IntruderResourcePools {
                revision: 1,
                pools,
                default_pool_id: raw.default_pool_id,
            };
            validated.validate()?;
            Some(StartupResourcePoolsPatch {
                pools: validated.pools,
                default_pool_id: validated.default_pool_id,
            })
        }
        None => {
            ensure!(
                raw.default_pool_id.is_none(),
                "启动配置文件指定 `default_pool_id` 时必须同时提供资源池列表"
            );
            None
        }
    };
    let (proxy_ws_interception, ssl_pass_through_rules, ws_match_and_replace_rules, response_mod) =
        parse_proxy_section(raw.proxy)?;
    ensure!(
        !(raw
            .proxy_messages
            .as_ref()
            .is_some_and(|m| !m.response_modification.is_default())
            && response_mod.is_some()),
        "启动配置文件不能同时在 proxy_messages 与 proxy.response_modification 中指定开关"
    );
    Ok(StartupProjectConfig {
        repeater,
        upstream_proxy: raw.upstream_proxy,
        upstream_tls: raw.upstream_tls,
        scope: raw.scope,
        match_and_replace_rules: raw.match_and_replace_rules,
        proxy_interception: raw.proxy_interception,
        proxy_messages: raw.proxy_messages,
        proxy_websocket_interception: raw.proxy_websocket_interception.or(proxy_ws_interception),
        ssl_pass_through_rules,
        ws_match_and_replace_rules,
        response_modification: response_mod,
        sessions: raw.sessions,
        resource_pools,
    })
}
