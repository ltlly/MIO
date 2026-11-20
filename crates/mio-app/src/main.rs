#![cfg_attr(not(test), windows_subsystem = "windows")]

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("MIO 无法完成运行：{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let initial_project = parse_args(std::env::args_os().skip(1))?;
    let preferences = mio_core::default_user_preferences_path()?;
    let (runtime, client) = mio_core::Runtime::start_app_with_preferences(preferences)?;
    mio_ui::run_with_project(runtime, client, initial_project)
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<Option<PathBuf>> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(None);
    };
    anyhow::ensure!(
        args.next().is_none(),
        "仅支持零个或一个项目路径参数；未忽略多余命令行参数"
    );
    let bytes = first.as_encoded_bytes();
    anyhow::ensure!(!bytes.is_empty(), "启动项目路径参数不能为空");
    anyhow::ensure!(!bytes.contains(&0), "启动项目路径参数不能包含空字节");
    Ok(Some(PathBuf::from(first)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_zero_or_one_project_path_and_rejects_extra_or_empty_arguments() {
        assert_eq!(parse_args([]).unwrap(), None);
        assert_eq!(
            parse_args([OsString::from("/tmp/sample.mio")]).unwrap(),
            Some(PathBuf::from("/tmp/sample.mio"))
        );
        assert!(parse_args([OsString::from("")]).is_err());
        assert!(
            parse_args([
                OsString::from("/tmp/first.mio"),
                OsString::from("/tmp/second.mio"),
            ])
            .is_err()
        );
    }
}
