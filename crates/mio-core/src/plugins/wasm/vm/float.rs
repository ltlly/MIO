use anyhow::{Result, bail, ensure};

use super::{Val, pop, pop_i32, pop_i64};

fn pop_f32(stack: &mut Vec<Val>) -> Result<f32> {
    match pop(stack)? {
        Val::F32(bits) => Ok(f32::from_bits(bits)),
        _ => bail!("需要 f32 值"),
    }
}

fn pop_f64(stack: &mut Vec<Val>) -> Result<f64> {
    match pop(stack)? {
        Val::F64(bits) => Ok(f64::from_bits(bits)),
        _ => bail!("需要 f64 值"),
    }
}

pub(super) fn float_op(op: u8, stack: &mut Vec<Val>) -> Result<()> {
    match op {
        0x5b..=0x60 => {
            let b = pop_f32(stack)?;
            let a = pop_f32(stack)?;
            let res = match op {
                0x5b => a == b,
                0x5c => a != b,
                0x5d => a < b,
                0x5e => a > b,
                0x5f => a <= b,
                _ => a >= b,
            };
            stack.push(Val::I32(i32::from(res)));
        }
        0x61..=0x66 => {
            let b = pop_f64(stack)?;
            let a = pop_f64(stack)?;
            let res = match op {
                0x61 => a == b,
                0x62 => a != b,
                0x63 => a < b,
                0x64 => a > b,
                0x65 => a <= b,
                _ => a >= b,
            };
            stack.push(Val::I32(i32::from(res)));
        }
        0x8b..=0x91 => {
            let a = pop_f32(stack)?;
            let res = match op {
                0x8b => f32::from_bits(a.to_bits() & 0x7fff_ffff),
                0x8c => f32::from_bits(a.to_bits() ^ 0x8000_0000),
                0x8d => a.ceil(),
                0x8e => a.floor(),
                0x8f => a.trunc(),
                0x90 => a.round_ties_even(),
                _ => a.sqrt(),
            };
            stack.push(Val::F32(res.to_bits()));
        }
        0x92..=0x98 => {
            let b = pop_f32(stack)?;
            let a = pop_f32(stack)?;
            let res = match op {
                0x92 => a + b,
                0x93 => a - b,
                0x94 => a * b,
                0x95 => a / b,
                0x96 => {
                    if a.is_nan() || b.is_nan() {
                        f32::NAN
                    } else if a == 0.0 && b == 0.0 && (a.is_sign_negative() || b.is_sign_negative())
                    {
                        -0.0
                    } else {
                        a.min(b)
                    }
                }
                0x97 => {
                    if a.is_nan() || b.is_nan() {
                        f32::NAN
                    } else if a == 0.0 && b == 0.0 && (a.is_sign_positive() || b.is_sign_positive())
                    {
                        0.0
                    } else {
                        a.max(b)
                    }
                }
                _ => f32::from_bits((a.to_bits() & 0x7fff_ffff) | (b.to_bits() & 0x8000_0000)),
            };
            stack.push(Val::F32(res.to_bits()));
        }
        0x99..=0x9f => {
            let a = pop_f64(stack)?;
            let res = match op {
                0x99 => f64::from_bits(a.to_bits() & 0x7fff_ffff_ffff_ffff),
                0x9a => f64::from_bits(a.to_bits() ^ 0x8000_0000_0000_0000),
                0x9b => a.ceil(),
                0x9c => a.floor(),
                0x9d => a.trunc(),
                0x9e => a.round_ties_even(),
                _ => a.sqrt(),
            };
            stack.push(Val::F64(res.to_bits()));
        }
        0xa0..=0xa6 => {
            let b = pop_f64(stack)?;
            let a = pop_f64(stack)?;
            let res = match op {
                0xa0 => a + b,
                0xa1 => a - b,
                0xa2 => a * b,
                0xa3 => a / b,
                0xa4 => {
                    if a.is_nan() || b.is_nan() {
                        f64::NAN
                    } else if a == 0.0 && b == 0.0 && (a.is_sign_negative() || b.is_sign_negative())
                    {
                        -0.0
                    } else {
                        a.min(b)
                    }
                }
                0xa5 => {
                    if a.is_nan() || b.is_nan() {
                        f64::NAN
                    } else if a == 0.0 && b == 0.0 && (a.is_sign_positive() || b.is_sign_positive())
                    {
                        0.0
                    } else {
                        a.max(b)
                    }
                }
                _ => f64::from_bits(
                    (a.to_bits() & 0x7fff_ffff_ffff_ffff) | (b.to_bits() & 0x8000_0000_0000_0000),
                ),
            };
            stack.push(Val::F64(res.to_bits()));
        }
        0xa8..=0xab => {
            let v = match op {
                0xa8 | 0xa9 => f64::from(pop_f32(stack)?),
                _ => pop_f64(stack)?,
            }
            .trunc();
            ensure!(v.is_finite(), "浮点截断 NaN/Inf trap");
            match op {
                0xa8 | 0xaa => {
                    ensure!(
                        v >= f64::from(i32::MIN) && v <= f64::from(i32::MAX),
                        "i32.trunc 溢出"
                    );
                    stack.push(Val::I32(v as i32));
                }
                _ => {
                    ensure!(v >= 0.0 && v <= f64::from(u32::MAX), "i32.trunc_u 溢出");
                    stack.push(Val::I32((v as u32) as i32));
                }
            }
        }
        0xae..=0xb1 => {
            let v = match op {
                0xae | 0xaf => f64::from(pop_f32(stack)?),
                _ => pop_f64(stack)?,
            }
            .trunc();
            ensure!(v.is_finite(), "浮点截断 NaN/Inf trap");
            match op {
                0xae | 0xb0 => {
                    ensure!(
                        (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&v),
                        "i64.trunc 溢出"
                    );
                    stack.push(Val::I64(v as i64));
                }
                _ => {
                    ensure!(
                        (0.0..18_446_744_073_709_551_616.0).contains(&v),
                        "i64.trunc_u 溢出"
                    );
                    stack.push(Val::I64((v as u64) as i64));
                }
            }
        }
        0xb2 => {
            let v = pop_i32(stack)? as f32;
            stack.push(Val::F32(v.to_bits()));
        }
        0xb3 => {
            let v = (pop_i32(stack)? as u32) as f32;
            stack.push(Val::F32(v.to_bits()));
        }
        0xb4 => {
            let v = pop_i64(stack)? as f32;
            stack.push(Val::F32(v.to_bits()));
        }
        0xb5 => {
            let v = (pop_i64(stack)? as u64) as f32;
            stack.push(Val::F32(v.to_bits()));
        }
        0xb6 => {
            let v = pop_f64(stack)? as f32;
            stack.push(Val::F32(v.to_bits()));
        }
        0xb7 => {
            let v = f64::from(pop_i32(stack)?);
            stack.push(Val::F64(v.to_bits()));
        }
        0xb8 => {
            let v = f64::from(pop_i32(stack)? as u32);
            stack.push(Val::F64(v.to_bits()));
        }
        0xb9 => {
            let v = pop_i64(stack)? as f64;
            stack.push(Val::F64(v.to_bits()));
        }
        0xba => {
            let v = (pop_i64(stack)? as u64) as f64;
            stack.push(Val::F64(v.to_bits()));
        }
        0xbb => {
            let v = f64::from(pop_f32(stack)?);
            stack.push(Val::F64(v.to_bits()));
        }
        0xbc => {
            let v = match pop(stack)? {
                Val::F32(bits) => bits as i32,
                _ => bail!("需要 f32 值"),
            };
            stack.push(Val::I32(v));
        }
        0xbd => {
            let v = match pop(stack)? {
                Val::F64(bits) => bits as i64,
                _ => bail!("需要 f64 值"),
            };
            stack.push(Val::I64(v));
        }
        0xbe => {
            let v = pop_i32(stack)? as u32;
            stack.push(Val::F32(v));
        }
        0xbf => {
            let v = pop_i64(stack)? as u64;
            stack.push(Val::F64(v));
        }
        _ => bail!("未知浮点指令 0x{op:02x}"),
    }
    Ok(())
}

pub(super) fn sat_trunc(sub: u8, stack: &mut Vec<Val>) -> Result<()> {
    let val = match sub {
        0 => Val::I32(pop_f32(stack)? as i32),
        1 => Val::I32((pop_f32(stack)? as u32) as i32),
        2 => Val::I32(pop_f64(stack)? as i32),
        3 => Val::I32((pop_f64(stack)? as u32) as i32),
        4 => Val::I64(pop_f32(stack)? as i64),
        5 => Val::I64((pop_f32(stack)? as u64) as i64),
        6 => Val::I64(pop_f64(stack)? as i64),
        7 => Val::I64((pop_f64(stack)? as u64) as i64),
        _ => bail!("未知饱和截断指令 {sub}"),
    };
    stack.push(val);
    Ok(())
}
