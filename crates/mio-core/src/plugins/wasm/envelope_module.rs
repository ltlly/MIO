use super::{
    I32, I64, MAGIC,
    envelope_consts::{CONST_BASE, Consts, build_consts},
    envelope_handlers::{fn_handle_commit, fn_handle_open},
    envelope_hooks::{fn_handle_passive, fn_handle_payload},
    envelope_ops::{
        Asm, HEAP_BASE, HOST_BUF, M_CTX_KIND, M_ED_REV, M_MSG_REV, M_READ_ONLY, M_RES_ID,
        M_RES_LEN, M_TAG, fn_copy_bytes, fn_mem_eq, fn_mio_alloc, fn_pack_ret, fn_read_uleb64,
        fn_uleb64_len, fn_write_uleb64,
    },
    envelope_wire::{
        fn_contains_ci, fn_copy_except_field, fn_emit_patch, fn_emit_wrapped_concat, fn_find_field,
    },
    module::{push_section, push_sleb, push_uleb},
};

/// Func 17: mio_dispatch(ptr=0: i32, len=1: i32) -> i64
fn fn_mio_dispatch(c: &Consts) -> Vec<u8> {
    // locals: 2=out, 3=tag, 4=bp, 5=bl, 6=sp, 7=sl | 8=env_rev, 9=slice, 10=tmp64
    let mut a = Asm::new(&[(6, I32), (3, I64)]);
    a.lg(0)
        .lg(1)
        .op(0x6a)
        .c32(64)
        .op(0x6a)
        .c32(-16)
        .op(0x71)
        .ls(2)
        .lg(0)
        .lg(1)
        .c32(8)
        .call(8)
        .ls(8)
        .lg(0)
        .lg(1)
        .c32(-1)
        .call(8)
        .slice(9, 4, 5)
        .c32(0)
        .ld32(M_TAG)
        .ls(3)
        // 20: Hello
        .lg(3)
        .c32(20)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .zero32(M_CTX_KIND)
        .zero32(M_READ_ONLY)
        .zero32(M_RES_LEN)
        .c32(0)
        .c64(1)
        .st64(M_ED_REV)
        .ret_const(c.hello_snap)
        .op(0x0b)
        // 34: Resync
        .lg(3)
        .c32(34)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .ret_const(c.ack)
        .op(0x0b)
        // 25: UiEvent
        .lg(3)
        .c32(25)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .lg(4)
        .lg(5)
        .c32(1)
        .call(8)
        .ls(10)
        .lg(4)
        .lg(5)
        .c32(2)
        .call(8)
        .op(0x1a)
        .c32(0)
        .ld32(M_TAG)
        .c32(2)
        .op(0x46)
        .lg(10)
        .c64(10)
        .op(0x51)
        .op(0x71)
        .raw(&[0x04, 0x40])
        .c32(c.pref_call.0)
        .c32(HOST_BUF)
        .c32(c.pref_call.1)
        .call(1)
        .c32(HOST_BUF)
        .c32(c.pref_call.1)
        .c32(512)
        .call(0)
        .op(0x1a)
        .lg(2)
        .lg(8)
        .c32(c.op_pref.0)
        .c32(c.op_pref.1)
        .call(11)
        .op(0x0f)
        .op(0x0b)
        .lg(4)
        .lg(5)
        .c32(3)
        .call(8)
        .slice(9, 6, 7)
        .lg(9)
        .c64(0)
        .op(0x52)
        .raw(&[0x04, 0x40])
        .lg(6)
        .lg(7)
        .c32(2)
        .call(8)
        .ls(10)
        .lg(10)
        .c32(0)
        .ld64(M_ED_REV)
        .op(0x56)
        .raw(&[0x04, 0x40])
        .c32(0)
        .lg(10)
        .st64(M_ED_REV)
        .op(0x0b)
        .op(0x0b)
        .ret_const(c.ack)
        .op(0x0b)
        // 26: OpenRequest / 45: OpenResponse
        .lg(3)
        .c32(26)
        .op(0x46)
        .lg(3)
        .c32(45)
        .op(0x46)
        .op(0x72)
        .raw(&[0x04, 0x40])
        .lg(2)
        .lg(8)
        .lg(4)
        .lg(5)
        .lg(3)
        .c32(45)
        .op(0x46)
        .c32(c.op_open.0)
        .c32(c.op_open.1)
        .c32(c.n30_hdr.0)
        .c32(c.n30_hdr.1)
        .c32(c.err_inv.0)
        .c32(c.err_inv.1)
        .call(13)
        .op(0x0f)
        .op(0x0b)
        // 27: CommitEditor / 46: CommitResponseEditor
        .lg(3)
        .c32(27)
        .op(0x46)
        .lg(3)
        .c32(46)
        .op(0x46)
        .op(0x72)
        .raw(&[0x04, 0x40])
        .lg(2)
        .lg(4)
        .lg(5)
        .lg(3)
        .c32(46)
        .op(0x46)
        .c32(1)
        .op(0x6a)
        .c32(c.err_inv.0)
        .c32(c.err_inv.1)
        .call(14)
        .op(0x0f)
        .op(0x0b)
        // 36: CloseRequest
        .lg(3)
        .c32(36)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .lg(4)
        .lg(5)
        .c32(1)
        .call(8)
        .slice(9, 6, 7)
        .lg(7)
        .c32(16)
        .op(0x47)
        .raw(&[0x04, 0x40])
        .ret_const(c.err_inv)
        .op(0x0b)
        .lg(6)
        .lg(7)
        .c32(M_RES_ID)
        .c32(0)
        .ld32(M_RES_LEN)
        .call(2)
        .raw(&[0x04, 0x40])
        .zero32(M_CTX_KIND)
        .zero32(M_READ_ONLY)
        .zero32(M_RES_LEN)
        .c32(0)
        .c64(0)
        .st64(M_MSG_REV)
        .op(0x0b)
        .ret_const(c.ack)
        .op(0x0b)
        // 37: HttpHookCall
        .lg(3)
        .c32(37)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .lg(4)
        .lg(5)
        .c32(4)
        .call(8)
        .op(0xa7)
        .raw(&[0x04, 0x40])
        .lg(4)
        .lg(5)
        .c32(7)
        .call(8)
        .slice(9, 6, 7)
        .lg(9)
        .op(0x50)
        .raw(&[0x04, 0x40])
        .ret_const(c.err_inv)
        .op(0x0b)
        .lg(2)
        .c32(306)
        .c32(0x1a)
        .lg(6)
        .lg(7)
        .c32(c.hdr_resp.0)
        .c32(c.hdr_resp.1)
        .call(12)
        .op(0x0f)
        .op(0x05)
        .lg(4)
        .lg(5)
        .c32(6)
        .call(8)
        .slice(9, 6, 7)
        .lg(9)
        .op(0x50)
        .raw(&[0x04, 0x40])
        .ret_const(c.err_inv)
        .op(0x0b)
        .lg(2)
        .c32(306)
        .c32(0x12)
        .lg(6)
        .lg(7)
        .c32(c.hdr_req.0)
        .c32(c.hdr_req.1)
        .call(12)
        .op(0x0f)
        .op(0x0b)
        .op(0x0b)
        // 39: WebSocketHookCall
        .lg(3)
        .c32(39)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .lg(4)
        .lg(5)
        .c32(6)
        .call(8)
        .slice(9, 6, 7)
        .lg(7)
        .c32(524_281)
        .op(0x4b)
        .raw(&[0x04, 0x40])
        .ret_const(c.err_inv)
        .op(0x0b)
        .lg(2)
        .c32(322)
        .c32(0x12)
        .c32(c.ws_pref.0)
        .c32(c.ws_pref.1)
        .lg(6)
        .lg(7)
        .call(12)
        .op(0x0f)
        .op(0x0b)
        // 41: SessionActionCall
        .lg(3)
        .c32(41)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .lg(4)
        .lg(5)
        .c32(2)
        .call(8)
        .slice(9, 6, 7)
        .lg(9)
        .op(0x50)
        .raw(&[0x04, 0x40])
        .ret_const(c.err_inv)
        .op(0x0b)
        .lg(2)
        .c32(338)
        .c32(0x12)
        .lg(6)
        .lg(7)
        .c32(c.hdr_sess.0)
        .c32(c.hdr_sess.1)
        .call(12)
        .op(0x0f)
        .op(0x0b)
        // 43: PayloadCall
        .lg(3)
        .c32(43)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .lg(2)
        .lg(4)
        .lg(5)
        .call(15)
        .op(0x0f)
        .op(0x0b)
        // 48: ContextMenuCall
        .lg(3)
        .c32(48)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .lg(4)
        .lg(5)
        .c32(6)
        .call(8)
        .op(0x50)
        .raw(&[0x04, 0x40])
        .ret_const(c.err_inv)
        .op(0x0b)
        .ret_const(c.ctx_res)
        .op(0x0b)
        // 50: PassiveCheckCall
        .lg(3)
        .c32(50)
        .op(0x46)
        .raw(&[0x04, 0x40])
        .lg(2)
        .lg(4)
        .lg(5)
        .call(16)
        .op(0x0f)
        .op(0x0b)
        // 31/32: Cancel / Shutdown
        .lg(3)
        .c32(31)
        .op(0x46)
        .lg(3)
        .c32(32)
        .op(0x46)
        .op(0x72)
        .raw(&[0x04, 0x40])
        .ret_const((0, 0))
        .op(0x0b)
        .ret_const(c.err_inv);
    a.finish()
}

pub(super) fn build() -> Vec<u8> {
    let (consts, blob) = build_consts();
    let mut wasm = MAGIC.to_vec();
    let sigs: &[(&[u8], &[u8])] = &[
        (&[I32, I32, I32], &[I32]),
        (&[I32, I32, I32], &[]),
        (&[I32, I32, I32, I32], &[I32]),
        (&[I32], &[I32]),
        (&[], &[I64]),
        (&[I32, I64], &[I32]),
        (&[I64], &[I32]),
        (&[I32, I32], &[I64]),
        (&[I32, I32, I32], &[I64]),
        (&[I32, I64, I32, I32], &[I64]),
        (&[I32, I32, I32, I32, I32, I32, I32], &[I64]),
        (
            &[I32, I64, I32, I32, I32, I32, I32, I32, I32, I32, I32],
            &[I64],
        ),
        (&[I32, I32, I32, I32, I32, I32], &[I64]),
    ];
    let mut types = Vec::new();
    push_uleb(sigs.len(), &mut types);
    for &(params, results) in sigs {
        types.push(0x60);
        push_uleb(params.len(), &mut types);
        types.extend_from_slice(params);
        push_uleb(results.len(), &mut types);
        types.extend_from_slice(results);
    }
    push_section(1, &types, &mut wasm);
    push_section(2, b"\x01\x08mio_host\x09host_call\x00\x00", &mut wasm);
    let func_types: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 2, 2, 9, 10, 11, 12, 8, 8, 7];
    let mut funcs = Vec::new();
    push_uleb(func_types.len(), &mut funcs);
    funcs.extend_from_slice(func_types);
    push_section(3, &funcs, &mut wasm);
    push_section(5, &[1, 1, 2, 64], &mut wasm);
    let mut globals = vec![1u8, I32, 1, 0x41];
    push_sleb(HEAP_BASE, &mut globals);
    globals.push(0x0b);
    push_section(6, &globals, &mut wasm);
    let mut exports = vec![3u8];
    exports.extend_from_slice(b"\x06memory\x02\x00");
    exports.extend_from_slice(b"\x09mio_alloc\x00\x03");
    exports.extend_from_slice(b"\x0cmio_dispatch\x00\x11");
    push_section(7, &exports, &mut wasm);

    let bodies = [
        fn_copy_bytes(),
        fn_mem_eq(),
        fn_mio_alloc(),
        fn_read_uleb64(),
        fn_write_uleb64(),
        fn_uleb64_len(),
        fn_pack_ret(),
        fn_find_field(),
        fn_contains_ci(),
        fn_copy_except_field(),
        fn_emit_patch(),
        fn_emit_wrapped_concat(),
        fn_handle_open(),
        fn_handle_commit(),
        fn_handle_payload(&consts),
        fn_handle_passive(&consts),
        fn_mio_dispatch(&consts),
    ];
    let mut code = Vec::new();
    push_uleb(bodies.len(), &mut code);
    for body in &bodies {
        push_uleb(body.len(), &mut code);
        code.extend_from_slice(body);
    }
    push_section(10, &code, &mut wasm);

    let ed_rev_init = 1u64.to_le_bytes();
    let segments: &[(i32, &[u8])] = &[(M_ED_REV as i32, &ed_rev_init), (CONST_BASE, &blob)];
    let mut data = Vec::new();
    push_uleb(segments.len(), &mut data);
    for &(off, bytes) in segments {
        data.extend_from_slice(&[0, 0x41]);
        push_sleb(off, &mut data);
        data.push(0x0b);
        push_uleb(bytes.len(), &mut data);
        data.extend_from_slice(bytes);
    }
    push_section(11, &data, &mut wasm);
    wasm
}
