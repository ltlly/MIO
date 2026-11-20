use anyhow::{Context, Result, bail, ensure};
use tokio_util::sync::CancellationToken;

use super::{Val, WasmInstance, addr, float, pop, pop_i32, pop_i64, pop_u};

impl WasmInstance {
    pub(super) fn memory_op(&mut self, op: u8, offset: usize, stack: &mut Vec<Val>) -> Result<()> {
        match op {
            0x28 => {
                let mut b = [0u8; 4];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 4)?);
                stack.push(Val::I32(i32::from_le_bytes(b)));
            }
            0x29 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 8)?);
                stack.push(Val::I64(i64::from_le_bytes(b)));
            }
            0x2a => {
                let mut b = [0u8; 4];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 4)?);
                stack.push(Val::F32(u32::from_le_bytes(b)));
            }
            0x2b => {
                let mut b = [0u8; 8];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 8)?);
                stack.push(Val::F64(u64::from_le_bytes(b)));
            }
            0x2c => {
                let b = self.read_memory(addr(stack, offset)?, 1)?[0] as i8;
                stack.push(Val::I32(i32::from(b)));
            }
            0x2d => {
                let b = self.read_memory(addr(stack, offset)?, 1)?[0];
                stack.push(Val::I32(i32::from(b)));
            }
            0x2e => {
                let mut b = [0u8; 2];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 2)?);
                stack.push(Val::I32(i32::from(i16::from_le_bytes(b))));
            }
            0x2f => {
                let mut b = [0u8; 2];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 2)?);
                stack.push(Val::I32(i32::from(u16::from_le_bytes(b))));
            }
            0x30 => {
                let b = self.read_memory(addr(stack, offset)?, 1)?[0] as i8;
                stack.push(Val::I64(i64::from(b)));
            }
            0x31 => {
                let b = self.read_memory(addr(stack, offset)?, 1)?[0];
                stack.push(Val::I64(i64::from(b)));
            }
            0x32 => {
                let mut b = [0u8; 2];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 2)?);
                stack.push(Val::I64(i64::from(i16::from_le_bytes(b))));
            }
            0x33 => {
                let mut b = [0u8; 2];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 2)?);
                stack.push(Val::I64(i64::from(u16::from_le_bytes(b))));
            }
            0x34 => {
                let mut b = [0u8; 4];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 4)?);
                stack.push(Val::I64(i64::from(i32::from_le_bytes(b))));
            }
            0x35 => {
                let mut b = [0u8; 4];
                b.copy_from_slice(self.read_memory(addr(stack, offset)?, 4)?);
                stack.push(Val::I64(i64::from(u32::from_le_bytes(b))));
            }
            0x36 => {
                let val = pop_i32(stack)?;
                let at = addr(stack, offset)?;
                self.write_memory(at, &val.to_le_bytes())?;
            }
            0x37 => {
                let val = pop_i64(stack)?;
                let at = addr(stack, offset)?;
                self.write_memory(at, &val.to_le_bytes())?;
            }
            0x38 => {
                let v = match pop(stack)? {
                    Val::F32(bits) => bits,
                    _ => bail!("需要 f32 值"),
                };
                let at = addr(stack, offset)?;
                self.write_memory(at, &v.to_le_bytes())?;
            }
            0x39 => {
                let v = match pop(stack)? {
                    Val::F64(bits) => bits,
                    _ => bail!("需要 f64 值"),
                };
                let at = addr(stack, offset)?;
                self.write_memory(at, &v.to_le_bytes())?;
            }
            0x3a => {
                let val = pop_i32(stack)? as u8;
                let at = addr(stack, offset)?;
                self.write_memory(at, &[val])?;
            }
            0x3b => {
                let val = pop_i32(stack)? as u16;
                let at = addr(stack, offset)?;
                self.write_memory(at, &val.to_le_bytes())?;
            }
            0x3c => {
                let val = pop_i64(stack)? as u8;
                let at = addr(stack, offset)?;
                self.write_memory(at, &[val])?;
            }
            0x3d => {
                let val = pop_i64(stack)? as u16;
                let at = addr(stack, offset)?;
                self.write_memory(at, &val.to_le_bytes())?;
            }
            0x3e => {
                let val = pop_i64(stack)? as u32;
                let at = addr(stack, offset)?;
                self.write_memory(at, &val.to_le_bytes())?;
            }
            _ => bail!("未知内存指令 0x{op:02x}"),
        }
        Ok(())
    }

    pub(super) fn bulk_memory_op(
        &mut self,
        sub: i64,
        idx: usize,
        stack: &mut Vec<Val>,
        fuel: &mut u64,
        cancel: &CancellationToken,
    ) -> Result<()> {
        match sub {
            0..=7 => float::sat_trunc(sub as u8, stack),
            8 => {
                let len = pop_u(stack)?;
                let src = pop_u(stack)?;
                let dst = pop_u(stack)?;
                charge_bulk_fuel(len, fuel, cancel)?;
                let seg = self
                    .passive_data
                    .get(idx)
                    .context("memory.init 数据段索引越界")?;
                let bytes = match seg {
                    Some(bytes) => bytes.as_slice(),
                    None => &[][..],
                };
                let s_end = src.checked_add(len).context("memory.init 源溢出")?;
                let slice = bytes
                    .get(src..s_end)
                    .context("memory.init 源越界或数据段已释放")?
                    .to_vec();
                self.write_memory(dst, &slice)
            }
            9 => {
                let slot = self
                    .passive_data
                    .get_mut(idx)
                    .context("data.drop 数据段索引越界")?;
                *slot = None;
                Ok(())
            }
            10 => {
                let len = pop_u(stack)?;
                let src = pop_u(stack)?;
                let dst = pop_u(stack)?;
                charge_bulk_fuel(len, fuel, cancel)?;
                let s_end = src.checked_add(len).context("memory.copy 源溢出")?;
                let d_end = dst.checked_add(len).context("memory.copy 目的溢出")?;
                ensure!(
                    s_end <= self.mem.len() && d_end <= self.mem.len(),
                    "memory.copy 越界"
                );
                if len > 0 {
                    self.mem.copy_within(src..s_end, dst);
                }
                Ok(())
            }
            11 => {
                let len = pop_u(stack)?;
                let val = pop_i32(stack)? as u8;
                let dst = pop_u(stack)?;
                charge_bulk_fuel(len, fuel, cancel)?;
                let d_end = dst.checked_add(len).context("memory.fill 溢出")?;
                let slice = self.mem.get_mut(dst..d_end).context("memory.fill 越界")?;
                slice.fill(val);
                Ok(())
            }
            _ => bail!("未知 0xfc 指令 {sub}"),
        }
    }
}

fn charge_bulk_fuel(len: usize, fuel: &mut u64, cancel: &CancellationToken) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "已取消");
    let cost = u64::try_from(len).unwrap_or(u64::MAX);
    ensure!(*fuel >= cost, "fuel 耗尽");
    *fuel -= cost;
    Ok(())
}
