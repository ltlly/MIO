use super::*;

impl IntruderState {
    pub(in crate::service) fn with_resource_pools(resource_pools: IntruderResourcePools) -> Self {
        let pool_timings = resource_pools
            .pools
            .iter()
            .map(|pool| (pool.id, PoolTiming::default()))
            .collect();
        Self {
            resource_pools,
            pool_timings,
            ..Self::default()
        }
    }

    pub(super) fn prepared_pool_timings(
        &self,
        pools: &[IntruderResourcePool],
    ) -> Result<BTreeMap<Uuid, PoolTiming>> {
        ensure!(
            pools.len() <= MAX_INTRUDER_RESOURCE_POOLS,
            "Intruder 资源池数量超过上限"
        );
        let mut next = BTreeMap::new();
        for pool in pools {
            pool.validate()?;
            let previous = self.pool_timings.get(&pool.id).copied().unwrap_or_default();
            let timing = if self
                .resource_pools
                .find(pool.id)
                .is_some_and(|old| old.delay == pool.delay)
            {
                // Name, concurrency and automatic-retry edits retain an already sampled deadline.
                previous.check_policy(pool.delay)?;
                previous
            } else {
                previous.reconfigured(pool.delay)?
            };
            ensure!(
                next.insert(pool.id, timing).is_none(),
                "Intruder 资源池 ID 重复"
            );
        }
        Ok(next)
    }

    pub(super) fn pool_ready_at(&self, run: &ActiveRun, now: PoolInstant) -> PoolInstant {
        run.summary
            .resource_pool
            .and_then(|id| self.pool_timings.get(&id))
            .map_or(now, |timing| timing.ready_at(now))
    }

    pub(super) fn check_pool_send(&self, run: &ActiveRun, now: PoolInstant) -> Result<()> {
        let Some(id) = run.summary.resource_pool else {
            return Ok(());
        };
        let pool = self
            .resource_pools
            .find(id)
            .context("Intruder 资源池不存在；未发送请求")?;
        let timing = self
            .pool_timings
            .get(&id)
            .context("Intruder 资源池时钟不存在；未发送请求")?;
        timing.check_after_send(now, pool.delay)
    }

    pub(super) fn prepared_pool_send(
        &self,
        run: &ActiveRun,
        now: PoolInstant,
    ) -> Result<Option<(Uuid, PoolTiming)>> {
        let Some(id) = run.summary.resource_pool else {
            return Ok(None);
        };
        let pool = self
            .resource_pools
            .find(id)
            .context("Intruder 资源池不存在；未发送请求")?;
        let timing = self
            .pool_timings
            .get(&id)
            .context("Intruder 资源池时钟不存在；未发送请求")?;
        Ok(Some((id, timing.after_send(now, pool.delay)?)))
    }

    pub(super) fn pool_has_capacity(&self, run: &ActiveRun) -> bool {
        match run.summary.resource_pool {
            None => {
                run.active + usize::from(run.preparing) < usize::from(run.max_concurrent_requests)
            }
            Some(id) => self.resource_pools.find(id).is_some_and(|pool| {
                let active_in_pool = self
                    .requests
                    .values()
                    .filter(|request| request.resource_pool == Some(id))
                    .count();
                let prep_in_pool = self
                    .runs
                    .values()
                    .filter(|r| r.preparing && r.summary.resource_pool == Some(id))
                    .count();
                active_in_pool + prep_in_pool
                    < pool
                        .max_concurrent_requests
                        .map_or(MAX_REQUESTS_IN_FLIGHT, usize::from)
            }),
        }
    }

    pub(in crate::service) fn can_accept_commands(&self) -> bool {
        self.notifications.is_empty()
    }

    pub(in crate::service) fn has_resources(&self) -> bool {
        !self.runs.is_empty() || !self.jobs.is_empty() || !self.prep_jobs.is_empty()
    }

    pub(in crate::service) fn cancel_all(&self) {
        for run in self.runs.values() {
            run.cancel.cancel();
        }
    }

    pub(super) fn reclaim_candidate(&self) -> Option<Uuid> {
        self.runs
            .iter()
            .find(|(_, run)| {
                (run.cancel.is_cancelled()
                    && matches!(
                        run.summary.state,
                        IntruderRunState::Running | IntruderRunState::Paused
                    ))
                    || (run.active == 0
                        && !run.preparing
                        && (run.summary.state == IntruderRunState::Stopping
                            || run.summary.dispatched == run.summary.total))
            })
            .map(|(id, _)| *id)
    }

    #[cfg(test)]
    pub(super) fn advance_candidate(&self) -> Option<Uuid> {
        if let Some(id) = self.reclaim_candidate() {
            return Some(id);
        }
        let now = PoolInstant::now();
        self.dispatch_candidate_at(now)
            .filter(|(_, deadline)| *deadline <= now)
            .map(|(id, _)| id)
    }

    pub(super) fn dispatch_candidate_at(&self, now: PoolInstant) -> Option<(Uuid, PoolInstant)> {
        if !self.can_accept_commands()
            || self.requests.len() + self.prep_jobs.len() >= MAX_REQUESTS_IN_FLIGHT
            || !self.recovery.is_empty()
        {
            return None;
        }
        let eligible = |(_, run): &(&Uuid, &ActiveRun)| {
            run.summary.state == IntruderRunState::Running
                && !run.cancel.is_cancelled()
                && !run.preparing
                && !run.waiting_for_capacity
                && self.pool_has_capacity(run)
                && run.summary.dispatched < run.summary.total
        };
        self.runs
            .iter()
            .filter(eligible)
            .map(|(id, run)| (*id, self.pool_ready_at(run, now)))
            .min_by_key(|(id, deadline)| {
                (
                    *deadline,
                    self.last_dispatched.is_some_and(|last| *id <= last),
                    *id,
                )
            })
    }

    pub(in crate::service) async fn next_signal(
        &mut self,
        events: &mpsc::Sender<Event>,
        reclaim_only: bool,
    ) -> Signal {
        let reclaim = self.reclaim_candidate();
        let next = if reclaim_only || reclaim.is_some() {
            None
        } else {
            self.dispatch_candidate_at(PoolInstant::now())
        };
        // A paused run can have neither jobs nor an event to wake this select. Keep each
        // registered wait pinned across Pending so out-of-band Stop still wakes it.
        // Stopping runs already have their cancellation applied and must not busy-spin.
        let mut cancellations: Vec<_> = self
            .runs
            .iter()
            .filter(|(_, run)| {
                matches!(
                    run.summary.state,
                    IntruderRunState::Running | IntruderRunState::Paused
                )
            })
            .map(|(id, run)| (*id, Box::pin(run.cancel.clone().cancelled_owned())))
            .collect();
        let cancelled = std::future::poll_fn(move |cx| {
            for (id, cancelled) in &mut cancellations {
                if cancelled.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(*id);
                }
            }
            Poll::Pending
        });
        tokio::select! {
            biased;
            Some(result) = self.jobs.join_next_with_id(), if !self.jobs.is_empty() => Signal::Completed(Box::new(result)),
            Some(result) = self.prep_jobs.join_next_with_id(), if !self.prep_jobs.is_empty() => Signal::Prepared(Box::new(result)),
            id = async { reclaim.unwrap() }, if reclaim.is_some() => Signal::Advance(id),
            id = cancelled => Signal::Advance(id),
            Ok(permit) = events.clone().reserve_owned(), if !reclaim_only && !self.notifications.is_empty() => Signal::Notification(permit),
            Ok(permit) = events.clone().reserve_owned(), if !reclaim_only && self.dirty && !self.notified => Signal::Notify(permit),
            Ok(permit) = async {
                // New requests yield to ready commands and wait for UI capacity; reclamation does neither.
                let deadline = next.unwrap().1;
                if deadline > PoolInstant::now() {
                    tokio::time::sleep_until(deadline).await;
                }
                tokio::task::yield_now().await;
                events.clone().reserve_owned().await
            }, if next.is_some() => {
                drop(permit);
                Signal::Advance(next.unwrap().0)
            },
            else => std::future::pending().await,
        }
    }

    pub(in crate::service) fn queried(&mut self) {
        // A single-run query cannot acknowledge other runs' undelivered changes.
        self.notified = false;
    }

    pub(super) fn capacity_released(&mut self) {
        for run in self.runs.values_mut() {
            run.waiting_for_capacity = false;
        }
    }
}
