use super::*;
use mio_core::{FinishedAttackClose, RunningAttackClose, UserPreferences, UserPreferencesState};

#[path = "results_close_policy_tests/part_1.rs"]
mod part_1;
#[path = "results_close_policy_tests/part_2.rs"]
mod part_2;

pub(super) fn preferences(
    fixture: &mut Fixture,
    running: RunningAttackClose,
    finished: FinishedAttackClose,
    revision: u64,
    load_error: Option<String>,
    cx: &mut TestAppContext,
) -> Vec<Command> {
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.query_user_preferences(cx))
    });
    let commands = drain(&mut fixture.commands);
    let [Command::QueryUserPreferences { operation_id }] = commands.as_slice() else {
        panic!("policy setup must use a correlated preferences query");
    };
    emit(
        fixture,
        Event::UserPreferences {
            operation_id: *operation_id,
            state: UserPreferencesState {
                preferences: UserPreferences {
                    version: 1,
                    revision,
                    running,
                    finished,
                    ..UserPreferences::default()
                },
                load_error,
            },
        },
        cx,
    );
    drain(&mut fixture.commands)
}

pub(super) fn close_without_prompt(started: &Started, cx: &mut TestAppContext) {
    // Native simulate_close re-registers its handler after synchronous auto-close removed the window.
    cx.update_window(started.window, |_, window, cx| {
        assert!(window.is_action_available(&CloseTab, cx));
        window.press(crate::kbd!("w"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        !has_close_dialog(cx) && !cx.has_pending_prompt(),
        "confirmed automatic policy must not ask again"
    );
}

pub(super) fn completed(fixture: &mut Fixture, started: &mut Started, cx: &mut TestAppContext) {
    let revision = poll(fixture, started, cx);
    stopped(fixture, started, revision, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}
