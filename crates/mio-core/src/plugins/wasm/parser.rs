use anyhow::{Context, Result, bail, ensure};

use super::{DataSeg, F32, F64, FUNCREF, I32, I64, MAGIC, MAX_MEMORY_PAGES, Op, Val, WasmModule};

mod instr;
use instr::parse_func;

impl WasmModule {
    pub fn has_export(&self, export: &str) -> bool {
        self.exports
            .iter()
            .any(|(name, kind, _)| *kind == 0 && name == export)
    }

    pub fn is_envelope_abi(&self) -> bool {
        self.has_export("mio_alloc") && self.has_export("mio_dispatch")
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let valid_hdr = bytes.len() <= (4 << 20) && bytes.starts_with(&MAGIC);
        ensure!(valid_hdr, "Wasm 头或大小无效");
        let (mut r, mut last, mut m) = (&bytes[8..], 0u8, Self::default());
        while !r.is_empty() {
            let (id, n) = (byte(&mut r)?, u(&mut r)?);
            let mut s = take(&mut r, n)?;
            if id != 0 {
                let ord = match id {
                    12 => 9,
                    10 | 11 => id + 1,
                    _ => id,
                };
                ensure!(
                    ord > last && matches!(id, 1..=7 | 9..=12),
                    "非法或乱序 Wasm 段 {id}"
                );
                last = ord;
                if id == 12 {
                    let cnt = u(&mut s)?;
                    ensure!(cnt <= 4096, "DataCount 超限");
                    m.data_count = Some(cnt);
                } else {
                    for _ in 0..u(&mut s)? {
                        m.parse_item(id, &mut s)?;
                    }
                }
                ensure!(s.is_empty(), "段尾部多余字节");
            }
        }
        ensure!(m.code.len() == m.funcs.len(), "函数体数量不匹配");
        if let Some(cnt) = m.data_count {
            ensure!(m.data.len() == cnt, "DataCount 与 Data 段数量不匹配");
        }
        Ok(m)
    }

    fn parse_item(&mut self, id: u8, s: &mut &[u8]) -> Result<()> {
        match id {
            1 => {
                ensure!(byte(s)? == 0x60, "函数类型标签无效");
                let (p, r) = (vts(s)?, vts(s)?);
                ensure!(p.len() <= 64 && r.len() <= 1, "函数签名超限");
                self.types.push((p, r));
            }
            2 => {
                let (ns, f, k, ti) = (name(s)?, name(s)?, byte(s)?, u(s)?);
                ensure!(self.host_import.is_none() && k == 0, "非法导入 {ns}.{f}");
                let ty = self.types.get(ti).context("导入类型越界")?;
                if ns == "mio_host" && f == "host_call" {
                    ensure!(
                        ty.0 == [I32; 3] && ty.1 == [I32],
                        "mio_host.host_call 签名无效"
                    );
                    self.host_import = Some((3, ti as u32));
                } else if (ns == "mio" || ns == "env") && f == "mio_host_call" {
                    ensure!(ty.0 == [I32; 4] && ty.1 == [I32], "mio_host_call 签名无效");
                    self.host_import = Some((4, ti as u32));
                } else {
                    bail!("非法导入 {ns}.{f}");
                }
            }
            3 => {
                let i = u(s)?;
                ensure!(i < self.types.len(), "函数类型越界");
                self.funcs.push(i as u32);
            }
            4 => {
                let (et, flags, min) = (byte(s)?, byte(s)?, u(s)? as u32);
                let max = if flags == 1 {
                    u(s)? as u32
                } else {
                    min.max(4096)
                };
                let ok = self.table.is_none()
                    && et == FUNCREF
                    && flags <= 1
                    && min <= max
                    && max <= 65_536;
                ensure!(ok, "funcref 表声明非法");
                self.table = Some((min, max));
            }
            5 => {
                let (flags, min) = (byte(s)?, u(s)? as u32);
                let max = if flags == 1 {
                    (u(s)? as u32).min(MAX_MEMORY_PAGES)
                } else {
                    MAX_MEMORY_PAGES
                };
                let ok = self.memory.is_none() && flags <= 1 && min <= max;
                ensure!(ok, "线性内存声明非法");
                self.memory = Some((min, max));
            }
            6 => {
                let (vt, mu) = (vt(s)?, byte(s)?);
                ensure!(mu <= 1, "可变标志无效");
                let init = const_expr(s, &self.globals, vt)?;
                self.globals.push((vt, mu == 1, init));
            }
            7 => {
                let (nm, kind, idx) = (name(s)?.to_owned(), byte(s)?, u(s)?);
                let funcs = usize::from(self.host_import.is_some()) + self.funcs.len();
                let ok = match kind {
                    0 => idx < funcs,
                    1 => idx == 0 && self.table.is_some(),
                    2 => idx == 0 && self.memory.is_some(),
                    _ => kind == 3 && idx < self.globals.len(),
                };
                ensure!(ok, "导出非法: {nm}");
                self.exports.push((nm, kind, idx as u32));
            }
            9 => {
                let mode = u(s)?;
                ensure!(
                    mode == 0 && self.table.is_some(),
                    "只支持 active 表 0 元素段"
                );
                let off = match const_expr(s, &self.globals, I32)? {
                    Val::I32(v) => u32::try_from(v).context("元素段偏移为负")?,
                    _ => bail!("元素段偏移类型无效"),
                };
                let total_funcs = usize::from(self.host_import.is_some()) + self.funcs.len();
                let count = u(s)?;
                ensure!(count <= 65_536, "元素段条目过多");
                let mut idxs = Vec::with_capacity(count);
                for _ in 0..count {
                    let fidx = u(s)?;
                    ensure!(fidx < total_funcs, "元素段函数索引越界");
                    idxs.push(fidx as u32);
                }
                self.elements.push((off, idxs));
            }
            10 => {
                let n = u(s)?;
                self.code
                    .push(parse_func(&mut take(s, n)?, self.types.len())?);
            }
            11 => {
                let mode = u(s)?;
                match mode {
                    0 => {
                        let off = match const_expr(s, &self.globals, I32)? {
                            Val::I32(v) => u32::try_from(v).context("数据段偏移为负")?,
                            _ => bail!("数据段偏移类型无效"),
                        };
                        let n = u(s)?;
                        self.data.push(DataSeg::Active(off, take(s, n)?.to_vec()));
                    }
                    1 => {
                        let n = u(s)?;
                        self.data.push(DataSeg::Passive(take(s, n)?.to_vec()));
                    }
                    2 => {
                        ensure!(u(s)? == 0, "只支持线性内存 0");
                        let off = match const_expr(s, &self.globals, I32)? {
                            Val::I32(v) => u32::try_from(v).context("数据段偏移为负")?,
                            _ => bail!("数据段偏移类型无效"),
                        };
                        let n = u(s)?;
                        self.data.push(DataSeg::Active(off, take(s, n)?.to_vec()));
                    }
                    _ => bail!("未知数据段模式 {mode}"),
                }
            }
            _ => bail!("未知 Wasm 段 {id}"),
        }
        Ok(())
    }
}

pub(super) fn byte(s: &mut &[u8]) -> Result<u8> {
    let (&b, rest) = s.split_first().context("Wasm 字节越界")?;
    *s = rest;
    Ok(b)
}
pub(super) fn leb(s: &mut &[u8], bits: u32, sig: bool) -> Result<i64> {
    let (mut v, mut sh) = (0i128, 0u32);
    let max_bytes = bits.div_ceil(7);
    for step in 0..max_bytes {
        let b = byte(s)?;
        v |= i128::from(b & 0x7f) << sh;
        sh += 7;
        if b & 0x80 == 0 {
            if sig && (b & 0x40) != 0 {
                v |= -1i128 << sh;
            } else if step + 1 == max_bytes {
                let rem = bits - (sh - 7);
                if sig {
                    let mask = 0x7fu8 << (rem - 1);
                    let sign_bits = b & mask;
                    ensure!(sign_bits == 0 || sign_bits == mask, "LEB128 越界");
                } else {
                    ensure!((b & (0x7fu8 << rem)) == 0, "LEB128 越界");
                }
            }
            let lim = 1i128 << (bits - u32::from(sig));
            ensure!((!sig || v >= -lim) && v < lim, "LEB128 越界");
            return Ok(v as i64);
        }
    }
    bail!("LEB128 过长")
}
pub(super) fn u(s: &mut &[u8]) -> Result<usize> {
    Ok(leb(s, 32, false)? as usize)
}
pub(super) fn take<'a>(s: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    ensure!(n <= s.len(), "切片越界");
    let (a, b) = s.split_at(n);
    *s = b;
    Ok(a)
}
fn name<'a>(s: &mut &'a [u8]) -> Result<&'a str> {
    let len = u(s)?;
    Ok(std::str::from_utf8(take(s, len)?)?)
}
pub(super) fn vt(s: &mut &[u8]) -> Result<u8> {
    let v = byte(s)?;
    ensure!(
        matches!(v, I32 | I64 | F32 | F64),
        "不支持的值类型 0x{v:02x}"
    );
    Ok(v)
}
fn vts(s: &mut &[u8]) -> Result<Vec<u8>> {
    (0..u(s)?).map(|_| vt(s)).collect()
}
fn const_expr(s: &mut &[u8], gs: &[(u8, bool, Val)], t: u8) -> Result<Val> {
    let v = match (byte(s)?, t) {
        (0x41, I32) => Val::I32(leb(s, 32, true)? as i32),
        (0x42, I64) => Val::I64(leb(s, 64, true)?),
        (0x43, F32) => {
            let mut b = [0u8; 4];
            b.copy_from_slice(take(s, 4)?);
            Val::F32(u32::from_le_bytes(b))
        }
        (0x44, F64) => {
            let mut b = [0u8; 8];
            b.copy_from_slice(take(s, 8)?);
            Val::F64(u64::from_le_bytes(b))
        }
        (0x23, _) => gs.get(u(s)?).filter(|g| g.0 == t && !g.1).context("g")?.2,
        _ => bail!("非法常量表达式"),
    };
    ensure!(byte(s)? == 0x0b, "常量表达式缺少 end");
    Ok(v)
}
