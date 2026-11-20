use std::{future::pending, path::PathBuf};

use tokio::task::{JoinError, JoinHandle};

use super::*;
use crate::ext_archive::import::{self, CleanupFailure, Inspection};

const STOP_TIMEOUT: &str = "External 任务仍在回收；保留任务句柄，未切换项目，请稍后重试";

pub(super) enum Output {
    Inspected(Box<Inspection>),
    Finished(ExtJobOutcome, Option<CleanupFailure>),
    Retried {
        cleanup: CleanupFailure,
        error: Option<String>,
    },
}

impl Output {
    fn failed(error: anyhow::Error) -> Self {
        let cleanup = error.downcast_ref::<CleanupFailure>().cloned();
        Self::Finished(import::failed(error, None), cleanup)
    }
}

struct Active {
    project: ProjectKey,
    job_id: Uuid,
    cancel: CancellationToken,
    acknowledge: bool,
    worker: JoinHandle<Output>,
}

pub(super) struct Progress {
    project: ProjectKey,
    job_id: Uuid,
    phase: ExtImportPhase,
    completed: u64,
    total: Option<u64>,
}

pub(super) enum Signal {
    Progress(Progress),
    Finished(Box<Result<Output, JoinError>>),
    AckReady(mpsc::OwnedPermit<Event>),
}

pub(super) struct ExtState {
    active: Option<Active>,
    inspection: Option<(ProjectKey, Inspection)>,
    cleanup: Option<CleanupFailure>,
    worker_error: Option<String>,
    stop_timed_out: bool,
    // Commands pause while an ACK is pending: one rejected command plus one active completion.
    acknowledgements: [Option<Event>; 2],
    progress: mpsc::Sender<Progress>,
    receiver: mpsc::Receiver<Progress>,
}

impl Default for ExtState {
    fn default() -> Self {
        let (progress, receiver) = mpsc::channel(1);
        Self {
            active: None,
            inspection: None,
            cleanup: None,
            worker_error: None,
            stop_timed_out: false,
            acknowledgements: [None, None],
            progress,
            receiver,
        }
    }
}

impl ExtState {
    pub(super) fn can_accept_commands(&self) -> bool {
        self.acknowledgements[0].is_none()
    }

    pub(super) fn can_quiesce(&self) -> bool {
        self.active.is_none()
            && self.inspection.is_none()
            && self.cleanup.is_none()
            && self.worker_error.is_none()
            && !self.stop_timed_out
    }

    pub(super) async fn next_signal(
        &mut self,
        events: &mpsc::Sender<Event>,
        reclaim_only: bool,
    ) -> Signal {
        let has_active = self.active.is_some();
        let has_ack = !reclaim_only && !self.can_accept_commands() && !events.is_closed();
        tokio::select! {
            biased;
            result = async {
                match &mut self.active {
                    Some(active) => (&mut active.worker).await,
                    None => pending().await,
                }
            } => Signal::Finished(Box::new(result)),
            Ok(permit) = events.clone().reserve_owned(), if has_ack => Signal::AckReady(permit),
            Some(progress) = self.receiver.recv(), if !reclaim_only && has_active => Signal::Progress(progress),
        }
    }

    fn take_ack(&mut self) -> Event {
        let event = self.acknowledgements[0]
            .take()
            .expect("reserved capacity belongs to retained External ACK");
        self.acknowledgements[0] = self.acknowledgements[1].take();
        event
    }

    fn require_clean(&self) -> Result<()> {
        if let Some(cleanup) = &self.cleanup {
            return Err(cleanup.clone().into());
        }
        if let Some(error) = &self.worker_error {
            anyhow::bail!("外部导入后台任务的清理状态未确认：{error}");
        }
        Ok(())
    }

    fn start(
        &mut self,
        project: ProjectKey,
        job_id: Uuid,
        cancel: CancellationToken,
        acknowledge: bool,
        work: impl FnOnce(
            &CancellationToken,
            &mut dyn FnMut(ExtImportPhase, u64, Option<u64>),
        ) -> Output
        + Send
        + 'static,
    ) {
        assert!(
            self.active.is_none(),
            "never replace an unjoined External worker"
        );
        let progress = self.progress.clone();
        let worker_cancel = cancel.clone();
        let worker = tokio::task::spawn_blocking(move || {
            work(&worker_cancel, &mut |phase, completed, total| {
                // Progress is replaceable; the separately joined completion is never dropped.
                let _ = progress.try_send(Progress {
                    project,
                    job_id,
                    phase,
                    completed,
                    total,
                });
            })
        });
        self.active = Some(Active {
            project,
            job_id,
            cancel,
            acknowledge,
            worker,
        });
    }

    fn release(
        &mut self,
        project: ProjectKey,
        job_id: Uuid,
        inspection: Inspection,
        outcome: ExtJobOutcome,
        acknowledge: bool,
    ) {
        self.start(
            project,
            job_id,
            CancellationToken::new(),
            acknowledge,
            move |_, _| match inspection.release() {
                Ok(()) => Output::Finished(outcome, None),
                Err(error) => Output::failed(error),
            },
        );
    }

    fn retry_cleanup(&mut self, project: ProjectKey) {
        let Some(mut cleanup) = self.cleanup.clone() else {
            return;
        };
        // Keep the original paths owned here until the retry worker has actually joined.
        self.start(
            project,
            Uuid::new_v4(),
            CancellationToken::new(),
            false,
            move |_, _| {
                let error = cleanup.retry().err().map(|error| format!("{error:#}"));
                Output::Retried { cleanup, error }
            },
        );
    }
}

impl State {
    pub(super) async fn inspect_ext(
        &mut self,
        project: ProjectKey,
        job_id: Uuid,
        path: PathBuf,
        cancel: CancellationToken,
    ) {
        let result =
            (|| -> Result<()> {
                self.check_project(project)?;
                self.ext_archive.require_clean()?;
                ensure!(!self.quiesced, "正在切换项目，不能开始 外部归档预检");
                ensure!(
                    self.ext_archive.active.is_none() && self.ext_archive.inspection.is_none(),
                    "已有 外部导入或待释放的预检"
                );
                self.ext_archive
                    .start(project, job_id, cancel, true, move |cancel, progress| {
                        match import::inspect(path, cancel, progress) {
                            Ok(inspection) => Output::Inspected(Box::new(inspection)),
                            Err(error) => Output::failed(error),
                        }
                    });
                Ok(())
            })();
        if let Err(error) = result {
            self.ext_result(project, job_id, import::failed(error, None));
        }
    }

    pub(super) async fn convert_ext(
        &mut self,
        project: ProjectKey,
        job_id: Uuid,
        inspection_id: Uuid,
        destination: PathBuf,
        cancel: CancellationToken,
    ) {
        let result = (|| -> Result<()> {
            self.check_project(project)?;
            self.ext_archive.require_clean()?;
            ensure!(self.ext_archive.active.is_none(), "已有 外部导入后台任务");
            let (owner, inspection) = self
                .ext_archive
                .inspection
                .as_ref()
                .context("外部归档预检已失效，请重新预检")?;
            ensure!(
                *owner == project && inspection.info.id == inspection_id,
                "外部归档预检归属不匹配"
            );
            let (_, inspection) = self.ext_archive.inspection.take().unwrap();
            let allowed = if self.quiesced {
                Err(anyhow::anyhow!(
                    "正在切换项目，未开始 外部归档转换；预检已释放"
                ))
            } else {
                self.store.check_path_outside_temporary(&destination)
            };
            if let Err(error) = allowed {
                self.ext_archive.release(
                    project,
                    job_id,
                    inspection,
                    import::failed(error, None),
                    true,
                );
                return Ok(());
            }
            self.ext_archive
                .start(project, job_id, cancel, true, move |cancel, progress| {
                    let (outcome, cleanup) =
                        import::convert(inspection, destination, cancel, progress);
                    Output::Finished(outcome, cleanup)
                });
            Ok(())
        })();
        if let Err(error) = result {
            self.ext_result(project, job_id, import::failed(error, None));
        }
    }

    pub(super) async fn release_ext(
        &mut self,
        project: ProjectKey,
        job_id: Uuid,
        inspection_id: Uuid,
    ) {
        let result = (|| -> Result<()> {
            self.check_project(project)?;
            self.ext_archive.require_clean()?;
            ensure!(
                self.ext_archive.active.is_none(),
                "External 任务尚未结束，暂不能释放预检"
            );
            let (owner, inspection) = self
                .ext_archive
                .inspection
                .as_ref()
                .context("外部归档预检已释放或过期")?;
            ensure!(
                *owner == project && inspection.info.id == inspection_id,
                "外部归档预检归属不匹配"
            );
            let (_, inspection) = self.ext_archive.inspection.take().unwrap();
            self.ext_archive.release(
                project,
                job_id,
                inspection,
                ExtJobOutcome::Released { inspection_id },
                true,
            );
            Ok(())
        })();
        if let Err(error) = result {
            self.ext_result(project, job_id, import::failed(error, None));
        }
    }

    fn ext_result(&mut self, project: ProjectKey, job_id: Uuid, outcome: ExtJobOutcome) {
        let slot = self
            .ext_archive
            .acknowledgements
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("External command gating reserves one rejection and one active completion");
        *slot = Some(Event::ExtJob {
            project,
            job_id,
            outcome,
        });
        while !self.awaiting_event_slot && !self.ext_archive.can_accept_commands() {
            let Ok(permit) = self.events.try_reserve() else {
                break;
            };
            permit.send(self.ext_archive.take_ack());
        }
    }

    pub(super) async fn ext_signal(&mut self, signal: Signal) {
        match signal {
            Signal::AckReady(permit) => {
                permit.send(self.ext_archive.take_ack());
            }
            Signal::Progress(progress) => {
                if self.ext_archive.active.as_ref().is_some_and(|active| {
                    active.project == progress.project && active.job_id == progress.job_id
                }) {
                    let _ = self.events.try_send(Event::ExtProgress {
                        project: progress.project,
                        job_id: progress.job_id,
                        phase: progress.phase,
                        completed: progress.completed,
                        total: progress.total,
                    });
                }
            }
            Signal::Finished(result) => {
                let active = self
                    .ext_archive
                    .active
                    .take()
                    .expect("completion belongs to active import");
                while self.ext_archive.receiver.try_recv().is_ok() {}
                let outcome = match *result {
                    Ok(Output::Inspected(inspection)) => {
                        let inspection = *inspection;
                        if active.cancel.is_cancelled()
                            || self.cancel.is_cancelled()
                            || active.project != self.project
                        {
                            self.ext_archive.release(
                                active.project,
                                active.job_id,
                                inspection,
                                ExtJobOutcome::Cancelled,
                                active.acknowledge,
                            );
                            return;
                        }
                        let info = inspection.info.clone();
                        self.ext_archive.inspection = Some((active.project, inspection));
                        ExtJobOutcome::Inspected(info)
                    }
                    Ok(Output::Finished(outcome, cleanup)) => {
                        self.ext_archive.cleanup = cleanup;
                        outcome
                    }
                    Ok(Output::Retried { cleanup, error }) => {
                        self.ext_archive.cleanup = (!cleanup.is_empty()).then_some(cleanup);
                        self.ext_archive.worker_error = error;
                        return;
                    }
                    Err(error) => {
                        let message = format!("外部导入后台任务异常退出：{error}");
                        self.ext_archive.worker_error = Some(message.clone());
                        ExtJobOutcome::Failed {
                            message,
                            created_path: None,
                        }
                    }
                };
                if active.acknowledge {
                    self.ext_result(active.project, active.job_id, outcome);
                }
            }
        }
    }

    pub(super) async fn stop_ext(&mut self) -> Result<()> {
        self.stop_ext_until(tokio::time::Instant::now() + Duration::from_secs(5))
            .await
    }

    async fn stop_ext_until(&mut self, deadline: tokio::time::Instant) -> Result<()> {
        self.ext_archive.stop_timed_out = false;
        if let Some(active) = &self.ext_archive.active {
            active.cancel.cancel();
        } else {
            self.ext_archive.retry_cleanup(self.project);
        }
        self.drain_ext(Some(deadline)).await
    }

    pub(super) async fn finish_ext_shutdown(&mut self) -> Result<()> {
        if let Some(active) = &self.ext_archive.active {
            active.cancel.cancel();
        }
        // spawn_blocking cannot be safely aborted; the runtime must retain ownership until joined.
        self.drain_ext(None).await?;
        ensure!(
            !self.ext_archive.stop_timed_out,
            "External 任务超过关闭期限后才完成回收；后台任务现已全部退出"
        );
        Ok(())
    }

    async fn drain_ext(&mut self, deadline: Option<tokio::time::Instant>) -> Result<()> {
        loop {
            if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                self.ext_archive.stop_timed_out = true;
                anyhow::bail!(STOP_TIMEOUT);
            }
            if self.ext_archive.active.is_none() {
                self.ext_archive.require_clean()?;
                let Some((project, inspection)) = self.ext_archive.inspection.take() else {
                    return Ok(());
                };
                let inspection_id = inspection.info.id;
                self.ext_archive.release(
                    project,
                    Uuid::new_v4(),
                    inspection,
                    ExtJobOutcome::Released { inspection_id },
                    false,
                );
            }
            let signal = if let Some(deadline) = deadline {
                match tokio::time::timeout_at(deadline, self.ext_archive.next_signal(&self.events, false))
                    .await
                {
                    Ok(signal) => signal,
                    Err(_) => {
                        self.ext_archive.stop_timed_out = true;
                        anyhow::bail!(STOP_TIMEOUT);
                    }
                }
            } else {
                self.ext_archive.next_signal(&self.events, false).await
            };
            self.ext_signal(signal).await;
        }
    }
}

#[cfg(test)]
mod tests;
