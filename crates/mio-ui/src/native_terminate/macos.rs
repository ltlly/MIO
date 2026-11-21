//! Minimal macOS `applicationShouldTerminate:` guard for `GPUIApplicationDelegate`.
//!
//! `gpui-pre-macos` (unvendored from crates.io) only implements `applicationWillTerminate:` via
//! `App::on_app_quit`, which executes after AppKit has already committed to quitting and clears
//! windows synchronously. MIO needs cancellable `NSTerminateLater` handoff so dirty drafts, save
//! prompts, and background `Runtime::join()` can complete or cancel exit cleanly.

use objc2::{
    runtime::{Imp, Sel},
    sel,
};
use objc2_app_kit::NSApplication;
use objc2_foundation::NSArray;

use super::*;

impl NativeTerminate {
    pub(crate) fn install() -> anyhow::Result<Self> {
        ensure!(
            MainThreadMarker::new().is_some(),
            "原生退出保护必须在主线程安装"
        );
        Self::create(None)
    }

    fn create(terminate_target: Option<Retained<AnyObject>>) -> anyhow::Result<Self> {
        let class = AnyClass::get(c"GPUIApplicationDelegate")
            .context("缺少固定版 GPUIApplicationDelegate，无法安装原生退出保护")?;
        install_hook(class)?;
        ensure!(
            CURRENT.with(|slot| slot.borrow().upgrade().is_none()),
            "原生退出保护已绑定运行循环"
        );
        let (changes, _) = watch::channel(None);
        let state = Rc::new(RefCell::new(State {
            pending: None,
            queued: false,
            permitted: false,
            terminate_target,
            run_loop: NSRunLoop::currentRunLoop(),
            changes,
        }));
        CURRENT.with(|slot| *slot.borrow_mut() = Rc::downgrade(&state));
        Ok(Self(state))
    }

    pub(crate) fn request(&self) {
        let run_loop = {
            let mut state = self.0.borrow_mut();
            if state.queued || state.pending.is_some() {
                return;
            }
            state.queued = true;
            state.run_loop.clone()
        };
        let state = Rc::downgrade(&self.0);
        let block = RcBlock::new(move || {
            let Some(state) = state.upgrade() else {
                return;
            };
            let target = {
                let mut state = state.borrow_mut();
                state.queued = false;
                if state.pending.is_some() {
                    return;
                }
                state.terminate_target.clone()
            };
            let target = target.unwrap_or_else(|| {
                NSApplication::sharedApplication(MainThreadMarker::new().expect("main run loop"))
                    .into()
            });
            unsafe {
                let _: () = msg_send![&*target, terminate: std::ptr::null_mut::<AnyObject>()];
            }
        });
        // NSTerminateLater may nest a modal loop; release GPUI and GCD callbacks first.
        unsafe {
            let modes = NSArray::from_slice(&[NSRunLoopCommonModes, NSModalPanelRunLoopMode]);
            run_loop.performInModes_block(&modes, &block);
        }
        run_loop.getCFRunLoop().wake_up();
    }

    pub(crate) fn verify_delegate(&self) -> anyhow::Result<()> {
        let mtm = MainThreadMarker::new().context("原生 delegate 检查必须在主线程执行")?;
        let application = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let delegate: *mut AnyObject = unsafe { msg_send![&*application, delegate] };
        let delegate =
            unsafe { delegate.as_ref() }.context("GPUI 尚未安装 application delegate")?;
        ensure!(
            Some(delegate.class()) == AnyClass::get(c"GPUIApplicationDelegate"),
            "GPUI application delegate 已被替换"
        );
        install_hook(delegate.class())
    }

    pub(crate) fn stop_failed_launch(&self) {
        self.reject();
        let block = RcBlock::new(|| {
            use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
            let app =
                NSApplication::sharedApplication(MainThreadMarker::new().expect("main run loop"));
            app.stop(None);
            // stop: needs an event to release AppKit's nextEvent wait.
            if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                NSEventType::ApplicationDefined, Default::default(), NSEventModifierFlags::empty(),
                0.0, 0, None, 0, 0, 0,
            ) {
                app.postEvent_atStart(&event, true);
            }
        });
        let run_loop = self.0.borrow().run_loop.clone();
        unsafe {
            let modes = NSArray::from_slice(&[NSRunLoopCommonModes, NSModalPanelRunLoopMode]);
            run_loop.performInModes_block(&modes, &block);
        }
        run_loop.getCFRunLoop().wake_up();
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
        // GPUI callbacks can resume before AppKit delivers an overdue timer.
        let overdue = self
            .0
            .borrow()
            .pending
            .as_ref()
            .filter(|pending| !pending.delivered && Instant::now() >= pending.deadline)
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

    // The caller has joined Runtime and obtained the current exit confirmation.
    pub(crate) fn permit_exit(&self) -> Result<bool, ()> {
        let id = {
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
            pending.id
        };
        self.schedule_reply(id);
        Ok(true)
    }

    pub(crate) fn reject(&self) {
        let id = {
            let mut state = self.0.borrow_mut();
            state.permitted = false;
            let Some(pending) = &mut state.pending else {
                return;
            };
            pending.decision = Some(false);
            pending.id
        };
        self.schedule_reply(id);
    }

    pub(crate) fn acknowledge_timeout(&self, id: u64) {
        let mut state = self.0.borrow_mut();
        if let Some(pending) = &mut state.pending
            && pending.id == id
        {
            pending.timed_out = false;
            if pending.delivered {
                state.pending = None;
            }
        }
    }

    pub(super) fn timeout(&self, id: u64) {
        {
            let mut state = self.0.borrow_mut();
            let Some(pending) = &mut state.pending else {
                return;
            };
            if pending.id != id || pending.delivered || pending.decision == Some(false) {
                return;
            }
            pending.decision = Some(false);
            pending.timed_out = true;
            state
                .changes
                .send_replace(Some(NativeRequest::TimedOut(id)));
        }
        self.schedule_reply(id);
    }

    fn schedule_reply(&self, id: u64) {
        let block = RcBlock::new(move || {
            if let Some(guard) = current() {
                guard.deliver_reply(id);
            }
        });
        let run_loop = self.0.borrow().run_loop.clone();
        // Native delivery runs after the GPUI update, including NSTerminateLater's modal mode.
        unsafe {
            let modes = NSArray::from_slice(&[NSRunLoopCommonModes, NSModalPanelRunLoopMode]);
            run_loop.performInModes_block(&modes, &block);
        }
        run_loop.getCFRunLoop().wake_up();
    }

    pub(super) fn deliver_reply(&self, id: u64) {
        let (application, decision) = {
            let mut state = self.0.borrow_mut();
            let Some(pending) = &mut state.pending else {
                return;
            };
            if pending.id != id || pending.delivered {
                return;
            }
            let Some(decision) = pending.decision else {
                return;
            };
            if decision && Instant::now() >= pending.deadline {
                drop(state);
                self.timeout(id);
                return;
            }
            pending.timer.invalidate();
            pending.delivered = true;
            let application = pending.application.clone();
            if !pending.timed_out {
                state.pending = None;
            }
            state.permitted = decision;
            (application, decision)
        };
        eprintln!(
            "MIO native-exit request={id} reply={}",
            if decision { "YES" } else { "NO" }
        );
        // AppKit may synchronously close GPUI windows during a YES reply.
        unsafe {
            let _: () = msg_send![&*application, replyToApplicationShouldTerminate: decision];
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::create(Some(tests::Probe::new().into())).unwrap()
    }

    #[cfg(test)]
    pub(crate) fn pump_queued(&self, deadline: Instant) -> bool {
        let work_pending = || {
            let state = self.0.borrow();
            state.queued
                || state
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.decision.is_some() && !pending.delivered)
        };
        let had_work = work_pending();
        while work_pending() {
            assert!(
                Instant::now() < deadline,
                "native run-loop work exceeded its bound"
            );
            let limit = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs_f64(),
            );
            let run_loop = self.0.borrow().run_loop.clone();
            run_loop.runMode_beforeDate(unsafe { NSModalPanelRunLoopMode }, &limit);
        }
        had_work
    }
}

pub(super) fn current() -> Option<NativeTerminate> {
    CURRENT.with(|slot| slot.borrow().upgrade().map(NativeTerminate))
}

fn hook_imp() -> Imp {
    // SAFETY: `should_terminate` uses `extern "C-unwind"` with Objective-C signature `Q@:@`
    // (`NSUInteger` return, receiver `&AnyObject`, selector `Sel`, argument `*mut AnyObject`),
    // matching `applicationShouldTerminate:` checked by `install_hook`.
    unsafe {
        std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> Reply, Imp>(
            should_terminate,
        )
    }
}

pub(super) fn install_hook(class: &AnyClass) -> anyhow::Result<()> {
    let _lock = HOOK_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("原生退出 hook 安装锁损坏"))?;
    let selector = sel!(applicationShouldTerminate:);
    if let Some(method) = class.instance_method(selector) {
        ensure!(
            std::ptr::fn_addr_eq(method.implementation(), hook_imp()),
            "applicationShouldTerminate: 已有未知实现，拒绝覆盖"
        );
        ensure!(
            &*method.return_type() == c"Q" && method.arguments_count() == 3,
            "原生退出 hook ABI 不匹配"
        );
        return Ok(());
    }
    // Add only the absent selector; GPUI retains its original delegate and platform ivar.
    let installed = unsafe {
        objc2::ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            selector,
            hook_imp(),
            c"Q@:@".as_ptr(),
        )
    };
    ensure!(installed.as_bool(), "class_addMethod 无法安装原生退出保护");
    Ok(())
}

extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, application: *mut AnyObject) -> Reply {
    let Some(guard) = current() else {
        eprintln!("MIO 原生退出请求被拒绝：退出保护尚未绑定。");
        return Reply::TerminateCancel;
    };
    let mut state = guard.0.borrow_mut();
    if let Some(pending) = &state.pending {
        return if pending.delivered {
            Reply::TerminateCancel
        } else {
            Reply::TerminateLater
        };
    }
    if state.permitted {
        return Reply::TerminateNow;
    }
    // AppKit keeps the sender alive for this call; retain it until the deferred reply.
    let Some(application) = (unsafe { Retained::retain(application) }) else {
        return Reply::TerminateCancel;
    };
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs_f64(TERMINATE_TIMEOUT);
    let block = RcBlock::new(move |_| {
        if let Some(guard) = current() {
            guard.timeout(id);
        }
    });
    let timer =
        unsafe { NSTimer::timerWithTimeInterval_repeats_block(TERMINATE_TIMEOUT, false, &block) };
    unsafe {
        state
            .run_loop
            .addTimer_forMode(&timer, NSRunLoopCommonModes);
        state
            .run_loop
            .addTimer_forMode(&timer, NSModalPanelRunLoopMode);
    }
    state.pending = Some(Pending {
        id,
        application,
        timer,
        deadline,
        decision: None,
        timed_out: false,
        delivered: false,
    });
    state
        .changes
        .send_replace(Some(NativeRequest::Requested(id)));
    eprintln!("MIO native-exit request={id} Later");
    Reply::TerminateLater
}
