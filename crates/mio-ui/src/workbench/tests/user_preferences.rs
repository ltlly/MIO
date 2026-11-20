use std::time::Duration;

use mio_core::{
    FinishedAttackClose as Finished, RunningAttackClose as Running, UserPreferences,
    UserPreferencesState,
};

use super::*;
use crate::workbench::user_preferences::RememberCloseChoice;

mod part_1;
mod part_2;
mod part_3;
mod part_4;
mod part_5;

fn remember(
    fixture: &Fixture,
    choice: RememberCloseChoice,
    cx: &mut TestAppContext,
) -> Result<Uuid, String> {
    cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            view.remember_intruder_closing_preference(choice, cx)
        })
    })
}

fn emit(fixture: &Fixture, event: Event, cx: &mut TestAppContext) {
    fixture.events.try_send(event).unwrap();
    cx.run_until_parked();
}

fn click(window: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
}

fn setup(cx: &mut TestAppContext) -> (Fixture, AnyWindowHandle) {
    let mut fixture = fixture(cx, vec![tab("项目草稿")], 64);
    drain(&mut fixture.commands);
    let settings = settings::open_settings(&fixture, cx);
    click(settings, "settings-intruder", cx);
    (fixture, settings)
}

fn take_save(fixture: &mut Fixture) -> (Uuid, UserPreferences) {
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetUserPreferences {
            operation_id,
            expected_revision,
            running,
            finished,
        },
    ] = commands.as_slice()
    else {
        panic!("one immediate user preference save expected")
    };
    (
        *operation_id,
        UserPreferences {
            version: 1,
            revision: expected_revision + 1,
            running: *running,
            finished: *finished,
            ..UserPreferences::default()
        },
    )
}

fn take_tls_save(fixture: &mut Fixture, cx: &TestAppContext) -> (Uuid, UserPreferences) {
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetUserTlsCertificates {
            operation_id,
            expected_revision,
            certificates,
        },
    ] = commands.as_slice()
    else {
        panic!("one app-scoped TLS preferences save expected");
    };
    let mut saved = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .user_preferences
            .confirmed
            .clone()
            .unwrap()
    });
    assert_eq!(saved.revision, *expected_revision);
    saved.revision += 1;
    saved.custom_ca_ders = certificates.clone();
    (*operation_id, saved)
}
