use anyhow::{Result, bail};
use tokio_util::sync::CancellationToken;

use super::*;

fn encode_wasm(secs: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    for &(id, body) in secs {
        module::push_section(id, body, &mut out);
    }
    out
}
fn code1(loc: &[u8], ops: &[u8]) -> Vec<u8> {
    let mut out = vec![1u8];
    module::push_uleb(loc.len() + ops.len(), &mut out);
    out.extend_from_slice(loc);
    out.extend_from_slice(ops);
    out
}
fn inst(secs: &[(u8, &[u8])]) -> Result<WasmInstance> {
    WasmInstance::new(WasmModule::parse(&encode_wasm(secs))?)
}

#[test]
fn transforms_memory_and_checks_bounds() -> Result<()> {
    let ops = [
        &b"\x20\x00\x20\x00\x28\x00\x00\x41\xa0\xc0\x80\x01\x73\x36\x00\x00"[..],
        &b"\x20\x00\x20\x00\x2d\x00\x04\x41\x01\x6a\x3a\x00\x04\x3f\x00\x0b"[..],
    ]
    .concat();
    let secs = &[
        (1, &b"\x01\x60\x01\x7f\x01\x7f"[..]),
        (3, b"\x01\x00"),
        (5, b"\x01\x01\x01\x02"),
        (7, &b"\x01\x01x\x00\x00"[..]),
        (10, &code1(&[0], &ops)),
    ];
    let mut vm = inst(secs)?;
    vm.write_memory(64, b"mio!A")?;
    let tok = CancellationToken::new();
    let ret = vm.invoke("x", &[64], 500, &tok, &mut |_, _, _, _| Ok(0))?;
    assert_eq!((ret, vm.read_memory(64, 5)?), (Some(1), &b"MIO!B"[..]));
    assert!(vm.read_memory(PAGE_BYTES, 1).is_err());
    Ok(())
}

#[test]
fn invokes_mio_host_call() -> Result<()> {
    let c = code1(&[0], b"\x41\x10\x41\x04\x41\x20\x41\x10\x10\x00\x0b");
    let secs = &[
        (
            1,
            &b"\x02\x60\x04\x7f\x7f\x7f\x7f\x01\x7f\x60\x00\x01\x7f"[..],
        ),
        (2, &b"\x01\x03mio\x0dmio_host_call\x00\x00"[..]),
        (3, b"\x01\x01"),
        (5, b"\x01\x00\x01"),
        (7, &b"\x01\x03run\x00\x01"[..]),
        (10, &c),
        (11, b"\x01\x00\x41\x10\x0b\x04ping"),
    ];
    let mut vm = inst(secs)?;
    let tok = CancellationToken::new();
    let ret = vm.invoke("run", &[], 5_000, &tok, &mut |req, out, env_abi, _| {
        assert!(!env_abi);
        assert_eq!(req, b"ping");
        out[..5].copy_from_slice(b"pong!");
        Ok(5)
    })?;
    assert_eq!((ret, vm.read_memory(32, 5)?), (Some(5), &b"pong!"[..]));
    Ok(())
}

#[test]
fn truncates_infinite_loop_on_fuel() -> Result<()> {
    let c = code1(&[0], b"\x03\x40\x0c\x00\x0b\x0b");
    let secs: &[(u8, &[u8])] = &[
        (1, &b"\x01\x60\x00\x00"[..]),
        (3, b"\x01\x00"),
        (7, &b"\x01\x01f\x00\x00"[..]),
        (10, &c),
    ];
    let mut vm = inst(secs)?;
    let tok = CancellationToken::new();
    let mut noop = |_: &[u8], _: &mut [u8], _: bool, _: &CancellationToken| Ok(0);
    let err = vm.invoke("f", &[], 25, &tok, &mut noop).unwrap_err();
    assert!(err.to_string().contains("fuel"));
    Ok(())
}

#[test]
fn rejects_wasi_and_foreign_imports() {
    let ty = b"\x01\x60\x04\x7f\x7f\x7f\x7f\x01\x7f";
    let imp = |ns: &[u8]| [&[1, ns.len() as u8][..], ns, b"\x08fd_write\x00\x00"].concat();
    let wasi = encode_wasm(&[(1, ty), (2, &imp(b"wasi_snapshot_preview1"))]);
    let bad_fn = encode_wasm(&[(1, ty), (2, &imp(b"env"))]);
    assert!(WasmModule::parse(&wasi).is_err() && WasmModule::parse(&bad_fn).is_err());
}

#[test]
fn demo_wasm_module_executes_all_hooks_and_kv_host_calls() -> Result<()> {
    let bytes = build_demo_wasm_module();
    let module = WasmModule::parse(&bytes)?;
    let mut vm = WasmInstance::new(module)?;
    let tok = CancellationToken::new();
    let mut kv = std::collections::BTreeMap::<Vec<u8>, Vec<u8>>::new();
    let mut host = |req: &[u8], out: &mut [u8], _: bool, _: &CancellationToken| -> Result<usize> {
        match req[0] {
            1 => {
                let val = kv.get(&req[1..]).cloned().unwrap_or_default();
                out[..val.len()].copy_from_slice(&val);
                Ok(val.len())
            }
            2 => {
                let klen = req[1] as usize;
                kv.insert(req[2..2 + klen].to_vec(), req[2 + klen..].to_vec());
                Ok(0)
            }
            _ => bail!("unexpected op"),
        }
    };
    let (_, h1) = run_wasm_op(&mut vm, 1, b"GET /", 1024, &tok, &mut host)?;
    assert_eq!(h1, b"x-mio-wasm-hook: sandboxed");
    let (_, h2) = run_wasm_op(&mut vm, 2, b"GET /", 1024, &tok, &mut host)?;
    assert_eq!(h2, b"x-mio-wasm-resp: verified");
    let (_, ws) = run_wasm_op(&mut vm, 3, b"hello", 1024, &tok, &mut host)?;
    assert_eq!(ws, b"[wasm] hello");
    let (_, sess) = run_wasm_op(&mut vm, 4, b"http://127.0.0.1/", 1024, &tok, &mut host)?;
    assert_eq!(sess, b"x-mio-wasm-session: token-v1");
    let (_, r#gen) = run_wasm_op(&mut vm, 5, b"", 1024, &tok, &mut host)?;
    assert_eq!(r#gen, b"wasm-gen-1\nwasm-gen-2\nwasm-gen-3");
    let (_, proc) = run_wasm_op(&mut vm, 6, b"item", 1024, &tok, &mut host)?;
    assert_eq!(proc, b"wasm:item");
    let (_, set_msg) = run_wasm_op(&mut vm, 7, &10u32.to_le_bytes(), 1024, &tok, &mut host)?;
    assert_eq!(set_msg, b"KV saved: ok-from-wasm!!");
    let (_, get_msg) = run_wasm_op(&mut vm, 7, &11u32.to_le_bytes(), 1024, &tok, &mut host)?;
    assert_eq!(get_msg, b"ok-from-wasm");
    Ok(())
}

#[test]
fn envelope_wasm_module_allocs_dispatches_and_calls_preferences_host_abi() -> Result<()> {
    use mio_plugin_api::{
        ProjectKey, Uuid,
        panel::{Empty, UiEvent, ui_event},
        protocol::{
            ChannelIdentity, Envelope, HostCall, HostResult, envelope, host_call, host_result,
        },
    };
    use std::time::Duration;

    let bytes = build_envelope_wasm_module();
    let module = WasmModule::parse(&bytes)?;
    assert!(module.is_envelope_abi());
    let mut vm = WasmInstance::new(module)?;
    let tok = CancellationToken::new();
    let mut seen_pref = None;
    let mut host =
        |req: &[u8], out: &mut [u8], env_abi: bool, _: &CancellationToken| -> Result<usize> {
            assert!(env_abi);
            let call = HostCall::decode_bytes(req).map_err(anyhow::Error::msg)?;
            if let Some(host_call::Operation::PreferencesSet(entry)) = call.operation {
                seen_pref = Some((entry.key, entry.value));
            }
            let res = HostResult {
                outcome: Some(host_result::Outcome::Done(Empty {})),
            }
            .encode_bytes();
            out[..res.len()].copy_from_slice(&res);
            Ok(res.len())
        };
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let event_env = Envelope::new(
        identity,
        3,
        1,
        envelope::Body::UiEvent(UiEvent {
            node_id: 10,
            event: Some(ui_event::Event::Click(Empty {})),
            editors: vec![],
        }),
        Duration::from_secs(5),
    );
    let reply = dispatch_wasm_envelope(&mut vm, &event_env, WASM_CALL_FUEL, &tok, &mut host)?;
    assert!(matches!(reply, Some(envelope::Body::Patch(p)) if p.revision == 2));
    assert_eq!(
        seen_pref,
        Some(("wasm_tag".to_owned(), b"envelope-v13".to_vec()))
    );
    Ok(())
}

#[test]
fn call_indirect_executes_valid_targets_and_traps_on_mismatch_or_oob() -> Result<()> {
    // types: 0 = (i32) -> i32, 1 = (i32, i32) -> i32, 2 = (i32, i32) -> i32 (entry: idx, arg)
    let types = [
        3u8, 0x60, 1, I32, 1, I32, // type 0: (i32) -> i32
        0x60, 2, I32, I32, 1, I32, // type 1: (i32, i32) -> i32
        0x60, 2, I32, I32, 1, I32, // type 2: dispatch(idx, x) -> i32
    ];
    let funcs = [3u8, 0, 1, 2]; // f0: type 0, f1: type 1, f2: type 2
    let table = [1u8, FUNCREF, 1, 4, 4]; // 1 table of funcref, min=4, max=4
    let exports = [1u8, 3, b'r', b'u', b'n', 0, 2]; // export "run" -> func 2
    let elems = [1u8, 0, 0x41, 0, 0x0b, 2, 0, 1]; // table[0]=f0, table[1]=f1, table[2..4]=None

    // f0(x): select(x + 10, x + 20, x > 0)
    let f0 = [
        0u8, 0x20, 0, 0x41, 10, 0x6a, 0x20, 0, 0x41, 20, 0x6a, 0x20, 0, 0x41, 0, 0x4a, 0x1b, 0x0b,
    ];
    // f1(a, b): a + b
    let f1 = [0u8, 0x20, 0, 0x20, 1, 0x6a, 0x0b];
    // f2(idx, x): call_indirect (type 0, table 0) with arg x and table index idx
    let f2 = [0u8, 0x20, 1, 0x20, 0, 0x11, 0, 0, 0x0b];

    let mut code = vec![3u8];
    for body in [&f0[..], &f1[..], &f2[..]] {
        module::push_uleb(body.len(), &mut code);
        code.extend_from_slice(body);
    }
    let mut vm = inst(&[
        (1, &types),
        (3, &funcs),
        (4, &table),
        (7, &exports),
        (9, &elems),
        (10, &code),
    ])?;
    let tok = CancellationToken::new();
    let mut noop = |_: &[u8], _: &mut [u8], _: bool, _: &CancellationToken| Ok(0);

    // Valid call_indirect to f0 via table[0]
    assert_eq!(vm.invoke("run", &[0, 5], 1_000, &tok, &mut noop)?, Some(15));
    assert_eq!(vm.invoke("run", &[0, 0], 1_000, &tok, &mut noop)?, Some(20));

    // Signature mismatch trap: table[1] has type 1 ((i32, i32) -> i32), not type 0
    let mismatch = vm
        .invoke("run", &[1, 5], 1_000, &tok, &mut noop)
        .unwrap_err();
    assert!(mismatch.to_string().contains("签名不匹配"), "{mismatch}");

    // Uninitialized slot trap: table[2] is None
    let uninit = vm
        .invoke("run", &[2, 5], 1_000, &tok, &mut noop)
        .unwrap_err();
    assert!(uninit.to_string().contains("未初始化"), "{uninit}");

    // Out-of-bounds table index trap: table[9] >= 4
    let oob = vm
        .invoke("run", &[9, 5], 1_000, &tok, &mut noop)
        .unwrap_err();
    assert!(oob.to_string().contains("越界"), "{oob}");
    Ok(())
}

#[test]
fn bulk_memory_copy_fill_init_and_adversarial_fuel_exhaustion() -> Result<()> {
    assert_eq!(MAX_MEMORY_PAGES, 256);
    // func 0: fill(dst, val, len) + copy(dst2, src, len) + init from passive seg 0 + data.drop 0
    let types = [
        2u8, 0x60, 1, I32, 1, I32, // 0: bulk_ops(len) -> i32
        0x60, 0, 1, I32, // 1: passive_init() -> i32
    ];
    let funcs = [2u8, 0, 1];
    let mem = [1u8, 1, 2, 0x80, 0x02]; // min 2 pages (128 KiB), max 256 pages (LEB128 0x80, 0x02)
    let exports = [
        2u8, 4, b'b', b'u', b'l', b'k', 0, 0, 4, b'i', b'n', b'i', b't', 0, 1,
    ];
    let data_cnt = [1u8]; // DataCountSection (12): 1 segment
    let data = [1u8, 1, 4, b'W', b'A', b'S', b'M']; // passive segment 0 = "WASM"

    // f0(len): memory.fill(dst=0, val=0x5a, n=len); memory.copy(dst=64, src=0, n=len); load8_u(64)
    let f0 = [
        0u8, 0x41, 0, 0x41, 0xda, 0x00, 0x20, 0, 0xfc, 11,
        0, // memory.fill (val=0x5a=90 -> sLEB128 [0xda, 0x00])
        0x41, 0xc0, 0x00, 0x41, 0, 0x20, 0, 0xfc, 10, 0,
        0, // memory.copy (64 in sLEB128 is [0xc0, 0x00])
        0x41, 0xc0, 0x00, 0x2d, 0, 0, // i32.load8_u(64)
        0x0b,
    ];
    // f1(): memory.init(seg=0, dst=128, src=0, len=4); data.drop(0); i32.load(128)
    let f1 = [
        0u8, 0x41, 0x80, 0x01, 0x41, 0, 0x41, 4, 0xfc, 8, 0, 0, // memory.init seg 0
        0xfc, 9, 0, // data.drop 0
        0x41, 0x80, 0x01, 0x28, 0, 0, // i32.load(128)
        0x0b,
    ];
    let mut code = vec![2u8];
    for body in [&f0[..], &f1[..]] {
        module::push_uleb(body.len(), &mut code);
        code.extend_from_slice(body);
    }
    let mut vm = inst(&[
        (1, &types),
        (3, &funcs),
        (5, &mem),
        (7, &exports),
        (12, &data_cnt),
        (10, &code),
        (11, &data),
    ])?;
    let tok = CancellationToken::new();
    let mut noop = |_: &[u8], _: &mut [u8], _: bool, _: &CancellationToken| Ok(0);

    assert_eq!(vm.invoke("bulk", &[32], 500, &tok, &mut noop)?, Some(0x5a));
    assert_eq!(vm.read_memory(64, 4)?, &[0x5a; 4]);

    // Adversarial test: large memory.fill / memory.copy cannot bypass fuel metering!
    let exhausted = vm
        .invoke("bulk", &[65_536], 1_000, &tok, &mut noop)
        .unwrap_err();
    assert!(exhausted.to_string().contains("fuel"), "{exhausted}");

    // Passive segment init + data.drop (second non-zero init traps after drop)
    let magic = i32::from_le_bytes(*b"WASM");
    assert_eq!(
        vm.invoke("init", &[], 500, &tok, &mut noop)?,
        Some(i64::from(magic))
    );
    assert!(vm.invoke("init", &[], 500, &tok, &mut noop).is_err());
    Ok(())
}

#[test]
fn subword_loads_stores_and_i32_i64_float_conversions() -> Result<()> {
    // Exercises store16, load16_s, load16_u, store32(i64), load32_s(i64), i32/i64 div/rot/popcnt/wrap/extend/sign-ext + f64 add
    let ops: &[u8] = &[
        1, 1, F64, // 1 local f64 (local 0)
        // store16 -32639 (0x8081; sLEB128: 0x81, 0x81, 0x7e) at addr 0
        0x41, 0, 0x41, 0x81, 0x81, 0x7e, 0x3b, 0, 0,
        // load16_s(0) -> -32639; extend_i32_s -> i64; store32 at addr 8
        0x41, 8, 0x41, 0, 0x2e, 0, 0, 0xac, 0x3e, 0, 0,
        // load32_s(8) as i64 -> -32639; wrap_i64 -> i32; sign-extend8_s (0x81 -> -127)
        0x41, 8, 0x34, 0, 0, 0xa7, 0xc0,
        // f64 const 1.5 + 2.5 = 4.0 -> i32.trunc_f64_s (0xaa) -> 4; add to -127 => -123
        0x44, 0, 0, 0, 0, 0, 0, 0xf8, 0x3f, 0x44, 0, 0, 0, 0, 0, 0, 0x04, 0x40, 0xa0, 0xaa, 0x6a,
        // popcnt(7) = 3; div_s(-120, 3) => (-123 + 3) / 3 = -40
        0x41, 7, 0x69, 0x6a, 0x41, 3, 0x6d, 0x0b,
    ];
    let mut c = vec![1u8];
    module::push_uleb(ops.len(), &mut c);
    c.extend_from_slice(ops);
    let secs: &[(u8, &[u8])] = &[
        (1, &b"\x01\x60\x00\x01\x7f"[..]),
        (3, b"\x01\x00"),
        (5, b"\x01\x00\x01"),
        (7, &b"\x01\x04calc\x00\x00"[..]),
        (10, &c),
    ];
    let mut vm = inst(secs)?;
    let tok = CancellationToken::new();
    let mut noop = |_: &[u8], _: &mut [u8], _: bool, _: &CancellationToken| Ok(0);
    assert_eq!(vm.invoke("calc", &[], 1_000, &tok, &mut noop)?, Some(-40));
    Ok(())
}
