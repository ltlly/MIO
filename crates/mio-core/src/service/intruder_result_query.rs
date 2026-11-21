use std::collections::BTreeMap;

use super::*;
use crate::store::IntruderMutation;

impl State {
    pub(super) async fn query_intruder_result_view(
        &mut self,
        query: IntruderResultViewQuery,
        cancel: CancellationToken,
    ) {
        let mut result = self.intruder_result_view(&query, &cancel);
        if cancel.is_cancelled() || self.cancel.is_cancelled() {
            result = Err(anyhow::anyhow!("Intruder 结果查询已取消"));
        }
        if result.is_ok() {
            // Later completions observed while awaiting UI capacity must remain dirty.
            self.intruder.queried();
        }
        self.flush_proxy_notifications().await;
        self.flush_repeater_notifications().await;
        if let Some(permit) = self.event_permit().await {
            if cancel.is_cancelled() || self.cancel.is_cancelled() {
                result = Err(anyhow::anyhow!("Intruder 结果查询已取消"));
            }
            permit.send(Event::IntruderResultView {
                query,
                result: result.map_err(|error| error.to_string()),
            });
        }
    }

    fn intruder_result_view(
        &self,
        query: &IntruderResultViewQuery,
        cancel: &CancellationToken,
    ) -> Result<IntruderResultViewPage> {
        self.check_project(query.project)?;
        ensure!(
            !cancel.is_cancelled() && !self.cancel.is_cancelled(),
            "Intruder 结果查询已取消"
        );
        ensure!(
            !query.run_id.is_nil() && !query.view_id.is_nil() && query.query_revision != 0,
            "Intruder 结果查询身份无效"
        );
        ensure!(
            (1..=500).contains(&query.limit)
                && i64::try_from(query.offset).is_ok()
                && query
                    .selected_ordinal
                    .is_none_or(|ordinal| i64::try_from(ordinal).is_ok()),
            "Intruder 结果查询范围无效"
        );
        let run = self.pending_intruder_run(query.run_id);
        let mut recovery = BTreeMap::new();
        let mut recovery_responses = BTreeMap::new();
        let mut has_recovery = false;
        for mutation in &self.intruder.recovery {
            ensure!(
                !cancel.is_cancelled() && !self.cancel.is_cancelled(),
                "Intruder 结果查询已取消"
            );
            let (IntruderMutation::Result { run, .. } | IntruderMutation::State { run }) = mutation;
            if run.id != query.run_id {
                continue;
            }
            has_recovery = true;
            if let IntruderMutation::Result {
                ordinal, result, ..
            } = mutation
            {
                if let Ok(record) = result
                    && let Some(response) = &record.response
                {
                    recovery_responses.insert(*ordinal, response);
                } else {
                    recovery_responses.remove(ordinal);
                }
                recovery.insert(
                    *ordinal,
                    IntruderResultSummary {
                        ordinal: *ordinal,
                        outcome: match result {
                            Ok(record) => IntruderResultOutcome::Flow(record.summary.clone()),
                            Err(error) => IntruderResultOutcome::Rejected(error.clone()),
                        },
                    },
                );
            }
        }
        let recovery: Vec<_> = recovery.into_values().collect();
        let mut page = self.store.intruder().query_result_view_with_responses(
            query,
            run.as_ref(),
            &recovery,
            &recovery_responses,
            cancel,
            &self.cancel,
        )?;
        page.persisted &= !has_recovery;
        Ok(page)
    }
}

#[cfg(test)]
mod tests;
