use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SslPassThroughRule {
    pub id: Uuid,
    pub enabled: bool,
    pub host_pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub is_regex: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsReplaceDirection {
    #[default]
    Both,
    ClientToServer,
    ServerToClient,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WsMatchAndReplaceRule {
    pub id: Uuid,
    pub enabled: bool,
    #[serde(default)]
    pub direction: WsReplaceDirection,
    pub match_pattern: String,
    pub replace_with: String,
    #[serde(default)]
    pub is_regex: bool,
    #[serde(default)]
    pub comment: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyResponseModificationSettings {
    #[serde(default)]
    pub unhide_hidden_form_fields: bool,
    #[serde(default)]
    pub enable_disabled_form_fields: bool,
    #[serde(default)]
    pub highlight_unhidden_fields: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_input_field_length_limits: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_javascript_form_validation: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_all_javascript: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_object_tags: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub convert_https_links_to_http: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_secure_flag_from_cookies: bool,
}

impl ProxyResponseModificationSettings {
    pub fn is_default(&self) -> bool {
        !self.unhide_hidden_form_fields
            && !self.enable_disabled_form_fields
            && !self.highlight_unhidden_fields
            && !self.remove_input_field_length_limits
            && !self.remove_javascript_form_validation
            && !self.remove_all_javascript
            && !self.remove_object_tags
            && !self.convert_https_links_to_http
            && !self.remove_secure_flag_from_cookies
    }
}
