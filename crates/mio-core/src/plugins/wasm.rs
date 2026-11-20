//! Minimal, fail-closed Wasm interpreter for trusted MIO plugin modules.
use anyhow::Result;
use tokio_util::sync::CancellationToken;

const MAGIC: [u8; 8] = [0, b'a', b's', b'm', 1, 0, 0, 0];
const PAGE_BYTES: usize = 65_536;
pub const MAX_MEMORY_PAGES: u32 = 256;
const I32: u8 = 0x7f;
const I64: u8 = 0x7e;
const F32: u8 = 0x7d;
const F64: u8 = 0x7c;
const FUNCREF: u8 = 0x70;
pub const WASM_CALL_FUEL: u64 = 2_000_000;
pub const WASM_HOST_CALL_FUEL: u64 = 2_000;
const SCRATCH_BASE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Val {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
}

impl Val {
    pub(super) fn zero(vt: u8) -> Self {
        match vt {
            I32 => Self::I32(0),
            I64 => Self::I64(0),
            F32 => Self::F32(0),
            _ => Self::F64(0),
        }
    }

    pub(super) fn to_raw_i64(self) -> i64 {
        match self {
            Self::I32(v) => i64::from(v),
            Self::I64(v) => v,
            Self::F32(v) => i64::from(v),
            Self::F64(v) => v as i64,
        }
    }

    pub(super) fn from_arg(vt: u8, raw: i64) -> Self {
        match vt {
            I32 => Self::I32(raw as i32),
            I64 => Self::I64(raw),
            F32 => Self::F32(raw as u32),
            _ => Self::F64(raw as u64),
        }
    }

    pub(super) fn vtype(self) -> u8 {
        match self {
            Self::I32(_) => I32,
            Self::I64(_) => I64,
            Self::F32(_) => F32,
            Self::F64(_) => F64,
        }
    }
}

type Op = (u8, i64, usize, usize);
type Label = (usize, usize, usize, usize, bool);
type Frame = (usize, usize, Vec<Val>, Vec<Label>);
type WasmHostFn<'a> =
    &'a mut dyn FnMut(&[u8], &mut [u8], bool, &CancellationToken) -> Result<usize>;

#[derive(Clone, Debug)]
pub(super) enum DataSeg {
    Active(u32, Vec<u8>),
    Passive(Vec<u8>),
}

#[derive(Clone, Debug, Default)]
pub struct WasmModule {
    types: Vec<(Vec<u8>, Vec<u8>)>,
    funcs: Vec<u32>,
    host_import: Option<(u8, u32)>,
    table: Option<(u32, u32)>,
    memory: Option<(u32, u32)>,
    globals: Vec<(u8, bool, Val)>,
    exports: Vec<(String, u8, u32)>,
    elements: Vec<(u32, Vec<u32>)>,
    data_count: Option<usize>,
    code: Vec<(Vec<u8>, Vec<Op>)>,
    data: Vec<DataSeg>,
}

#[derive(Clone, Debug)]
pub struct WasmInstance {
    m: WasmModule,
    mem: Vec<u8>,
    table: Vec<Option<u32>>,
    g: Vec<Val>,
    passive_data: Vec<Option<Vec<u8>>>,
}

mod envelope_consts;
mod envelope_handlers;
mod envelope_hooks;
mod envelope_module;
mod envelope_ops;
mod envelope_wire;
mod legacy;
mod module;
mod parser;
mod runner;
mod vm;

#[cfg(test)]
use legacy::run_wasm_op;
pub use module::{
    build_demo_wasm_module, build_envelope_wasm_module, default_sandbox_panel, initial_wasm_panel,
};
pub use runner::dispatch_wasm_envelope;
pub(crate) use runner::spawn_sandbox_worker;

use super::Wire;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_envelope;
#[cfg(test)]
mod tests_envelope_worker;
