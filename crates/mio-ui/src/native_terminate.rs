#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
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
use objc2_app_kit::{
    NSApplicationTerminateReply as Reply, NSModalPanelRunLoopMode, NSModalResponse,
    NSModalResponseCancel, NSModalResponseOK, NSView, NSWindow,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSTimer, NSURL};
#[cfg(target_os = "macos")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tokio::sync::{oneshot, watch};

#[cfg(all(target_os = "macos", not(test)))]
use objc2::class;
#[cfg(all(target_os = "macos", not(test)))]
use objc2_app_kit::{NSOpenPanel, NSSavePanel};
#[cfg(all(target_os = "macos", not(test)))]
use objc2_foundation::{NSProcessInfo, NSString};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(target_os = "macos", test))]
pub(crate) mod tests;

#[cfg(all(target_os = "macos", test))]
use macos::{current, install_hook};

pub(crate) fn prompt_for_paths(
    window: &Window,
    cx: &App,
    options: PathPromptOptions,
) -> oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>> {
    let (done_tx, done_rx) = oneshot::channel();
    #[cfg(any(test, not(target_os = "macos")))]
    {
        let _ = window;
        let prompt = cx.prompt_for_paths(options);
        cx.foreground_executor()
            .spawn(async move {
                if let Ok(result) = prompt.await {
                    let _ = done_tx.send(result);
                }
            })
            .detach();
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let parent = match native_prompt_parent(window) {
            Ok(parent) => parent,
            Err(error) => {
                let _ = done_tx.send(Err(error));
                return done_rx;
            }
        };
        cx.foreground_executor()
            .spawn(async move {
                if !parent.isVisible() || parent.attachedSheet().is_some() {
                    let _ = done_tx.send(Err(anyhow::anyhow!(
                        "触发窗口已关闭或已有模态窗口；未打开文件选择器。"
                    )));
                    return;
                }
                let panel: Option<Retained<NSOpenPanel>> =
                    unsafe { msg_send![class!(NSOpenPanel), openPanel] };
                let Some(panel) = panel else {
                    let _ = done_tx.send(Err(anyhow::anyhow!("macOS 无法创建文件选择器。")));
                    return;
                };
                panel.setCanChooseDirectories(options.directories);
                panel.setCanChooseFiles(options.files);
                panel.setAllowsMultipleSelection(options.multiple);
                panel.setCanCreateDirectories(true);
                panel.setResolvesAliases(false);
                if let Some(prompt) = options.prompt {
                    panel.setPrompt(Some(&NSString::from_str(&prompt)));
                }
                let done_tx = std::cell::Cell::new(Some(done_tx));
                let handler = RcBlock::new({
                    let panel = panel.clone();
                    move |response: NSModalResponse| {
                        let Some(done_tx) = done_tx.take() else {
                            return;
                        };
                        let result = file_panel_result(response, || {
                            let paths = panel
                                .URLs()
                                .iter()
                                .map(|url| selected_file_path(&url))
                                .collect::<anyhow::Result<Vec<_>>>()?;
                            ensure!(!paths.is_empty(), "文件选择器未返回有效路径。");
                            ensure!(
                                options.multiple || paths.len() == 1,
                                "文件选择器返回了多个路径，未应用选择。"
                            );
                            Ok(paths)
                        });
                        panel.orderOut(None);
                        let _ = done_tx.send(result);
                    }
                });
                panel.beginSheetModalForWindow_completionHandler(&parent, &handler);
            })
            .detach();
    }
    done_rx
}

pub(crate) fn prompt_for_new_path(
    window: &Window,
    cx: &App,
    directory: &Path,
    suggested_name: Option<&str>,
) -> oneshot::Receiver<anyhow::Result<Option<PathBuf>>> {
    let (done_tx, done_rx) = oneshot::channel();
    #[cfg(any(test, not(target_os = "macos")))]
    {
        let _ = window;
        let prompt = cx.prompt_for_new_path(directory, suggested_name);
        cx.foreground_executor()
            .spawn(async move {
                if let Ok(result) = prompt.await {
                    let _ = done_tx.send(result);
                }
            })
            .detach();
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let parent = match native_prompt_parent(window) {
            Ok(parent) => parent,
            Err(error) => {
                let _ = done_tx.send(Err(error));
                return done_rx;
            }
        };
        let directory = directory.to_path_buf();
        let suggested_name = suggested_name.map(str::to_owned);
        cx.foreground_executor()
            .spawn(async move {
                if !parent.isVisible() || parent.attachedSheet().is_some() {
                    let _ = done_tx.send(Err(anyhow::anyhow!(
                        "触发窗口已关闭或已有模态窗口；未打开保存选择器。"
                    )));
                    return;
                }
                let Some(directory_url) = NSURL::from_directory_path(&directory) else {
                    let _ = done_tx.send(Err(anyhow::anyhow!("保存目录无法表示为文件 URL。")));
                    return;
                };
                let panel: Option<Retained<NSSavePanel>> =
                    unsafe { msg_send![class!(NSSavePanel), savePanel] };
                let Some(panel) = panel else {
                    let _ = done_tx.send(Err(anyhow::anyhow!("macOS 无法创建保存选择器。")));
                    return;
                };
                panel.setDirectoryURL(Some(&directory_url));
                if let Some(suggested_name) = suggested_name {
                    panel.setNameFieldStringValue(&NSString::from_str(&suggested_name));
                }
                let done_tx = std::cell::Cell::new(Some(done_tx));
                let handler = RcBlock::new({
                    let panel = panel.clone();
                    move |response: NSModalResponse| {
                        let Some(done_tx) = done_tx.take() else {
                            return;
                        };
                        let result = file_panel_result(response, || {
                            let url = panel.URL().context("保存选择器未返回文件 URL。")?;
                            let path = selected_file_path(&url)?;
                            Ok(normalize_save_path(
                                path,
                                NSProcessInfo::processInfo()
                                    .operatingSystemVersion()
                                    .majorVersion,
                            ))
                        });
                        panel.orderOut(None);
                        let _ = done_tx.send(result);
                    }
                });
                panel.beginSheetModalForWindow_completionHandler(&parent, &handler);
            })
            .detach();
    }
    done_rx
}

#[cfg(target_os = "macos")]
fn native_prompt_parent(window: &Window) -> anyhow::Result<Retained<NSWindow>> {
    MainThreadMarker::new().context("文件选择器必须在主线程创建。")?;
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("触发窗口缺少原生句柄；未打开文件选择器：{error}"))?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        anyhow::bail!("触发窗口不是 AppKit 窗口；未打开文件选择器。")
    };
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window().context("触发窗口已关闭；未打开文件选择器。")
}

#[cfg(target_os = "macos")]
fn file_panel_result<T>(
    response: NSModalResponse,
    selection: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    if response == NSModalResponseOK {
        selection().map(Some)
    } else if response == NSModalResponseCancel {
        Ok(None)
    } else {
        anyhow::bail!("文件选择器无法显示或意外关闭（响应 {response}）。")
    }
}

#[cfg(target_os = "macos")]
fn selected_file_path(url: &NSURL) -> anyhow::Result<PathBuf> {
    ensure!(url.isFileURL(), "文件选择器返回的不是本地文件 URL。");
    url.to_file_path()
        .context("文件选择器返回的路径无法完整读取；未应用选择。")
}

#[cfg(target_os = "macos")]
fn normalize_save_path(mut path: PathBuf, macos_major: isize) -> PathBuf {
    let Some(filename) = path.file_name() else {
        return path;
    };
    let chunks = filename
        .as_bytes()
        .split(|&byte| byte == b'.')
        .collect::<Vec<_>>();
    if let &[_, second, third] = chunks.as_slice()
        && second.starts_with(third)
        && macos_major >= 15
    {
        path.set_extension("");
    }
    path
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
