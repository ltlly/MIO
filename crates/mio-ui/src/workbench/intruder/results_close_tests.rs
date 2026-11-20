use gpui_kit::test::TestEventExt;
use mio_core::{
    IntruderDiscardReceipt, IntruderResultViewPage, IntruderResultViewRow, IntruderRunPage,
};

use super::*;
#[cfg(target_os = "macos")]
use crate::native_terminate::tests::Probe;
use crate::workbench::tests::pump_native;

#[path = "results_close_tests/part_1.rs"]
mod part_1;
#[path = "results_close_tests/part_2.rs"]
mod part_2;
#[path = "results_close_tests/part_3.rs"]
mod part_3;
#[path = "results_close_tests/part_4.rs"]
mod part_4;
#[path = "results_close_tests/part_5.rs"]
mod part_5;
#[path = "results_close_tests/part_6.rs"]
mod part_6;
#[path = "results_close_tests/part_7.rs"]
mod part_7;
#[path = "results_close_tests/part_8.rs"]
mod part_8;
#[path = "results_close_policy_tests.rs"]
mod policy_tests;
#[path = "results_remember_tests.rs"]
mod remember_tests;
#[path = "results_close_safety_tests.rs"]
mod safety_tests;
#[path = "results_saved_close_tests.rs"]
mod saved_close_tests;

pub(in crate::workbench::intruder) fn close_dialog(
    cx: &mut TestAppContext,
) -> Option<AnyWindowHandle> {
    cx.windows().into_iter().find(|handle| {
        cx.update_window(*handle, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("intruder-close-dialog").is_some()
        })
        .unwrap_or(false)
    })
}

pub(in crate::workbench::intruder) fn has_close_dialog(cx: &mut TestAppContext) -> bool {
    close_dialog(cx).is_some()
}

pub(in crate::workbench::intruder) fn answer_close_dialog(answer: &str, cx: &mut TestAppContext) {
    let dialog = close_dialog(cx).expect("result close confirmation must be visible");
    cx.update_window(dialog, |_, window, cx| {
        if answer == "取消" {
            window.press("escape", cx);
        } else {
            let label = if answer == "在后台继续运行" {
                "在后台继续攻击"
            } else {
                answer
            };
            let choice = window.find_by_label(label).bounds().center();
            window.pointer_down(choice, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(choice, gpui_kit::MouseButton::Left, cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
}
pub(in crate::workbench::intruder) use part_1::*;
