use super::*;
use crate::native_terminate::{NativeRequest, NativeTerminate, tests::Probe};
use objc2_app_kit::NSApplicationTerminateReply as Reply;

mod part_1;
mod part_2;
mod part_3;
mod part_4;
mod part_5;

fn native_save_picker(fixture: &mut Fixture, probe: &Probe, cx: &mut TestAppContext) {
    drain(&mut fixture.commands);
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    cx.simulate_prompt_answer("保存");
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { .. }]
    ));
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
}

struct AuxiliaryWindow;

impl Render for AuxiliaryWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().key_context("MioWorkspace")
    }
}

fn observe_runtime(
    fixture: &Fixture,
    cx: &mut TestAppContext,
) -> tokio::sync::oneshot::Sender<anyhow::Result<()>> {
    let (finished, joined) = tokio::sync::oneshot::channel();
    cx.update(|cx| {
        let joined = cx.background_spawn(async move { joined.await? });
        Workbench::observe_runtime(&fixture.view, fixture.window, joined, cx);
    });
    cx.run_until_parked();
    finished
}

fn uninitialized_startup_fixture(cx: &mut TestAppContext) -> Fixture {
    cx.update(|cx| {
        cx.set_global(NativeTerminate::for_test());
        gpui_kit::init(cx);
        theme::init(cx);
        crate::init_actions(cx);
    });
    let (commands_tx, commands) = mpsc::channel(64);
    let (events, events_rx) = mpsc::channel(128);
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(902.), px(610.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    Workbench::new_startup(
                        Client {
                            commands: commands_tx,
                            events: events_rx,
                            shutdown: CancellationToken::new(),
                            ipc_socket_path: None,
                        },
                        None,
                        window,
                        cx,
                    )
                })
            },
        )
        .unwrap()
    });
    cx.update(|cx| Workbench::observe_native(&view, window, cx));
    cx.run_until_parked();
    Fixture {
        window,
        view,
        commands,
        events,
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    }
}

fn answer_file_picker(menu: &str, answer: Option<PathBuf>, cx: &mut TestAppContext) {
    if menu == "打开项目…" {
        cx.simulate_path_prompt_response(|_| answer.map(|path| vec![path]));
    } else {
        cx.simulate_new_path_selection(|_| answer);
    }
    cx.run_until_parked();
}
