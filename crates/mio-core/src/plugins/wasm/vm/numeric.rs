use anyhow::{Result, bail, ensure};

use super::{Val, pop_i32, pop_i64};

pub(super) fn integer(op: u8, stack: &mut Vec<Val>) -> Result<()> {
    match op {
        0x45 => {
            let a = pop_i32(stack)?;
            stack.push(Val::I32(i32::from(a == 0)));
        }
        0x46..=0x4f => {
            let b = pop_i32(stack)?;
            let a = pop_i32(stack)?;
            let (ua, ub) = (a as u32, b as u32);
            let res = match op {
                0x46 => a == b,
                0x47 => a != b,
                0x48 => a < b,
                0x49 => ua < ub,
                0x4a => a > b,
                0x4b => ua > ub,
                0x4c => a <= b,
                0x4d => ua <= ub,
                0x4e => a >= b,
                _ => ua >= ub,
            };
            stack.push(Val::I32(i32::from(res)));
        }
        0x50 => {
            let a = pop_i64(stack)?;
            stack.push(Val::I32(i32::from(a == 0)));
        }
        0x51..=0x5a => {
            let b = pop_i64(stack)?;
            let a = pop_i64(stack)?;
            let (ua, ub) = (a as u64, b as u64);
            let res = match op {
                0x51 => a == b,
                0x52 => a != b,
                0x53 => a < b,
                0x54 => ua < ub,
                0x55 => a > b,
                0x56 => ua > ub,
                0x57 => a <= b,
                0x58 => ua <= ub,
                0x59 => a >= b,
                _ => ua >= ub,
            };
            stack.push(Val::I32(i32::from(res)));
        }
        0x67..=0x69 => {
            let a = pop_i32(stack)? as u32;
            let v = match op {
                0x67 => a.leading_zeros(),
                0x68 => a.trailing_zeros(),
                _ => a.count_ones(),
            };
            stack.push(Val::I32(v as i32));
        }
        0x6a..=0x78 => {
            let b = pop_i32(stack)?;
            let a = pop_i32(stack)?;
            let (ua, ub) = (a as u32, b as u32);
            let sh = ub & 31;
            let v = match op {
                0x6a => a.wrapping_add(b),
                0x6b => a.wrapping_sub(b),
                0x6c => a.wrapping_mul(b),
                0x6d => {
                    ensure!(b != 0, "i32 除零 trap");
                    ensure!(!(a == i32::MIN && b == -1), "i32 整数溢出 trap");
                    a / b
                }
                0x6e => {
                    ensure!(ub != 0, "i32 无符号除零 trap");
                    (ua / ub) as i32
                }
                0x6f => {
                    ensure!(b != 0, "i32 取余除零 trap");
                    if a == i32::MIN && b == -1 { 0 } else { a % b }
                }
                0x70 => {
                    ensure!(ub != 0, "i32 无符号取余除零 trap");
                    (ua % ub) as i32
                }
                0x71 => a & b,
                0x72 => a | b,
                0x73 => a ^ b,
                0x74 => a.wrapping_shl(sh),
                0x75 => a.wrapping_shr(sh),
                0x76 => ua.wrapping_shr(sh) as i32,
                0x77 => ua.rotate_left(sh) as i32,
                _ => ua.rotate_right(sh) as i32,
            };
            stack.push(Val::I32(v));
        }
        0x79..=0x7b => {
            let a = pop_i64(stack)? as u64;
            let v = match op {
                0x79 => a.leading_zeros(),
                0x7a => a.trailing_zeros(),
                _ => a.count_ones(),
            };
            stack.push(Val::I64(i64::from(v)));
        }
        0x7c..=0x8a => {
            let b = pop_i64(stack)?;
            let a = pop_i64(stack)?;
            let (ua, ub) = (a as u64, b as u64);
            let sh = (ub & 63) as u32;
            let v = match op {
                0x7c => a.wrapping_add(b),
                0x7d => a.wrapping_sub(b),
                0x7e => a.wrapping_mul(b),
                0x7f => {
                    ensure!(b != 0, "i64 除零 trap");
                    ensure!(!(a == i64::MIN && b == -1), "i64 整数溢出 trap");
                    a / b
                }
                0x80 => {
                    ensure!(ub != 0, "i64 无符号除零 trap");
                    (ua / ub) as i64
                }
                0x81 => {
                    ensure!(b != 0, "i64 取余除零 trap");
                    if a == i64::MIN && b == -1 { 0 } else { a % b }
                }
                0x82 => {
                    ensure!(ub != 0, "i64 无符号取余除零 trap");
                    (ua % ub) as i64
                }
                0x83 => a & b,
                0x84 => a | b,
                0x85 => a ^ b,
                0x86 => a.wrapping_shl(sh),
                0x87 => a.wrapping_shr(sh),
                0x88 => ua.wrapping_shr(sh) as i64,
                0x89 => ua.rotate_left(sh) as i64,
                _ => ua.rotate_right(sh) as i64,
            };
            stack.push(Val::I64(v));
        }
        0xa7 => {
            let v = pop_i64(stack)? as i32;
            stack.push(Val::I32(v));
        }
        0xac => {
            let v = i64::from(pop_i32(stack)?);
            stack.push(Val::I64(v));
        }
        0xad => {
            let v = i64::from(pop_i32(stack)? as u32);
            stack.push(Val::I64(v));
        }
        0xc0 => {
            let v = i32::from(pop_i32(stack)? as i8);
            stack.push(Val::I32(v));
        }
        0xc1 => {
            let v = i32::from(pop_i32(stack)? as i16);
            stack.push(Val::I32(v));
        }
        0xc2 => {
            let v = i64::from(pop_i64(stack)? as i8);
            stack.push(Val::I64(v));
        }
        0xc3 => {
            let v = i64::from(pop_i64(stack)? as i16);
            stack.push(Val::I64(v));
        }
        0xc4 => {
            let v = i64::from(pop_i64(stack)? as i32);
            stack.push(Val::I64(v));
        }
        _ => bail!("未知整数指令 0x{op:02x}"),
    }
    Ok(())
}
