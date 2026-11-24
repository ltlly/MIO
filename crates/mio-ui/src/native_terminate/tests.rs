use std::{os::unix::ffi::OsStrExt, time::{Duration, Instant}};

use objc2::{
    AnyThread, DefinedClass, define_class,
    runtime::{ClassBuilder, NSObject, Sel},
    sel,
};
use objc2_foundation::NSDate;

use super::*;

#[gpui_kit::test]
fn file_panel_headless_forwarders_preserve_path_selection_and_cancel(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::AppContext as _;
    use std::ffi::OsStr;

    let window = cx.update(|cx| {
        cx.open_window(gpui_kit::WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui_kit::EmptyView)
        })
        .unwrap()
    });
    let directory = Path::new("/tmp/file-panel-test");
    let expected = PathBuf::from(OsStr::from_bytes(b"/tmp/file-panel-test/\xff.mio"));
    let mut save = window
        .update(cx, |_, window, cx| {
            prompt_for_new_path(window, cx, directory, Some("draft.mio"))
        })
        .unwrap();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|actual| {
        assert_eq!(actual, directory);
        Some(expected.clone())
    });
    cx.run_until_parked();
    assert_eq!(save.try_recv().unwrap().unwrap(), Some(expected.clone()));
    let mut cancel = window
        .update(cx, |_, window, cx| {
            prompt_for_new_path(window, cx, directory, Some("draft.mio"))
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert_eq!(cancel.try_recv().unwrap().unwrap(), None);

    let mut open = window
        .update(cx, |_, window, cx| {
            prompt_for_paths(
                window,
                cx,
                PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("打开测试项目".into()),
                },
            )
        })
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && options.multiple);
        assert_eq!(options.prompt.as_deref(), Some("打开测试项目"));
        Some(vec![expected.clone(), PathBuf::from("/tmp/other.mio")])
    });
    cx.run_until_parked();
    assert_eq!(
        open.try_recv().unwrap().unwrap(),
        Some(vec![expected, PathBuf::from("/tmp/other.mio")])
    );
    let mut cancelled_open = window
        .update(cx, |_, window, cx| {
            prompt_for_paths(
                window,
                cx,
                PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: None,
                },
            )
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert_eq!(cancelled_open.try_recv().unwrap().unwrap(), None);
}

#[derive(Default)]
pub(crate) struct ProbeCalls {
    terminations: Vec<Reply>,
    replies: Vec<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MIONativeTerminateReplyProbe"]
    #[ivars = RefCell<ProbeCalls>]
    pub(crate) struct Probe;

    impl Probe {
        #[unsafe(method(terminate:))]
        fn terminate(&self, _: *mut AnyObject) {
            let reply = self.request();
            self.ivars().borrow_mut().terminations.push(reply);
            NSRunLoop::currentRunLoop().getCFRunLoop().stop();
        }

        #[unsafe(method(replyToApplicationShouldTerminate:))]
        fn reply(&self, answer: bool) {
            self.ivars().borrow_mut().replies.push(answer);
            NSRunLoop::currentRunLoop().getCFRunLoop().stop();
        }
    }
);

impl Probe {
    pub(crate) fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(RefCell::new(ProbeCalls::default()));
        unsafe { msg_send![super(this), init] }
    }

    pub(crate) fn installed() -> Retained<Self> {
        current()
            .unwrap()
            .0
            .borrow()
            .terminate_target
            .clone()
            .unwrap()
            .downcast()
            .unwrap()
    }

    pub(crate) fn terminations(&self) -> Vec<Reply> {
        self.ivars().borrow().terminations.clone()
    }

    pub(crate) fn request(&self) -> Reply {
        let class = AnyClass::get(c"GPUIApplicationDelegate").unwrap();
        let delegate: Retained<AnyObject> = unsafe { msg_send![class, new] };
        unsafe { msg_send![&*delegate, applicationShouldTerminate: self] }
    }

    pub(crate) fn replies(&self) -> Vec<bool> {
        self.ivars().borrow().replies.clone()
    }

    pub(crate) fn pump_modal_until_replies(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.replies().len() < count && Instant::now() < deadline {
            let limit = NSDate::dateWithTimeIntervalSinceNow(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs_f64(),
            );
            NSRunLoop::currentRunLoop()
                .runMode_beforeDate(unsafe { NSModalPanelRunLoopMode }, &limit);
        }
        assert_eq!(
            self.replies().len(),
            count,
            "reply must be delivered in modal mode within the bound"
        );
    }

    pub(crate) fn expire(&self) {
        let guard = current().unwrap();
        let timer = {
            let mut state = guard.0.borrow_mut();
            let pending = state.pending.as_mut().unwrap();
            pending.deadline = Instant::now();
            pending.timer.clone()
        };
        timer.setFireDate(&NSDate::distantPast());
    }
}

#[test]
fn adds_only_absent_selector_and_preserves_gpui_delegate_methods() {
    let class = AnyClass::get(c"GPUIApplicationDelegate").unwrap();
    let original = class
        .instance_method(sel!(applicationWillTerminate:))
        .unwrap()
        .implementation();
    let guard = NativeTerminate::for_test();
    install_hook(class).unwrap();
    assert!(std::ptr::fn_addr_eq(
        original,
        class
            .instance_method(sel!(applicationWillTerminate:))
            .unwrap()
            .implementation()
    ));
    let method = class
        .instance_method(sel!(applicationShouldTerminate:))
        .unwrap();
    assert_eq!(&*method.return_type(), c"Q");
    assert_eq!(&*method.argument_type(2).unwrap(), c"@");
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    assert_eq!(probe.request(), Reply::TerminateLater);
    guard.reject();
    assert!(probe.replies().is_empty());
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
}

#[test]
fn native_entry_permission_requires_pending_request() {
    let guard = NativeTerminate::for_test();
    let requests = guard.subscribe();
    assert_eq!(guard.permit_exit(), Err(()));
    assert!(!guard.0.borrow().permitted);
    assert!(requests.borrow().is_none());
    let probe = Probe::installed();
    assert!(probe.terminations().is_empty() && probe.replies().is_empty());
}

#[test]
fn existing_inherited_hook_fails_without_replacement() {
    extern "C-unwind" fn existing(_: &AnyObject, _: Sel, _: *mut AnyObject) -> Reply {
        Reply::TerminateCancel
    }
    let mut base = ClassBuilder::new(
        c"MIOExistingTerminateHookProbe",
        AnyClass::get(c"NSObject").unwrap(),
    )
    .unwrap();
    unsafe {
        base.add_method(
            sel!(applicationShouldTerminate:),
            existing as extern "C-unwind" fn(_, _, _) -> _,
        );
    }
    let base = base.register();
    let child = ClassBuilder::new(c"MIOInheritedTerminateHookProbe", base)
        .unwrap()
        .register();
    let original = child
        .instance_method(sel!(applicationShouldTerminate:))
        .unwrap()
        .implementation();
    assert!(install_hook(child).is_err());
    assert!(std::ptr::fn_addr_eq(
        original,
        child
            .instance_method(sel!(applicationShouldTerminate:))
            .unwrap()
            .implementation()
    ));
}

#[test]
fn overdue_timer_rejects_permission_before_callback() {
    let guard = NativeTerminate::for_test();
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    probe.expire();
    let id = {
        let state = guard.0.borrow();
        let pending = state.pending.as_ref().unwrap();
        assert!(pending.timer.isValid());
        assert!(pending.timer.fireDate().timeIntervalSinceNow() < 0.0);
        assert_eq!(pending.decision, None);
        assert!(!pending.timed_out && !pending.delivered);
        pending.id
    };
    let result = guard.permit_exit();
    assert!(probe.replies().is_empty());
    // Execute the queued reply target without pumping the overdue timer.
    guard.deliver_reply(id);
    assert_eq!(
        result,
        Err(()),
        "an overdue timer must reject permission before its callback runs"
    );
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
}

#[test]
fn overdue_timer_rejects_queued_yes_before_callback() {
    let guard = NativeTerminate::for_test();
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    assert_eq!(guard.permit_exit(), Ok(true));
    probe.expire();
    let id = {
        let state = guard.0.borrow();
        let pending = state.pending.as_ref().unwrap();
        assert!(pending.timer.isValid());
        assert!(pending.timer.fireDate().timeIntervalSinceNow() < 0.0);
        assert_eq!(pending.decision, Some(true));
        assert!(!pending.timed_out && !pending.delivered);
        pending.id
    };
    assert!(probe.replies().is_empty());
    // Force queued-reply delivery before the overdue timer callback.
    guard.deliver_reply(id);
    assert!(
        !probe.replies().contains(&true),
        "a queued YES delivered after expiry must not escape before the timer callback runs"
    );
    assert!(guard.rejected());
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert_eq!(guard.permit_exit(), Err(()));
}

#[test]
fn modal_deadline_overrides_queued_yes_and_blocks_late_permission() {
    let guard = NativeTerminate::for_test();
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    assert_eq!(guard.permit_exit(), Ok(true));
    let timer = guard.0.borrow().pending.as_ref().unwrap().timer.clone();
    timer.fire();
    assert_eq!(guard.permit_exit(), Err(()));
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert_eq!(probe.request(), Reply::TerminateCancel);
}

#[test]
fn extend_deadline_for_user_interaction_pushes_deadline_forward_unless_expired() {
    let guard = NativeTerminate::for_test();
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    {
        let mut state = guard.0.borrow_mut();
        let pending = state.pending.as_mut().unwrap();
        pending.deadline = Instant::now() + Duration::from_secs(1);
    }
    let before = guard.0.borrow().pending.as_ref().unwrap().deadline;
    guard.extend_deadline_for_user_interaction();
    let after = guard.0.borrow().pending.as_ref().unwrap().deadline;
    assert!(
        after > before + Duration::from_secs(60),
        "user interaction must extend the pending termination deadline"
    );
    assert!(!guard.rejected());

    // Once expired, extending the deadline must not revive the request.
    probe.expire();
    let expired_deadline = guard.0.borrow().pending.as_ref().unwrap().deadline;
    guard.extend_deadline_for_user_interaction();
    assert_eq!(
        guard.0.borrow().pending.as_ref().unwrap().deadline,
        expired_deadline
    );
    assert!(guard.rejected());
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
}
