use anyhow::{Context, Result, bail, ensure};
use tokio_util::sync::CancellationToken;

use super::{
    DataSeg, Frame, MAX_MEMORY_PAGES, PAGE_BYTES, Val, WASM_HOST_CALL_FUEL, WasmHostFn,
    WasmInstance, WasmModule,
};

mod float;
mod memory;
mod numeric;

impl WasmInstance {
    pub fn new(m: WasmModule) -> Result<Self> {
        ensure!(m.memory.is_some() || m.data.is_empty(), "无内存");
        let mem = vec![0; m.memory.map_or(0, |(n, _)| n as usize * PAGE_BYTES)];
        let table = vec![None; m.table.map_or(0, |(n, _)| n as usize)];
        let g = m.globals.iter().map(|x| x.2).collect();
        let mut passive_data = Vec::with_capacity(m.data.len());
        for seg in &m.data {
            passive_data.push(match seg {
                DataSeg::Passive(bytes) => Some(bytes.clone()),
                DataSeg::Active(_, _) => None,
            });
        }
        let mut inst = Self {
            m,
            mem,
            table,
            g,
            passive_data,
        };
        for seg in inst.m.data.clone() {
            if let DataSeg::Active(off, bytes) = seg {
                inst.write_memory(off as usize, &bytes)?;
            }
        }
        for (off, funcs) in &inst.m.elements {
            let at = *off as usize;
            let end = at.checked_add(funcs.len()).context("元素段初始化溢出")?;
            let slots = inst.table.get_mut(at..end).context("元素段初始化越界")?;
            for (slot, idx) in slots.iter_mut().zip(funcs) {
                *slot = Some(*idx);
            }
        }
        Ok(inst)
    }

    pub fn ensure_memory(&mut self, needed_bytes: usize) -> Result<()> {
        if needed_bytes <= self.mem.len() {
            return Ok(());
        }
        let (_, max_pages) = self.m.memory.context("Wasm 模块未声明线性内存")?;
        let pages = needed_bytes.div_ceil(PAGE_BYTES);
        ensure!(
            pages <= (max_pages.min(MAX_MEMORY_PAGES) as usize),
            "Wasm 线性内存超出模块上限"
        );
        self.mem.resize(pages * PAGE_BYTES, 0);
        Ok(())
    }

    pub fn read_memory(&self, offset: usize, len: usize) -> Result<&[u8]> {
        let end = offset.checked_add(len).context("内存读取溢出")?;
        self.mem.get(offset..end).context("内存读取越界")
    }

    pub fn write_memory(&mut self, offset: usize, data: &[u8]) -> Result<()> {
        let end = offset.checked_add(data.len()).context("内存写入溢出")?;
        let dst = self.mem.get_mut(offset..end).context("内存写入越界")?;
        dst.copy_from_slice(data);
        Ok(())
    }

    fn frame(&self, def: usize, stack: &mut Vec<Val>) -> Result<Frame> {
        let ty = &self.m.types[*self.m.funcs.get(def).context("函数越界")? as usize];
        ensure!(stack.len() >= ty.0.len(), "调用参数不足");
        let (base, (extra, ops)) = (stack.len() - ty.0.len(), &self.m.code[def]);
        let mut locals = Vec::with_capacity(ty.0.len() + extra.len());
        for (val, &vt) in stack.drain(base..).zip(&ty.0) {
            ensure!(val.vtype() == vt, "调用参数类型不匹配");
            locals.push(val);
        }
        locals.extend(extra.iter().copied().map(Val::zero));
        let ret = ty.1.len();
        Ok((def, 1, locals, vec![(ops.len(), ret, ret, base, false)]))
    }

    fn function_type(&self, idx: usize) -> Result<&(Vec<u8>, Vec<u8>)> {
        if let Some((_, ti)) = self.m.host_import
            && idx == 0
        {
            return self.m.types.get(ti as usize).context("宿主函数类型越界");
        }
        let imp = usize::from(self.m.host_import.is_some());
        let def = idx.checked_sub(imp).context("函数索引越界")?;
        let tidx = *self.m.funcs.get(def).context("函数索引越界")? as usize;
        self.m.types.get(tidx).context("函数类型越界")
    }

    pub fn invoke(
        &mut self,
        export: &str,
        args: &[i64],
        mut fuel: u64,
        cancel: &CancellationToken,
        host: WasmHostFn<'_>,
    ) -> Result<Option<i64>> {
        let imp = usize::from(self.m.host_import.is_some());
        let (kind, idx) = self
            .m
            .exports
            .iter()
            .find(|e| e.0 == export)
            .map(|e| (e.1, e.2))
            .context("导出不存在")?;
        ensure!(kind == 0 && (idx as usize) >= imp, "非法导出");
        let def = idx as usize - imp;
        let ty = &self.m.types[self.m.funcs[def] as usize];
        ensure!(args.len() == ty.0.len(), "参数错误");
        let mut stack: Vec<Val> = args
            .iter()
            .copied()
            .zip(&ty.0)
            .map(|(raw, &vt)| Val::from_arg(vt, raw))
            .collect();
        let mut steps = 0u64;
        let mut frames = vec![self.frame(def, &mut stack)?];
        while let Some(frame) = frames.last_mut() {
            ensure!(steps & 511 != 0 || !cancel.is_cancelled(), "已取消");
            ensure!(fuel > 0 && stack.len() < 2048, "fuel 耗尽或栈溢出");
            fuel -= 1;
            steps += 1;
            let &(op, imm, x, y) = self.m.code[frame.0].1.get(frame.1).context("PC 越界")?;
            frame.1 += 1;
            let arity = imm as usize;
            match op {
                0x00 => bail!("unreachable trap"),
                0x01 => {}
                0x02 => frame.3.push((y, arity, arity, stack.len(), false)),
                0x03 => frame.3.push((frame.1, 0, arity, stack.len(), true)),
                0x04 => {
                    let take = pop_i32(&mut stack)? != 0;
                    if take || x < y {
                        frame.3.push((y, arity, arity, stack.len(), false));
                    }
                    frame.1 = if take { frame.1 } else { x };
                }
                0x05 | 0x0b => {
                    let label = frame.3.pop().context("控制栈空")?;
                    unwind(&mut stack, label.3, label.2)?;
                    frame.1 = if op == 0x05 { y } else { frame.1 };
                }
                0x0c..=0x0f => {
                    let depth = match op {
                        0x0f => frame.3.len() - 1,
                        0x0e => {
                            let index = pop_i32(&mut stack)? as u32 as usize;
                            frame.1 += y;
                            if index < y {
                                self.m.code[frame.0].1[imm as usize + index].2
                            } else {
                                x
                            }
                        }
                        _ => x,
                    };
                    if op == 0x0d && pop_i32(&mut stack)? == 0 {
                        continue;
                    }
                    let at = frame.3.len().checked_sub(depth + 1).context("br 越界")?;
                    let label = frame.3[at];
                    unwind(&mut stack, label.3, label.1)?;
                    frame.3.truncate(at + usize::from(label.4));
                    frame.1 = label.0;
                }
                0x10 | 0x11 => {
                    let fidx = if op == 0x11 {
                        let ti = pop_i32(&mut stack)? as u32 as usize;
                        let idx = self
                            .table
                            .get(ti)
                            .context("call_indirect 表索引越界")?
                            .context("call_indirect 未初始化的表项")?
                            as usize;
                        let required = self.m.types.get(x).context("call_indirect 类型越界")?;
                        ensure!(
                            self.function_type(idx)? == required,
                            "call_indirect 签名不匹配"
                        );
                        idx
                    } else {
                        x
                    };
                    if self.m.host_import.is_some() && fidx == 0 {
                        self.host_call(&mut stack, &mut fuel, cancel, host)?;
                    } else {
                        ensure!(frames.len() < 64, "调用栈超限");
                        let next =
                            self.frame(fidx.checked_sub(imp).context("越界")?, &mut stack)?;
                        frames.push(next);
                        continue;
                    }
                }
                0x1a => _ = pop(&mut stack)?,
                0x1b => {
                    let take_first = pop_i32(&mut stack)? != 0;
                    let right = pop(&mut stack)?;
                    let left = pop(&mut stack)?;
                    ensure!(right.vtype() == left.vtype(), "select 类型不匹配");
                    stack.push(if take_first { left } else { right });
                }
                0x20 => stack.push(*frame.2.get(x).context("local 越界")?),
                0x21 | 0x22 => {
                    let val = if op == 0x21 {
                        pop(&mut stack)?
                    } else {
                        *stack.last().context("下溢")?
                    };
                    let local = frame.2.get_mut(x).context("local 越界")?;
                    ensure!(val.vtype() == local.vtype(), "local 类型不匹配");
                    *local = val;
                }
                0x23 => stack.push(*self.g.get(x).context("global 越界")?),
                0x24 => {
                    ensure!(self.m.globals.get(x).is_some_and(|g| g.1), "不可变");
                    let val = pop(&mut stack)?;
                    ensure!(val.vtype() == self.g[x].vtype(), "global 类型不匹配");
                    self.g[x] = val;
                }
                0x28..=0x3e => self.memory_op(op, x, &mut stack)?,
                0x3f => {
                    ensure!(self.m.memory.is_some(), "无内存");
                    stack.push(Val::I32((self.mem.len() / PAGE_BYTES) as i32));
                }
                0x40 => {
                    let (_, max) = self.m.memory.context("无内存")?;
                    let cur = (self.mem.len() / PAGE_BYTES) as u32;
                    let delta = pop_i32(&mut stack)? as u32;
                    let ok = cur
                        .checked_add(delta)
                        .is_some_and(|n| n <= max.min(MAX_MEMORY_PAGES));
                    if ok {
                        self.mem.resize((cur + delta) as usize * PAGE_BYTES, 0);
                    }
                    stack.push(Val::I32(if ok { cur as i32 } else { -1 }));
                }
                0x41 => stack.push(Val::I32(imm as i32)),
                0x42 => stack.push(Val::I64(imm)),
                0x43 => stack.push(Val::F32(imm as u32)),
                0x44 => stack.push(Val::F64(imm as u64)),
                0x45..=0x5a | 0x67..=0x8a | 0xa7 | 0xac | 0xad | 0xc0..=0xc4 => {
                    numeric::integer(op, &mut stack)?;
                }
                0x5b..=0x66 | 0x8b..=0xa6 | 0xa8..=0xab | 0xae..=0xbf => {
                    float::float_op(op, &mut stack)?;
                }
                0xfc => self.bulk_memory_op(imm, x, &mut stack, &mut fuel, cancel)?,
                _ => bail!("非法指令或状态 0x{op:02x}"),
            }
            if frames.last().is_some_and(|top| top.3.is_empty()) {
                frames.pop();
            }
        }
        Ok(stack.pop().map(Val::to_raw_i64))
    }

    fn host_call(
        &mut self,
        stack: &mut Vec<Val>,
        fuel: &mut u64,
        cancel: &CancellationToken,
        host: WasmHostFn<'_>,
    ) -> Result<()> {
        ensure!(!cancel.is_cancelled(), "已取消");
        ensure!(*fuel >= WASM_HOST_CALL_FUEL, "fuel 耗尽");
        *fuel -= WASM_HOST_CALL_FUEL;
        let cap = pop_u(stack)?;
        let envelope_abi = matches!(self.m.host_import, Some((3, _)));
        let out = if envelope_abi {
            None
        } else {
            Some(pop_u(stack)?)
        };
        let len = pop_u(stack)?;
        let ptr = pop_u(stack)?;
        let req = self.read_memory(ptr, len)?.to_vec();
        let out = out.unwrap_or(ptr);
        let end = out.checked_add(cap).context("内存指针溢出")?;
        let dst = self.mem.get_mut(out..end).context("内存缓冲越界")?;
        let n = host(&req, dst, envelope_abi, cancel)?;
        ensure!(n <= cap, "宿主写入越界");
        stack.push(Val::I32(i32::try_from(n)?));
        Ok(())
    }
}

pub(super) fn pop(stack: &mut Vec<Val>) -> Result<Val> {
    stack.pop().context("值栈下溢")
}
pub(super) fn pop_i32(stack: &mut Vec<Val>) -> Result<i32> {
    match pop(stack)? {
        Val::I32(v) => Ok(v),
        _ => bail!("需要 i32 值"),
    }
}
pub(super) fn pop_i64(stack: &mut Vec<Val>) -> Result<i64> {
    match pop(stack)? {
        Val::I64(v) => Ok(v),
        _ => bail!("需要 i64 值"),
    }
}
pub(super) fn pop_u(stack: &mut Vec<Val>) -> Result<usize> {
    Ok(pop_i32(stack)? as u32 as usize)
}
pub(super) fn addr(stack: &mut Vec<Val>, off: usize) -> Result<usize> {
    pop_u(stack)?.checked_add(off).context("地址溢出")
}
fn unwind(stack: &mut Vec<Val>, base: usize, arity: usize) -> Result<()> {
    ensure!(stack.len() >= base + arity, "值栈下溢");
    let ret = (arity == 1).then(|| stack.pop()).flatten();
    stack.truncate(base);
    stack.extend(ret);
    Ok(())
}
