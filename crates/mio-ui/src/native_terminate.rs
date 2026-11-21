#[cfg(target_os = "macos")]
use std::rc::Weak;
#[cfg(target_os = "macos")]
use std::sync::Mutex;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use anyhow::{Context as _, ensure};
#[cfg(target_os = "macos")]
use block2::RcBlock;
use gpui_kit::{App, Global, PathPromptOptions, Window};
#[cfg(target_os = "macos")]
use objc2::{
    MainThreadMarker, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject},
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplicationTerminateReply as Reply, NSModalPanelRunLoopMode};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSTimer};
use tokio::sync::{oneshot, watch};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(target_os = "macos", test))]
pub(crate) mod tests;

#[cfg(all(target_os = "macos", test))]
use macos::{current, install_hook};

pub(crate) fn prompt_for_paths(
    _window: &Window,
    cx: &App,
    options: PathPromptOptions,
) -> oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>> {
    let (done_tx, done_rx) = oneshot::channel();
    let prompt = cx.prompt_for_paths(options);
    cx.foreground_executor()
        .spawn(async move {
            if let Ok(result) = prompt.await {
                let _ = done_tx.send(result);
            }
        })
        .detach();
    done_rx
}

pub(crate) fn prompt_for_new_path(
    _window: &Window,
    cx: &App,
    directory: &Path,
    suggested_name: Option<&str>,
) -> oneshot::Receiver<anyhow::Result<Option<PathBuf>>> {
    let (done_tx, done_rx) = oneshot::channel();
    let prompt = cx.prompt_for_new_path(directory, suggested_name);
    cx.foreground_executor()
        .spawn(async move {
            if let Ok(result) = prompt.await {
                let _ = done_tx.send(result);
            }
        })
        .detach();
    done_rx
}

const TERMINATE_TIMEOUT: f64 = 30.0;
#[cfg(target_os = "macos")]
static HOOK_LOCK: Mutex<()> = Mutex::new(());
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
#[cfg(target_os = "macos")]
thread_local! {
    static CURRENT: RefCell<Weak<RefCell<State>>> = const { RefCell::new(Weak::new()) };
}

#[derive(Clone, Copy)]
pub(crate) enum NativeRequest {
    Requested(u64),
    TimedOut(u64),
}

#[cfg(target_os = "macos")]
struct Pending {
    id: u64,
    application: Retained<AnyObject>,
    timer: Retained<NSTimer>,
    deadline: Instant,
    decision: Option<bool>,
    timed_out: bool,
    delivered: bool,
}

#[cfg(target_os = "macos")]
struct State {
    pending: Option<Pending>,
    queued: bool,
    permitted: bool,
    terminate_target: Option<Retained<AnyObject>>,
    run_loop: Retained<NSRunLoop>,
    changes: watch::Sender<Option<NativeRequest>>,
}

#[cfg(not(target_os = "macos"))]
struct Pending {
    id: u64,
    deadline: Instant,
    decision: Option<bool>,
    timed_out: bool,
}

#[cfg(not(target_os = "macos"))]
struct State {
    pending: Option<Pending>,
    changes: watch::Sender<Option<NativeRequest>>,
}

#[derive(Clone)]
pub(crate) struct NativeTerminate(Rc<RefCell<State>>);
impl Global for NativeTerminate {}

#[cfg(not(target_os = "macos"))]
impl NativeTerminate {
    pub(crate) fn install() -> anyhow::Result<Self> {
        let (changes, _) = watch::channel(None);
        Ok(Self(Rc::new(RefCell::new(State {
            pending: None,
            changes,
        }))))
    }

    pub(crate) fn request(&self) {
        let mut state = self.0.borrow_mut();
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.decision.is_none())
        {
            return;
        }
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs_f64(TERMINATE_TIMEOUT);
        state.pending = Some(Pending {
            id,
            deadline,
            decision: None,
            timed_out: false,
        });
        state
            .changes
            .send_replace(Some(NativeRequest::Requested(id)));
    }

    pub(crate) fn verify_delegate(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub(crate) fn stop_failed_launch(&self) {
        self.reject();
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Option<NativeRequest>> {
        let mut receiver = self.0.borrow().changes.subscribe();
        receiver.mark_changed();
        receiver
    }

    pub(crate) fn is_pending(&self, id: u64) -> bool {
        !self.rejected()
            && self
                .0
                .borrow()
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == id && pending.decision.is_none())
    }

    pub(crate) fn rejected(&self) -> bool {
        let overdue = self
            .0
            .borrow()
            .pending
            .as_ref()
            .filter(|pending| pending.decision.is_none() && Instant::now() >= pending.deadline)
            .map(|pending| pending.id);
        if let Some(id) = overdue {
            self.timeout(id);
        }
        self.0
            .borrow()
            .pending
            .as_ref()
            .is_some_and(|pending| pending.decision == Some(false))
    }

    pub(crate) fn permit_exit(&self) -> Result<bool, ()> {
        let mut state = self.0.borrow_mut();
        let Some(pending) = &mut state.pending else {
            return Err(());
        };
        if pending.decision == Some(false) {
            return Err(());
        }
        if Instant::now() >= pending.deadline {
            let id = pending.id;
            drop(state);
            self.timeout(id);
            return Err(());
        }
        pending.decision = Some(true);
        state.pending = None;
        Ok(true)
    }

    pub(crate) fn reject(&self) {
        let mut state = self.0.borrow_mut();
        if let Some(pending) = &mut state.pending {
            pending.decision = Some(false);
            if !pending.timed_out {
                state.pending = None;
            }
        }
    }

    pub(crate) fn acknowledge_timeout(&self, id: u64) {
        let mut state = self.0.borrow_mut();
        if let Some(pending) = &mut state.pending
            && pending.id == id
        {
            pending.timed_out = false;
            state.pending = None;
        }
    }

    fn timeout(&self, id: u64) {
        let mut state = self.0.borrow_mut();
        if let Some(pending) = &mut state.pending
            && pending.id == id
            && pending.decision.is_none()
        {
            pending.decision = Some(false);
            pending.timed_out = true;
            state
                .changes
                .send_replace(Some(NativeRequest::TimedOut(id)));
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::install().unwrap()
    }

    #[cfg(test)]
    pub(crate) fn pump_queued(&self, _deadline: Instant) -> bool {
        false
    }
}
