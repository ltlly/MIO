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
}
