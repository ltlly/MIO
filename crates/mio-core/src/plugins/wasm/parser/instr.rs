use anyhow::{Context, Result, bail, ensure};

use super::{F32, F64, I32, I64, Op, byte, leb, take, u, vt};

pub(super) fn parse_func(b: &mut &[u8], num_types: usize) -> Result<(Vec<u8>, Vec<Op>)> {
    let mut locals = Vec::new();
    for _ in 0..u(b)? {
        let (n, lvt) = (u(b)?, vt(b)?);
        ensure!(locals.len().saturating_add(n) <= 2048, "局部变量过多");
        locals.resize(locals.len() + n, lvt);
    }
    let (mut ops, mut ctl) = (vec![(2, 0, 0, 0)], vec![(2u8, 0usize, 0usize)]);
    while !ctl.is_empty() {
        let op = byte(b)?;
        match op {
            0x00 | 0x01 => ops.push((op, 0, 0, 0)),
            0x02..=0x04 => {
                let bt = byte(b)?;
                ensure!(
                    matches!(bt, 0x40 | I32 | I64 | F32 | F64) && ctl.len() < 64,
                    "块非法"
                );
                ctl.push((op, ops.len(), 0));
                ops.push((op, i64::from(bt != 0x40), 0, 0));
            }
            0x05 if ctl.last().is_some_and(|c| c.0 == 4) => {
                let top = ctl.last_mut().unwrap();
                *top = (0x05, top.1, ops.len());
                ops.push((0x05, 0, 0, 0));
            }
            0x0b => {
                let (kop, open, epc) = ctl.pop().context("多余 end")?;
                ensure!(kop != 0x04 || ops[open].1 == 0, "if 缺少 else");
                ops.push((0x0b, 0, 0, 0));
                let after = ops.len();
                ops[open].2 = if kop == 0x05 { epc + 1 } else { after };
                ops[open].3 = after;
                ops[if kop == 0x05 { epc } else { open }].3 = after;
            }
            0x0c | 0x0d | 0x10 | 0x20..=0x24 => ops.push((op, 0, u(b)?, 0)),
            0x0e => {
                let cnt = u(b)?;
                ensure!(cnt <= 4096, "br_table 过大");
                let mut targets = Vec::with_capacity(cnt + 1);
                for _ in 0..=cnt {
                    targets.push(u(b)?);
                }
                // Encode br_table as a sequence of helper pseudo-ops `(0xfe, target, idx, 0)` followed by `(0x0e, 0, default, cnt)`
                let def = targets.pop().unwrap();
                let start = ops.len();
                ops.push((0x0e, 0, def, cnt));
                for t in targets {
                    ops.push((0xfe, 0, t, 0));
                }
                ops[start].1 = (start + 1) as i64;
            }
            0x11 => {
                let (ty_idx, tbl_idx) = (u(b)?, u(b)?);
                ensure!(ty_idx < num_types && tbl_idx == 0, "call_indirect 索引非法");
                ops.push((op, 0, ty_idx, tbl_idx));
            }
            0x1c => {
                let n = u(b)?;
                ensure!(n == 1, "typed select 仅支持单值");
                let _ = vt(b)?;
                ops.push((0x1b, 0, 0, 0));
            }
            0x28..=0x3e => ops.push((op, 0, (u(b)?, u(b)?).1, 0)),
            0x3f | 0x40 => {
                ensure!(byte(b)? == 0, "内存索引必须为 0");
                ops.push((op, 0, 0, 0));
            }
            0x41 | 0x42 => ops.push((op, leb(b, if op == 0x41 { 32 } else { 64 }, true)?, 0, 0)),
            0x43 => {
                let mut raw = [0u8; 4];
                raw.copy_from_slice(take(b, 4)?);
                ops.push((op, i64::from(u32::from_le_bytes(raw)), 0, 0));
            }
            0x44 => {
                let mut raw = [0u8; 8];
                raw.copy_from_slice(take(b, 8)?);
                ops.push((op, u64::from_le_bytes(raw) as i64, 0, 0));
            }
            0x0f | 0x1a | 0x1b | 0x45..=0xc4 => ops.push((op, 0, 0, 0)),
            0xfc => {
                let sub = u(b)?;
                match sub {
                    0..=7 => ops.push((0xfc, sub as i64, 0, 0)),
                    8 => {
                        let seg = u(b)?;
                        ensure!(byte(b)? == 0, "memory.init 内存索引必须为 0");
                        ops.push((0xfc, 8, seg, 0));
                    }
                    9 => ops.push((0xfc, 9, u(b)?, 0)),
                    10 => {
                        ensure!(
                            byte(b)? == 0 && byte(b)? == 0,
                            "memory.copy 内存索引必须为 0"
                        );
                        ops.push((0xfc, 10, 0, 0));
                    }
                    11 => {
                        ensure!(byte(b)? == 0, "memory.fill 内存索引必须为 0");
                        ops.push((0xfc, 11, 0, 0));
                    }
                    _ => bail!("不支持 0xfc 子指令 {sub}"),
                }
            }
            _ => bail!("不支持指令 0x{op:02x}"),
        }
    }
    ensure!(b.is_empty(), "函数尾部多余字节");
    Ok((locals, ops))
}
