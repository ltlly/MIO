use super::*;

#[gpui_kit::test]
fn builtin_ca_preset_import_enqueues_the_preset_pem(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let preset = mio_core::ca_preset("emudhra-emsign-dv-g3b").unwrap();

    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| view.use_builtin_ca_preset(*preset, cx))
    });

    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        Command::ImportCa { cert_pem, key_pem } => {
            assert_eq!(cert_pem, preset.pem);
            assert!(key_pem.is_empty());
        }
        _ => panic!("unexpected command variant"),
    }

    fixture
        .events
        .try_send(Event::CaRegenerated {
            fingerprint: preset.fingerprint_sha256.to_owned(),
        })
        .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let fp = view.last_ca_fingerprint.as_deref().unwrap();
        assert_eq!(fp, preset.fingerprint_sha256);
        assert_eq!(mio_core::ca_preset_by_fingerprint(fp), Some(preset));
    });

    // Disconnected backend makes `use_builtin_ca_preset` a safe no-op.
    drop(fixture.events);
    cx.run_until_parked();
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, cx| {
                assert!(!view.backend_available());
                view.use_builtin_ca_preset(*preset, cx);
            })
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
