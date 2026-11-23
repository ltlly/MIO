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
    let mut args = args.into_iter().peekable();
    let mut project_path: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        anyhow::ensure!(!bytes.is_empty(), "启动项目路径参数不能为空");
        anyhow::ensure!(!bytes.contains(&0), "启动项目路径参数不能包含空字节");
        if arg == "-n" || arg == "--new-window" {
            continue;
        }
        if arg == "--dock-action" {
            let index = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--dock-action 缺少动作序号参数"))?;
            let index_str = index
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("--dock-action 动作序号无效"))?;
            let _: usize = index_str
                .parse()
                .map_err(|_| anyhow::anyhow!("--dock-action 动作序号无效"))?;
            continue;
        }
        if bytes.starts_with(b"-psn_") {
            continue;
        }
        anyhow::ensure!(
            project_path.is_none(),
            "仅支持零个或一个项目路径参数；未忽略多余命令行参数"
        );
        project_path = Some(PathBuf::from(arg));
    }
    Ok(project_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_zero_or_one_project_path_and_rejects_extra_or_empty_arguments() {
        assert_eq!(parse_args([]).unwrap(), None);
        assert_eq!(parse_args([OsString::from("--new-window")]).unwrap(), None);
        assert_eq!(parse_args([OsString::from("-n")]).unwrap(), None);
        assert_eq!(
            parse_args([OsString::from("--dock-action"), OsString::from("0")]).unwrap(),
            None
        );
        assert_eq!(
            parse_args([OsString::from("-psn_0_12345")]).unwrap(),
            None
        );
        assert_eq!(
            parse_args([OsString::from("/tmp/sample.mio")]).unwrap(),
            Some(PathBuf::from("/tmp/sample.mio"))
        );
        assert_eq!(
            parse_args([
                OsString::from("--new-window"),
                OsString::from("/tmp/sample.mio"),
            ])
            .unwrap(),
            Some(PathBuf::from("/tmp/sample.mio"))
        );
        assert!(parse_args([OsString::from("")]).is_err());
        assert!(parse_args([OsString::from("--dock-action")]).is_err());
        assert!(
            parse_args([OsString::from("--dock-action"), OsString::from("bad")]).is_err()
        );
        assert!(
            parse_args([
                OsString::from("/tmp/first.mio"),
                OsString::from("/tmp/second.mio"),
            ])
            .is_err()
        );
    }
}
