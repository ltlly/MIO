use std::{
    cell::{Cell, OnceCell, RefCell},
    rc::Rc,
};

use anyhow::{Result, ensure};
use rhai::{AST, Dynamic, Engine, EvalAltResult, ImmutableString};
use tokio_util::sync::CancellationToken;

pub(super) use crate::lru::BoundedLruCache;

use super::{MAX_BODY, builtins, highlight::ScriptRequirements};

pub(super) const SCRIPT_CANCELLED_MESSAGE: &str = "脚本执行已取消";
pub(super) const MIO_SCRIPT_AST_CACHE_LIMIT: usize = 128;
const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_CAPTURED_PRINT_LINES: usize = 256;

#[derive(Clone)]
pub(super) struct CompiledScript {
    pub(super) ast: Rc<AST>,
    pub(super) requirements: ScriptRequirements,
}

// `rhai::Engine` and `rhai::AST` are `!Send + !Sync` when `rhai/sync` is disabled,
// so each worker thread lazily builds one shared `Engine` in `OnceCell` and reuses
// it across all filter, match-replace, scan check, and custom action evaluations.
thread_local! {
    static ACTIVE_CANCEL: RefCell<Option<CancellationToken>> = const { RefCell::new(None) };
    static ACTIVE_PRINT_SINK: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    static ENGINE_BUILD_COUNT: Cell<usize> = const { Cell::new(0) };
    static SHARED_ENGINE: OnceCell<Engine> = const { OnceCell::new() };
    static MIO_SCRIPT_AST_CACHE: RefCell<BoundedLruCache<String, CompiledScript>> =
        RefCell::new(BoundedLruCache::new(MIO_SCRIPT_AST_CACHE_LIMIT));
}

fn build_engine() -> Engine {
    ENGINE_BUILD_COUNT.with(|count| count.set(count.get() + 1));

    let mut engine = Engine::new();
    engine.set_max_operations(50_000);
    engine.set_max_string_size(MAX_BODY);
    engine.set_max_array_size(4096);
    engine.set_max_map_size(4096);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(64, 32);

    builtins::register_builtins(&mut engine);

    engine.on_progress(|_| {
        ACTIVE_CANCEL.with(|cell| {
            if cell
                .borrow()
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                Some(Dynamic::from(SCRIPT_CANCELLED_MESSAGE))
            } else {
                None
            }
        })
    });

    engine.on_print(|text| {
        ACTIVE_PRINT_SINK.with(|cell| {
            if let Some(lines) = cell.borrow_mut().as_mut()
                && lines.len() < MAX_CAPTURED_PRINT_LINES
            {
                lines.push(text.to_owned());
            }
        });
    });

    engine
}

pub(super) fn with_shared_engine<R>(f: impl FnOnce(&Engine) -> R) -> R {
    SHARED_ENGINE.with(|cell| {
        let engine = cell.get_or_init(build_engine);
        f(engine)
    })
}

pub(super) struct ActiveCancelGuard(Option<CancellationToken>);

impl ActiveCancelGuard {
    pub(super) fn set(token: Option<CancellationToken>) -> Self {
        let previous = ACTIVE_CANCEL.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), token));
        Self(previous)
    }
}

impl Drop for ActiveCancelGuard {
    fn drop(&mut self) {
        ACTIVE_CANCEL.with(|cell| {
            *cell.borrow_mut() = self.0.take();
        });
    }
}

pub(super) struct PrintCaptureGuard {
    previous: Option<Vec<String>>,
    finished: bool,
}

impl PrintCaptureGuard {
    pub(super) fn start() -> Self {
        let previous = ACTIVE_PRINT_SINK.with(|cell| cell.borrow_mut().replace(Vec::new()));
        Self {
            previous,
            finished: false,
        }
    }

    pub(super) fn finish(mut self) -> Vec<String> {
        self.finished = true;
        ACTIVE_PRINT_SINK.with(|cell| {
            std::mem::replace(&mut *cell.borrow_mut(), self.previous.take()).unwrap_or_default()
        })
    }
}

impl Drop for PrintCaptureGuard {
    fn drop(&mut self) {
        if !self.finished {
            ACTIVE_PRINT_SINK.with(|cell| {
                *cell.borrow_mut() = self.previous.take();
            });
        }
    }
}

pub(super) fn compile_cached(script: &str) -> Result<CompiledScript> {
    ensure!(!script.trim().is_empty(), "脚本不能为空");
    ensure!(script.len() <= MAX_SCRIPT_BYTES, "脚本超过 64 KiB 上限");

    if let Some(cached) = MIO_SCRIPT_AST_CACHE.with(|cache| cache.borrow_mut().get(script)) {
        return Ok(cached);
    }

    let ast = with_shared_engine(|engine| {
        engine
            .compile(script)
            .map_err(|error| anyhow::anyhow!("MioScript 编译错误：{error}"))
    })?;
    let compiled = CompiledScript {
        ast: Rc::new(ast),
        requirements: ScriptRequirements::analyze(script),
    };

    MIO_SCRIPT_AST_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(script.to_owned(), compiled.clone());
    });

    Ok(compiled)
}

/// Maps a Rhai evaluation error, surfacing custom termination tokens (such as cancellation).
pub(super) fn format_eval_error(err: Box<EvalAltResult>, context: &str) -> anyhow::Error {
    if let EvalAltResult::ErrorTerminated(token, _) = err.as_ref()
        && let Some(msg) = token.clone().try_cast::<ImmutableString>()
        && !msg.is_empty()
    {
        return anyhow::anyhow!("{msg}");
    }
    anyhow::anyhow!("{context}：{err}")
}

/// Validates MioScript syntax and limits using the shared `Engine` and AST cache.
pub fn validate_filter_script(script: &str) -> Result<()> {
    compile_cached(script).map(|_| ())
}

#[cfg(test)]
pub(super) fn shared_engine_build_count() -> usize {
    ENGINE_BUILD_COUNT.with(Cell::get)
}

#[cfg(test)]
pub(super) fn ast_cache_contains(script: &str) -> bool {
    MIO_SCRIPT_AST_CACHE.with(|cache| cache.borrow().contains_key(script))
}
