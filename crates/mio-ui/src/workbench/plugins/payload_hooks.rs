use mio_core::{
    PluginAction, Uuid,
    protocol::{ChannelIdentity, Permission},
};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn request_plugin_payload_generation(
        &mut self,
        instance_id: Uuid,
        tab_id: TabId,
        config: String,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self
            .plugin_action(
                PluginAction::GeneratePayloads {
                    instance_id,
                    tab_id,
                    config,
                    max_items: 1000,
                },
                Operation::GeneratePayloads { tab_id },
                cx,
            )
            .is_some()
        {
            self.status = "正在调用插件生成 Intruder payload…".into();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn request_plugin_payload_processing(
        &mut self,
        instance_id: Uuid,
        tab_id: TabId,
        position: Option<usize>,
        config: String,
        inputs: Vec<Vec<u8>>,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self
            .plugin_action(
                PluginAction::ProcessPayloads {
                    instance_id,
                    tab_id,
                    position,
                    config,
                    inputs,
                },
                Operation::ProcessPayloads { tab_id, position },
                cx,
            )
            .is_some()
        {
            self.status = "正在调用插件处理 Intruder payload…".into();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn finish_plugin_generated_payloads(
        &mut self,
        tab_id: TabId,
        identity: ChannelIdentity,
        reply_tab: TabId,
        payloads: &[Vec<u8>],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if reply_tab != tab_id
            || !self
                .extensions
                .instance(identity.instance_id)
                .is_some_and(|instance| instance.live && instance.info.identity == identity)
        {
            self.fail("生成 payload 的插件结果已过期；列表未改变。", cx);
            return;
        }
        self.extensions
            .push_log(format!("插件生成 payload：{} 条", payloads.len()));
        if self.append_intruder_payloads_from_plugin(tab_id, payloads, window, cx) {
            self.status = format!("插件已生成并追加 {} 条 payload。", payloads.len());
        }
    }

    pub(in crate::workbench) fn finish_plugin_processed_payloads(
        &mut self,
        expected: (TabId, Option<usize>),
        actual: (ChannelIdentity, TabId, Option<usize>),
        payloads: Vec<Vec<u8>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (tab_id, position) = expected;
        let (identity, reply_tab, reply_position) = actual;
        if (reply_tab, reply_position) != (tab_id, position)
            || !self
                .extensions
                .instance(identity.instance_id)
                .is_some_and(|instance| instance.live && instance.info.identity == identity)
        {
            self.fail("插件处理结果已过期；payload 列表未改变。", cx);
            return;
        }
        self.extensions
            .push_log(format!("插件处理 payload：{} 条", payloads.len()));
        if self.apply_processed_intruder_payloads(tab_id, position, payloads, window, cx) {
            self.status = "插件处理结果已应用到 Intruder payload 列表。".into();
        }
    }

    pub(in crate::workbench) fn live_payload_plugins(
        &self,
        contribution: &str,
    ) -> Vec<(Uuid, String)> {
        self.extensions
            .instances
            .iter()
            .filter(|instance| {
                instance.live
                    && instance.info.tab_id.is_none()
                    && instance.context_owner.is_none()
                    && instance
                        .info
                        .manifest
                        .contributions
                        .iter()
                        .any(|item| item == contribution)
                    && instance.info.grants.contains(&Permission::PayloadProcess)
            })
            .map(|instance| {
                (
                    instance.info.identity.instance_id,
                    instance.info.manifest.id.clone(),
                )
            })
            .collect()
    }
}
