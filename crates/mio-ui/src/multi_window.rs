use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
#[cfg(test)]
use std::{cell::RefCell, rc::Rc};

use anyhow::{Context as _, Result};
use gpui_kit::{App, Global};

#[cfg(test)]
type LaunchInterceptor = Rc<RefCell<Vec<Option<PathBuf>>>>;

#[derive(Clone, Default)]
pub(crate) struct WindowLauncher {
    #[cfg(test)]
    intercepted: Option<LaunchInterceptor>,
}

impl Global for WindowLauncher {}

impl WindowLauncher {
    pub(crate) fn install(cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
    }

    #[cfg(test)]
    pub(crate) fn install_recorder(cx: &mut App) -> LaunchInterceptor {
        let recorded = Rc::new(RefCell::new(Vec::new()));
        cx.set_global(Self {
            intercepted: Some(recorded.clone()),
        });
        recorded
    }

    pub(crate) fn launch(project_path: Option<&Path>, cx: &mut App) -> Result<()> {
        let resolved_project = match project_path {
            Some(path) => Some(resolve_project_path(path)?),
            None => None,
        };
        #[cfg(test)]
        if let Some(intercepted) = cx
            .try_global::<Self>()
            .and_then(|launcher| launcher.intercepted.clone())
        {
            intercepted.borrow_mut().push(resolved_project);
            return Ok(());
        }
        #[cfg(not(test))]
        let _ = cx;
        spawn_mio_instance(resolved_project.as_deref())
    }
}

fn resolve_project_path(path: &Path) -> Result<PathBuf> {
    let bytes = path.as_os_str().as_encoded_bytes();
    anyhow::ensure!(!bytes.is_empty(), "启动项目路径不能为空");
    anyhow::ensure!(!bytes.contains(&0), "启动项目路径不能包含空字节");
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("无法解析相对项目路径的当前工作目录")?
            .join(path))
    }
}

pub(crate) fn resolve_macos_app_bundle(exe_path: &Path) -> Option<PathBuf> {
    for ancestor in exe_path.ancestors() {
        if ancestor.extension().is_some_and(|ext| ext == "app") && ancestor.is_dir() {
            let macos_bin = ancestor.join("Contents").join("MacOS");
            if exe_path.starts_with(&macos_bin) {
                return Some(ancestor.to_path_buf());
            }
        }
    }
    None
}

pub(crate) fn build_spawn_command_spec(
    current_exe: &Path,
    project_path: Option<&Path>,
    is_macos: bool,
) -> (PathBuf, Vec<OsString>) {
    if is_macos
        && let Some(bundle) = resolve_macos_app_bundle(current_exe)
    {
        let mut args = vec![OsString::from("-n"), OsString::from("-a"), bundle.into_os_string()];
        if let Some(project) = project_path {
            args.push(OsString::from("--args"));
            args.push(OsString::from("--"));
            args.push(project.as_os_str().to_os_string());
        }
        return (PathBuf::from("/usr/bin/open"), args);
    }
    let mut args = Vec::new();
    if let Some(project) = project_path {
        args.push(OsString::from("--"));
        args.push(project.as_os_str().to_os_string());
    }
    (current_exe.to_path_buf(), args)
}

pub(crate) fn spawn_mio_instance(project_path: Option<&Path>) -> Result<()> {
    let current_exe = std::env::current_exe().context("无法定位当前 MIO 可执行文件路径")?;
    let (program, args) = build_spawn_command_spec(&current_exe, project_path, cfg!(target_os = "macos"));
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .env_remove(mio_core::ENV_MIO_IPC_SOCKET)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("无法启动新的 MIO 窗口进程（{}）", program.display()))?;
    #[cfg(unix)]
    {
        let _ = std::thread::Builder::new()
            .name("mio-reap".into())
            .spawn(move || {
                let _ = child.wait();
            });
    }
    #[cfg(not(unix))]
    let _ = &mut child;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_spec_uses_open_for_macos_app_bundle_and_direct_binary_otherwise() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("MIO.app");
        let macos_dir = bundle.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos_dir).unwrap();
        let bundled_exe = macos_dir.join("mio");
        std::fs::write(&bundled_exe, b"binary").unwrap();
        let sample_proj = temp.path().join("demo.mio");

        assert_eq!(
            resolve_macos_app_bundle(&bundled_exe),
            Some(bundle.clone())
        );

        let (prog_mac_empty, args_mac_empty) =
            build_spawn_command_spec(&bundled_exe, None, true);
        assert_eq!(prog_mac_empty, PathBuf::from("/usr/bin/open"));
        assert_eq!(
            args_mac_empty,
            vec![
                OsString::from("-n"),
                OsString::from("-a"),
                bundle.clone().into_os_string(),
            ]
        );

        let (prog_mac_proj, args_mac_proj) =
            build_spawn_command_spec(&bundled_exe, Some(&sample_proj), true);
        assert_eq!(prog_mac_proj, PathBuf::from("/usr/bin/open"));
        assert_eq!(
            args_mac_proj,
            vec![
                OsString::from("-n"),
                OsString::from("-a"),
                bundle.into_os_string(),
                OsString::from("--args"),
                OsString::from("--"),
                sample_proj.clone().into_os_string(),
            ]
        );

        let standalone = temp.path().join("mio");
        std::fs::write(&standalone, b"binary").unwrap();
        assert_eq!(resolve_macos_app_bundle(&standalone), None);
        let (prog_bin, args_bin) =
            build_spawn_command_spec(&standalone, Some(&sample_proj), true);
        assert_eq!(prog_bin, standalone);
        assert_eq!(
            args_bin,
            vec![OsString::from("--"), sample_proj.clone().into_os_string()]
        );

        let (prog_other, args_other) =
            build_spawn_command_spec(&bundled_exe, Some(&sample_proj), false);
        assert_eq!(prog_other, bundled_exe);
        assert_eq!(
            args_other,
            vec![OsString::from("--"), sample_proj.into_os_string()]
        );
    }
}
