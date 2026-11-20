use std::cell::RefCell;

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

use super::*;
use crate::workbench::intruder::tests::{attack, intruder_fixture};
use crate::workbench::tests::{Fixture, drain};

type Answers = Rc<RefCell<Vec<(Uuid, Option<(usize, bool)>)>>>;

fn dialog(
    fixture: &Fixture,
    terminal: bool,
    can_save: bool,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<CloseDialog>, Uuid, Answers) {
    let operation = Uuid::new_v4();
    let answers = Rc::new(RefCell::new(Vec::new()));
    let recorded = answers.clone();
    let (window, view) = cx
        .update_window(fixture.window, |_, parent, cx| {
            open(
                fixture.view.downgrade(),
                operation,
                terminal,
                can_save,
                parent,
                cx,
                move |_, operation, selection, _| {
                    recorded.borrow_mut().push((operation, selection));
                    Ok(true)
                },
            )
            .unwrap()
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    (window, view, operation, answers)
}

#[gpui_kit::test]
fn close_dialog_remember_starts_focused_and_space_tab_delivers_button_mapping(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let (window, _, operation, answers) = dialog(&fixture, true, true, cx);
    cx.update_window(window, |_, window, cx| {
        let remember = window.find("intruder-close-remember");
        assert_eq!(remember.focused(), Some(true));
        assert_eq!(remember.checked(), Some(false));
        window.press("space", cx);
        assert_eq!(window.find("intruder-close-remember").checked(), Some(true));
        window.press("tab", cx);
        assert_eq!(
            window.find(("intruder-close-choice", 0_usize)).focused(),
            Some(true)
        );
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(answers.borrow().as_slice(), &[(operation, Some((0, true)))]);
    assert!(window.update(cx, |_, _, _| ()).is_err());
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn close_dialog_escape_cmd_w_cancel_and_reopen_reset_remember(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    for key in ["escape", crate::kbd!("w")] {
        let (window, _, operation, answers) = dialog(&fixture, false, false, cx);
        cx.update_window(window, |_, window, cx| {
            assert_eq!(window.find("intruder-close-remember").focused(), Some(true));
            assert_eq!(
                window.find("intruder-close-remember").checked(),
                Some(false)
            );
            window.press("space", cx);
            window.press(key, cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(answers.borrow().as_slice(), &[(operation, None)]);
        assert!(window.update(cx, |_, _, _| ()).is_err());
    }
    let (window, _, operation, answers) = dialog(&fixture, true, false, cx);
    cx.update_window(window, |_, window, cx| {
        assert_eq!(
            window.find("intruder-close-remember").checked(),
            Some(false)
        );
        assert!(
            window
                .try_find(("intruder-close-choice", 0_usize))
                .is_none()
        );
        window.press("tab", cx);
        assert_eq!(
            window.find(("intruder-close-choice", 1_usize)).focused(),
            Some(true)
        );
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        answers.borrow().as_slice(),
        &[(operation, Some((1, false)))]
    );
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn close_dialog_cancel_before_deferred_answer_never_delivers_superseded_choice(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let (window, _, operation, answers) = dialog(&fixture, true, true, cx);
    cx.update_window(window, |_, window, cx| {
        window.click(("intruder-close-choice", 2_usize), cx);
        window.click(("intruder-close-choice", 2_usize), cx);
        window.press("escape", cx);
        assert!(
            answers.borrow().is_empty(),
            "the owner callback is deferred until after these real input events"
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        answers.borrow().as_slice(),
        &[(operation, None)],
        "the cancelled Discard choice must never reach the business callback"
    );
    assert!(window.update(cx, |_, _, _| ()).is_err());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, original));
    assert!(drain(&mut fixture.commands).is_empty());
}
