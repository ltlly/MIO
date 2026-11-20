use super::{
    I32, I64,
    module::{push_sleb, push_uleb},
};

pub(super) const M_CUR: usize = 0x0010;
pub(super) const M_TAG: usize = 0x0014;
pub(super) const M_ED_REV: usize = 0x0018;
pub(super) const M_MSG_REV: usize = 0x0020;
pub(super) const M_CTX_KIND: usize = 0x0028;
pub(super) const M_READ_ONLY: usize = 0x002c;
pub(super) const M_RES_LEN: usize = 0x0030;
pub(super) const M_RES_ID: i32 = 0x0040;
pub(super) const HOST_BUF: i32 = 0x0800;
pub(super) const HEAP_BASE: i32 = 0x4000;

pub(super) fn push_sleb64(mut v: i64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
        out.push(if done { b } else { b | 0x80 });
        if done {
            break;
        }
    }
}

pub(super) struct Asm(pub(super) Vec<u8>);

impl Asm {
    pub(super) fn new(locals: &[(usize, u8)]) -> Self {
        let mut v = Vec::new();
        push_uleb(locals.len(), &mut v);
        for &(n, t) in locals {
            push_uleb(n, &mut v);
            v.push(t);
        }
        Self(v)
    }
    pub(super) fn op(&mut self, b: u8) -> &mut Self {
        self.0.push(b);
        self
    }
    pub(super) fn raw(&mut self, bs: &[u8]) -> &mut Self {
        self.0.extend_from_slice(bs);
        self
    }
    pub(super) fn c32(&mut self, v: i32) -> &mut Self {
        self.0.push(0x41);
        push_sleb(v, &mut self.0);
        self
    }
    pub(super) fn c64(&mut self, v: i64) -> &mut Self {
        self.0.push(0x42);
        push_sleb64(v, &mut self.0);
        self
    }
    pub(super) fn lg(&mut self, idx: usize) -> &mut Self {
        self.0.push(0x20);
        push_uleb(idx, &mut self.0);
        self
    }
    pub(super) fn ls(&mut self, idx: usize) -> &mut Self {
        self.0.push(0x21);
        push_uleb(idx, &mut self.0);
        self
    }
    pub(super) fn call(&mut self, idx: usize) -> &mut Self {
        self.0.push(0x10);
        push_uleb(idx, &mut self.0);
        self
    }
    pub(super) fn ld32(&mut self, off: usize) -> &mut Self {
        self.0.extend_from_slice(&[0x28, 0]);
        push_uleb(off, &mut self.0);
        self
    }
    pub(super) fn ld64(&mut self, off: usize) -> &mut Self {
        self.0.extend_from_slice(&[0x29, 0]);
        push_uleb(off, &mut self.0);
        self
    }
    pub(super) fn ld8u(&mut self, off: usize) -> &mut Self {
        self.0.extend_from_slice(&[0x2d, 0]);
        push_uleb(off, &mut self.0);
        self
    }
    pub(super) fn st32(&mut self, off: usize) -> &mut Self {
        self.0.extend_from_slice(&[0x36, 0]);
        push_uleb(off, &mut self.0);
        self
    }
    pub(super) fn st64(&mut self, off: usize) -> &mut Self {
        self.0.extend_from_slice(&[0x37, 0]);
        push_uleb(off, &mut self.0);
        self
    }
    pub(super) fn st8(&mut self, off: usize) -> &mut Self {
        self.0.extend_from_slice(&[0x3a, 0]);
        push_uleb(off, &mut self.0);
        self
    }
    pub(super) fn st8c(&mut self, ptr_loc: usize, off: usize, val: i32) -> &mut Self {
        self.lg(ptr_loc).c32(val).st8(off)
    }
    pub(super) fn advance(&mut self, loc: usize, delta: i32) -> &mut Self {
        self.lg(loc).c32(delta).op(0x6a).ls(loc)
    }
    pub(super) fn ret_const(&mut self, c: (i32, i32)) -> &mut Self {
        self.c32(c.0).c32(c.1).call(7).op(0x0f)
    }
    pub(super) fn ret_locals(&mut self, p_loc: usize, l_loc: usize) -> &mut Self {
        self.lg(p_loc).lg(l_loc).call(7).op(0x0f)
    }
    pub(super) fn zero32(&mut self, off: usize) -> &mut Self {
        self.c32(0).c32(0).st32(off)
    }
    pub(super) fn slice(&mut self, tmp64: usize, p_loc: usize, l_loc: usize) -> &mut Self {
        self.ls(tmp64)
            .lg(tmp64)
            .c64(32)
            .op(0x88)
            .op(0xa7)
            .ls(p_loc)
            .lg(tmp64)
            .op(0xa7)
            .ls(l_loc)
    }
    pub(super) fn finish(mut self) -> Vec<u8> {
        self.0.push(0x0b);
        self.0
    }
}

/// Func 1: copy_bytes(src=0, dst=1, len=2)
pub(super) fn fn_copy_bytes() -> Vec<u8> {
    let mut a = Asm::new(&[(1, I32)]);
    a.raw(&[0x02, 0x40, 0x03, 0x40])
        .lg(3)
        .c32(8)
        .op(0x6a)
        .lg(2)
        .op(0x4b)
        .raw(&[0x0d, 1])
        .lg(1)
        .lg(3)
        .op(0x6a)
        .lg(0)
        .lg(3)
        .op(0x6a)
        .ld64(0)
        .st64(0)
        .advance(3, 8)
        .raw(&[0x0c, 0, 0x0b, 0x0b])
        .raw(&[0x02, 0x40, 0x03, 0x40])
        .lg(3)
        .lg(2)
        .op(0x4f)
        .raw(&[0x0d, 1])
        .lg(1)
        .lg(3)
        .op(0x6a)
        .lg(0)
        .lg(3)
        .op(0x6a)
        .ld8u(0)
        .st8(0)
        .advance(3, 1)
        .raw(&[0x0c, 0, 0x0b, 0x0b]);
    a.finish()
}

/// Func 2: mem_eq(a=0, al=1, b=2, bl=3) -> i32
pub(super) fn fn_mem_eq() -> Vec<u8> {
    let mut a = Asm::new(&[(1, I32)]);
    a.lg(1)
        .lg(3)
        .op(0x47)
        .raw(&[0x04, 0x40])
        .c32(0)
        .op(0x0f)
        .op(0x0b)
        .raw(&[0x02, 0x40, 0x03, 0x40])
        .lg(4)
        .lg(1)
        .op(0x4f)
        .raw(&[0x0d, 1])
        .lg(0)
        .lg(4)
        .op(0x6a)
        .ld8u(0)
        .lg(2)
        .lg(4)
        .op(0x6a)
        .ld8u(0)
        .op(0x47)
        .raw(&[0x04, 0x40])
        .c32(0)
        .op(0x0f)
        .op(0x0b)
        .advance(4, 1)
        .raw(&[0x0c, 0, 0x0b, 0x0b])
        .c32(1);
    a.finish()
}

/// Func 3: mio_alloc(len=0) -> i32
pub(super) fn fn_mio_alloc() -> Vec<u8> {
    let mut a = Asm::new(&[(3, I32)]);
    a.raw(&[0x23, 0])
        .ls(1)
        .lg(1)
        .lg(0)
        .c32(7)
        .op(0x6c)
        .op(0x6a)
        .c32(0x20000)
        .op(0x6a)
        .c32(16)
        .op(0x76)
        .ls(2)
        .raw(&[0x3f, 0])
        .ls(3)
        .lg(2)
        .lg(3)
        .op(0x4b)
        .raw(&[0x04, 0x40])
        .lg(2)
        .lg(3)
        .op(0x6b)
        .raw(&[0x40, 0, 0x1a, 0x0b])
        .lg(1)
        .lg(0)
        .op(0x6a)
        .c32(15)
        .op(0x6a)
        .c32(-16)
        .op(0x71)
        .raw(&[0x24, 0])
        .lg(1);
    a.finish()
}

/// Func 4: read_uleb64() -> i64 (reads from M_CUR and advances M_CUR)
pub(super) fn fn_read_uleb64() -> Vec<u8> {
    let mut a = Asm::new(&[(3, I32), (1, I64)]);
    a.c32(0)
        .ld32(M_CUR)
        .ls(0)
        .raw(&[0x03, 0x40])
        .lg(0)
        .ld8u(0)
        .ls(2)
        .advance(0, 1)
        .lg(1)
        .c32(64)
        .op(0x49)
        .raw(&[0x04, 0x40])
        .lg(3)
        .lg(2)
        .c32(0x7f)
        .op(0x71)
        .op(0xad)
        .lg(1)
        .op(0xad)
        .op(0x86)
        .op(0x84)
        .ls(3)
        .op(0x0b)
        .advance(1, 7)
        .lg(2)
        .c32(0x80)
        .op(0x4f)
        .raw(&[0x0d, 0, 0x0b])
        .c32(0)
        .lg(0)
        .st32(M_CUR)
        .lg(3);
    a.finish()
}

/// Func 5: write_uleb64(dst=0: i32, val=1: i64) -> i32
pub(super) fn fn_write_uleb64() -> Vec<u8> {
    let mut a = Asm::new(&[(1, I32)]);
    a.raw(&[0x03, 0x40])
        .lg(1)
        .op(0xa7)
        .c32(0x7f)
        .op(0x71)
        .ls(2)
        .lg(1)
        .c64(7)
        .op(0x88)
        .ls(1)
        .lg(1)
        .c64(0)
        .op(0x52)
        .raw(&[0x04, 0x40])
        .lg(2)
        .c32(0x80)
        .op(0x72)
        .ls(2)
        .op(0x0b)
        .lg(0)
        .lg(2)
        .st8(0)
        .advance(0, 1)
        .lg(1)
        .c64(0)
        .op(0x52)
        .raw(&[0x0d, 0, 0x0b])
        .lg(0);
    a.finish()
}

/// Func 6: uleb64_len(val=0: i64) -> i32
pub(super) fn fn_uleb64_len() -> Vec<u8> {
    let mut a = Asm::new(&[(1, I32)]);
    a.raw(&[0x03, 0x40])
        .lg(0)
        .c64(7)
        .op(0x88)
        .ls(0)
        .advance(1, 1)
        .lg(0)
        .c64(0)
        .op(0x52)
        .raw(&[0x0d, 0, 0x0b])
        .lg(1);
    a.finish()
}

/// Func 7: pack_ret(ptr=0: i32, len=1: i32) -> i64 (resets heap global to HEAP_BASE)
pub(super) fn fn_pack_ret() -> Vec<u8> {
    let mut a = Asm::new(&[]);
    a.c32(HEAP_BASE)
        .raw(&[0x24, 0])
        .lg(0)
        .op(0xad)
        .c64(32)
        .op(0x86)
        .lg(1)
        .op(0xad)
        .op(0x84);
    a.finish()
}
