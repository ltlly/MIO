//! Local command-path acceptance check, not a External compatibility or cold-load benchmark.

#[path = "ext_archive_project_check/cli.rs"]
mod cli;
#[path = "ext_archive_project_check/model.rs"]
mod model;
#[path = "ext_archive_project_check/runner.rs"]
mod runner;
#[path = "ext_archive_project_check/runtime.rs"]
mod runtime;
#[cfg(all(test, unix))]
#[path = "ext_archive_project_check/tests.rs"]
mod tests;

pub(crate) use cli::*;
pub(crate) use model::*;
pub(crate) use runner::*;
pub(crate) use runtime::*;
fn main() -> ExitCode {
    let result = arguments(std::env::args_os().skip(1))
        .and_then(|paths| check(&paths, &mut std::io::stdout().lock(), WORK_BUDGET));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("project check failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}
