extern crate gpui_kit as gpui;

mod chrome;
mod editing;
mod history;
mod local_time;
mod native_terminate;
mod plugin_panel;
mod theme;
mod workbench;

#[cfg(test)]
macro_rules! kbd {
    ("alt-c") => {{
        #[cfg(target_os = "macos")]
        {
            "alt-cmd-c"
        }
        #[cfg(not(target_os = "macos"))]
        {
            "alt-ctrl-c"
        }
    }};
    ("shift-z") => {{
        #[cfg(target_os = "macos")]
        {
            "cmd-shift-z"
        }
        #[cfg(not(target_os = "macos"))]
        {
            "ctrl-y"
        }
    }};
    ("up") => {{
        #[cfg(target_os = "macos")]
        {
            "cmd-up"
        }
        #[cfg(not(target_os = "macos"))]
        {
            "ctrl-home"
        }
    }};
    ($rest:literal) => {{
        #[cfg(target_os = "macos")]
        {
            concat!("cmd-", $rest)
        }
        #[cfg(not(target_os = "macos"))]
        {
            concat!("ctrl-", $rest)
        }
    }};
}
#[cfg(test)]
pub(crate) use kbd;

#[cfg(test)]
pub(crate) fn secondary_modifiers(active: bool) -> gpui_kit::Modifiers {
    #[cfg(target_os = "macos")]
    {
        gpui_kit::Modifiers {
            platform: active,
            ..Default::default()
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        gpui_kit::Modifiers {
            control: active,
            ..Default::default()
        }
    }
}

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gpui_kit::{
    App, AppContext, Bounds, KeyBinding, Menu, MenuItem, OsAction,
    component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo},
    px, size,
};
use mio_core::Client;

gpui_kit::actions!(
    mio,
    [
        NewProject,
        OpenProject,
        ImportExtProject,
        ShowExtReport,
        SaveProject,
        SaveProjectAs,
        SlimSaveProjectAs,
        ExportCa,
        OpenSettings,
        OpenUserSettings,
        OpenProjectSettings,
        SearchSettings,
        OpenProxySettings,
        OpenIntruderSettings,
        OpenRepeaterSettings,
        OpenScopeSettings,
        CloseSettings,
        Quit,
        ToggleProxyIntercept,
        OpenConfiguredBrowser,
        StartIntruderAttack,
        OpenSavedIntruderAttacks,
        AutoMarkIntruderPositions,
        ClearIntruderPositions,
        DuplicateActiveTab,
        RestoreDefaultTabLayout,
        ToggleRepeaterUpdateContentLength,
        ToggleRepeaterUnpackCompressed,
        ToggleRepeaterNormalizeLineEndings,
        ToggleRepeaterHttp1Close,
        ToggleRepeaterStripH2Connection,
        SetRepeaterRedirectNever,
        SetRepeaterRedirectOnSite,
        SetRepeaterRedirectInScope,
        SetRepeaterRedirectAlways,
        OpenInIntruder,
        OpenInRepeater,
        SendToDecoder,
        SendToSequencer,
        SendToScanner,
        OpenScanLauncher,
        SendToPassiveScanner,
        SendToOrganiser,
        SendToMioAt,
        AddToSiteMap,
        SendToComparerRequest,
        SendToComparerResponse,
        OpenResponseInBrowser,
        CopyUrl,
        CopyCurlCommand,
        CopyResponse,
        CopyLinksInResponse,
        CopyPrettified,
        ChangeRequestMethod,
        ChangeRequestMethodGet,
        ChangeRequestMethodPost,
        ChangeRequestMethodPut,
        ChangeRequestMethodDelete,
        ChangeRequestMethodPatch,
        ChangeRequestMethodOptions,
        ChangeBodyEncodingToggle,
        ChangeBodyEncodingForm,
        ChangeBodyEncodingMultipart,
        ChangeBodyEncodingJson,
        PasteUrlAsRequest,
        CopyToFile,
        SaveSelectedTextToFile,
        CopyResponseBodyToFile,
        ExportSelectedHistoryXml,
        PasteFromFile,
        PasteBase64FromFile,
        InsertCollaboratorPayload,
        DeleteSelectedHttpHistory,
        ClearLiveProxyHistory,
        AddToScope,
        ExcludeFromScope,
        AddHttpNotes,
        SendRequest,
        CancelRequest,
        NewTab,
        CloseTab,
        RestoreTab,
        NextTab,
        PreviousTab,
        MoveTabUp,
        MoveTabDown,
        SelectToolDashboard,
        SelectToolTarget,
        SelectToolProxy,
        SelectToolIntruder,
        SelectToolRepeater,
        SelectToolCollaborator,
        SelectToolSequencer,
        SelectToolDecoder,
        SelectToolComparer,
        SelectToolLogger,
        SelectToolOrganiser,
        SelectToolExtensions,
        SelectToolDiscover,
        ToggleTabOrientation,
        OpenCommandPalette,
        OpenGlobalSearch,
        MinimizeWindow,
        ZoomWindow,
        ToggleSettingsWindow,
        BringAllWindowsToFront,
        OpenDocumentation,
        OpenKeyboardShortcuts,
        RunSystemDiagnostics,
        OpenAboutMio,
    ]
);

fn init_actions(cx: &mut App) {
    gpui_kit::component::native_menu::NativeMenu::set_use_drawn_popup(true);
    cx.on_action(|_: &Quit, cx| cx.global::<native_terminate::NativeTerminate>().request());
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-k", OpenCommandPalette, Some("MioWorkspace")),
        KeyBinding::new("cmd-shift-p", OpenCommandPalette, Some("MioWorkspace")),
        KeyBinding::new("cmd-shift-f", OpenGlobalSearch, Some("MioWorkspace")),
        KeyBinding::new("escape", CloseSettings, Some("MioSettings")),
        KeyBinding::new("cmd-w", CloseSettings, Some("MioSettings")),
        KeyBinding::new("escape", CloseSettings, Some("MioRepeaterTarget")),
        KeyBinding::new("escape", CloseSettings, Some("MioRepeaterTarget > Input")),
        KeyBinding::new("cmd-w", CloseSettings, Some("MioRepeaterTarget")),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-s", SaveProject, Some("MioSettings")),
        KeyBinding::new("cmd-shift-s", SaveProjectAs, Some("MioSettings")),
        KeyBinding::new("cmd-n", NewProject, Some("MioWorkspace")),
        KeyBinding::new("cmd-o", OpenProject, Some("MioWorkspace")),
        KeyBinding::new("cmd-shift-o", OpenProject, Some("MioWorkspace")),
        KeyBinding::new("cmd-s", SaveProject, Some("MioWorkspace")),
        KeyBinding::new("cmd-shift-s", SaveProjectAs, Some("MioWorkspace")),
        KeyBinding::new("cmd-i", OpenInIntruder, Some("MioWorkspace")),
        KeyBinding::new("cmd-r", OpenInRepeater, Some("MioWorkspace")),
        KeyBinding::new("cmd-o", SendToOrganiser, Some("MioWorkspace > Input")),
        KeyBinding::new("cmd-o", SendToOrganiser, Some("MioWorkspace > DataTable")),
        KeyBinding::new("cmd-o", SendToOrganiser, Some("MioWorkspace > HexGrid")),
        KeyBinding::new("cmd-g", SendToMioAt, Some("MioWorkspace")),
        KeyBinding::new(
            "cmd-u",
            editing::ConvertKind::UrlEncodeKey,
            Some("MioWorkspace"),
        ),
        KeyBinding::new(
            "cmd-shift-u",
            editing::ConvertKind::UrlDecode,
            Some("MioWorkspace"),
        ),
        KeyBinding::new(
            "cmd-b",
            editing::ConvertKind::Base64Encode,
            Some("MioWorkspace"),
        ),
        KeyBinding::new(
            "cmd-shift-b",
            editing::ConvertKind::Base64Decode,
            Some("MioWorkspace"),
        ),
        KeyBinding::new("alt-cmd-c", CopyPrettified, Some("MioWorkspace")),
        KeyBinding::new("cmd-shift-c", CopyUrl, Some("MioWorkspace")),
        KeyBinding::new("cmd-alt-m", ChangeRequestMethod, Some("MioWorkspace")),
        KeyBinding::new("cmd-alt-e", ChangeBodyEncodingToggle, Some("MioWorkspace")),
        KeyBinding::new("cmd-d", DuplicateActiveTab, Some("MioWorkspace")),
        KeyBinding::new("cmd-1", SelectToolDashboard, Some("MioWorkspace")),
        KeyBinding::new("cmd-2", SelectToolTarget, Some("MioWorkspace")),
        KeyBinding::new("cmd-3", SelectToolProxy, Some("MioWorkspace")),
        KeyBinding::new("cmd-4", SelectToolIntruder, Some("MioWorkspace")),
        KeyBinding::new("cmd-5", SelectToolRepeater, Some("MioWorkspace")),
        KeyBinding::new("cmd-6", SelectToolCollaborator, Some("MioWorkspace")),
        KeyBinding::new("cmd-7", SelectToolSequencer, Some("MioWorkspace")),
        KeyBinding::new("cmd-8", SelectToolDecoder, Some("MioWorkspace")),
        KeyBinding::new("cmd-9", SelectToolComparer, Some("MioWorkspace")),
        KeyBinding::new("cmd-0", SelectToolLogger, Some("MioWorkspace")),
        KeyBinding::new("cmd-enter", SendRequest, Some("MioWorkspace")),
        KeyBinding::new("cmd-enter", SendRequest, Some("MioRepeater > Input")),
        KeyBinding::new("cmd-period", CancelRequest, Some("MioWorkspace")),
        KeyBinding::new("cmd-t", NewTab, Some("MioWorkspace")),
        KeyBinding::new("cmd-w", CloseTab, Some("MioWorkspace")),
        KeyBinding::new("cmd-shift-t", RestoreTab, Some("MioWorkspace")),
        KeyBinding::new("ctrl-tab", NextTab, Some("MioWorkspace")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("MioWorkspace")),
        KeyBinding::new("cmd-alt-up", MoveTabUp, Some("MioWorkspace")),
        KeyBinding::new("cmd-alt-down", MoveTabDown, Some("MioWorkspace")),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("ctrl-k", OpenCommandPalette, Some("MioWorkspace")),
        KeyBinding::new("ctrl-shift-p", OpenCommandPalette, Some("MioWorkspace")),
        KeyBinding::new("ctrl-shift-f", OpenGlobalSearch, Some("MioWorkspace")),
        KeyBinding::new("ctrl-w", CloseSettings, Some("MioSettings")),
        KeyBinding::new("ctrl-w", CloseSettings, Some("MioRepeaterTarget")),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("ctrl-s", SaveProject, Some("MioSettings")),
        KeyBinding::new("ctrl-shift-s", SaveProjectAs, Some("MioSettings")),
        KeyBinding::new("ctrl-n", NewProject, Some("MioWorkspace")),
        KeyBinding::new("ctrl-o", OpenProject, Some("MioWorkspace")),
        KeyBinding::new("ctrl-shift-o", OpenProject, Some("MioWorkspace")),
        KeyBinding::new("ctrl-s", SaveProject, Some("MioWorkspace")),
        KeyBinding::new("ctrl-shift-s", SaveProjectAs, Some("MioWorkspace")),
        KeyBinding::new("ctrl-i", OpenInIntruder, Some("MioWorkspace")),
        KeyBinding::new("ctrl-r", OpenInRepeater, Some("MioWorkspace")),
        KeyBinding::new("ctrl-o", SendToOrganiser, Some("MioWorkspace > Input")),
        KeyBinding::new("ctrl-o", SendToOrganiser, Some("MioWorkspace > DataTable")),
        KeyBinding::new("ctrl-o", SendToOrganiser, Some("MioWorkspace > HexGrid")),
        KeyBinding::new("ctrl-g", SendToMioAt, Some("MioWorkspace")),
        KeyBinding::new(
            "ctrl-u",
            editing::ConvertKind::UrlEncodeKey,
            Some("MioWorkspace"),
        ),
        KeyBinding::new(
            "ctrl-shift-u",
            editing::ConvertKind::UrlDecode,
            Some("MioWorkspace"),
        ),
        KeyBinding::new(
            "ctrl-b",
            editing::ConvertKind::Base64Encode,
            Some("MioWorkspace"),
        ),
        KeyBinding::new(
            "ctrl-shift-b",
            editing::ConvertKind::Base64Decode,
            Some("MioWorkspace"),
        ),
        KeyBinding::new("alt-ctrl-c", CopyPrettified, Some("MioWorkspace")),
        KeyBinding::new("ctrl-shift-c", CopyUrl, Some("MioWorkspace")),
        KeyBinding::new("ctrl-alt-m", ChangeRequestMethod, Some("MioWorkspace")),
        KeyBinding::new("ctrl-alt-e", ChangeBodyEncodingToggle, Some("MioWorkspace")),
        KeyBinding::new("ctrl-d", DuplicateActiveTab, Some("MioWorkspace")),
        KeyBinding::new("ctrl-1", SelectToolDashboard, Some("MioWorkspace")),
        KeyBinding::new("ctrl-2", SelectToolTarget, Some("MioWorkspace")),
        KeyBinding::new("ctrl-3", SelectToolProxy, Some("MioWorkspace")),
        KeyBinding::new("ctrl-4", SelectToolIntruder, Some("MioWorkspace")),
        KeyBinding::new("ctrl-5", SelectToolRepeater, Some("MioWorkspace")),
        KeyBinding::new("ctrl-6", SelectToolCollaborator, Some("MioWorkspace")),
        KeyBinding::new("ctrl-7", SelectToolSequencer, Some("MioWorkspace")),
        KeyBinding::new("ctrl-8", SelectToolDecoder, Some("MioWorkspace")),
        KeyBinding::new("ctrl-9", SelectToolComparer, Some("MioWorkspace")),
        KeyBinding::new("ctrl-0", SelectToolLogger, Some("MioWorkspace")),
        KeyBinding::new("ctrl-enter", SendRequest, Some("MioWorkspace")),
        KeyBinding::new("ctrl-enter", SendRequest, Some("MioRepeater > Input")),
        KeyBinding::new("ctrl-period", CancelRequest, Some("MioWorkspace")),
        KeyBinding::new("ctrl-t", NewTab, Some("MioWorkspace")),
        KeyBinding::new("ctrl-w", CloseTab, Some("MioWorkspace")),
        KeyBinding::new("ctrl-shift-t", RestoreTab, Some("MioWorkspace")),
        KeyBinding::new("ctrl-alt-up", MoveTabUp, Some("MioWorkspace")),
        KeyBinding::new("ctrl-alt-down", MoveTabDown, Some("MioWorkspace")),
    ]);
    cx.set_menus([
        Menu {
            name: "MIO".into(),
            items: vec![
                MenuItem::action("设置", OpenSettings),
                MenuItem::action("用户设置…", OpenUserSettings),
                MenuItem::action("搜索设置…", SearchSettings),
                MenuItem::action("命令面板…", OpenCommandPalette),
                MenuItem::action("全局搜索…", OpenGlobalSearch),
                MenuItem::separator(),
                MenuItem::action("退出 MIO", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "项目".into(),
            items: vec![
                MenuItem::action("新建项目", NewProject),
                MenuItem::action("打开项目…", OpenProject),
                MenuItem::action("导入 外部归档项目…", ImportExtProject),
                MenuItem::action("查看 外部导入记录", ShowExtReport),
                MenuItem::separator(),
                MenuItem::action("项目设置…", OpenProjectSettings),
                MenuItem::separator(),
                MenuItem::action("保存项目", SaveProject),
                MenuItem::action("另存为…", SaveProjectAs),
                MenuItem::action("瘦身另存为…", SlimSaveProjectAs),
                MenuItem::separator(),
                MenuItem::action("导出 Proxy CA…", ExportCa),
            ],
            disabled: false,
        },
        Menu {
            name: "编辑".into(),
            items: vec![
                MenuItem::os_action("撤销", Undo, OsAction::Undo),
                MenuItem::os_action("重做", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("剪切", Cut, OsAction::Cut),
                MenuItem::os_action("复制", Copy, OsAction::Copy),
                MenuItem::os_action("粘贴", Paste, OsAction::Paste),
                MenuItem::os_action("全选", SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("复制美化报文", CopyPrettified),
                MenuItem::action("保存到文件…", CopyToFile),
                MenuItem::action("从文件粘贴…", PasteFromFile),
                MenuItem::action("从文件 Base64 编码插入…", PasteBase64FromFile),
                MenuItem::action("插入 Collaborator Payload", InsertCollaboratorPayload),
                MenuItem::action("在 CyberChef 中打开", editing::OpenSelectionInCyberChef),
            ],
            disabled: false,
        },
        Menu {
            name: "视图".into(),
            items: vec![
                MenuItem::action("恢复默认标签布局", RestoreDefaultTabLayout),
                MenuItem::action("切换工作标签方向", ToggleTabOrientation),
                MenuItem::separator(),
                MenuItem::action("Dashboard", SelectToolDashboard),
                MenuItem::action("Target", SelectToolTarget),
                MenuItem::action("Proxy", SelectToolProxy),
                MenuItem::action("Intruder", SelectToolIntruder),
                MenuItem::action("Repeater", SelectToolRepeater),
                MenuItem::action("Collaborator", SelectToolCollaborator),
                MenuItem::action("Sequencer", SelectToolSequencer),
                MenuItem::action("Decoder", SelectToolDecoder),
                MenuItem::action("Comparer", SelectToolComparer),
                MenuItem::action("Logger", SelectToolLogger),
                MenuItem::action("Organiser", SelectToolOrganiser),
                MenuItem::action("Extensions", SelectToolExtensions),
                MenuItem::action("Discover", SelectToolDiscover),
            ],
            disabled: false,
        },
        Menu {
            name: "Target".into(),
            items: vec![
                MenuItem::action("添加到范围", AddToScope),
                MenuItem::action("从范围排除", ExcludeFromScope),
                MenuItem::separator(),
                MenuItem::action("目标范围设置…", OpenScopeSettings),
            ],
            disabled: false,
        },
        Menu {
            name: "Proxy".into(),
            items: vec![
                MenuItem::action("打开配置浏览器", OpenConfiguredBrowser),
                MenuItem::action("切换 Intercept 开关", ToggleProxyIntercept),
                MenuItem::separator(),
                MenuItem::action("复制 URL", CopyUrl),
                MenuItem::action("复制为 curl 命令", CopyCurlCommand),
                MenuItem::action("复制响应", CopyResponse),
                MenuItem::separator(),
                MenuItem::action("清空实时历史记录", ClearLiveProxyHistory),
                MenuItem::separator(),
                MenuItem::action("Proxy 设置…", OpenProxySettings),
            ],
            disabled: false,
        },
        Menu {
            name: "Intruder".into(),
            items: vec![
                MenuItem::action("开始攻击", StartIntruderAttack),
                MenuItem::action("打开已保存的攻击", OpenSavedIntruderAttacks),
                MenuItem::action("发起主动扫描 (Scan)", SendToScanner),
                MenuItem::separator(),
                MenuItem::action("发送到 Intruder", OpenInIntruder),
                MenuItem::action("发送到 Repeater", OpenInRepeater),
                MenuItem::separator(),
                MenuItem::action("自动标记 Payload 位置 (§)", AutoMarkIntruderPositions),
                MenuItem::action("清除 Payload 位置标记", ClearIntruderPositions),
                MenuItem::separator(),
                MenuItem::action("关闭攻击结果设置…", OpenIntruderSettings),
            ],
            disabled: false,
        },
        Menu {
            name: "Repeater".into(),
            items: vec![
                MenuItem::action("更新 Content-Length", ToggleRepeaterUpdateContentLength),
                MenuItem::action("解包压缩响应", ToggleRepeaterUnpackCompressed),
                MenuItem::Submenu(Menu {
                    name: "跟随重定向".into(),
                    items: vec![
                        MenuItem::action("从不 (Never)", SetRepeaterRedirectNever),
                        MenuItem::action("仅同站 (On-site only)", SetRepeaterRedirectOnSite),
                        MenuItem::action("仅范围内 (In-scope only)", SetRepeaterRedirectInScope),
                        MenuItem::action("始终 (Always)", SetRepeaterRedirectAlways),
                    ],
                    disabled: false,
                }),
                MenuItem::action("规范化 HTTP/1 换行符", ToggleRepeaterNormalizeLineEndings),
                MenuItem::action("HTTP/1 强制 Connection: close", ToggleRepeaterHttp1Close),
                MenuItem::action(
                    "HTTP/2 自动剥离 Connection 标头",
                    ToggleRepeaterStripH2Connection,
                ),
                MenuItem::action("流式响应与超时设置…", OpenRepeaterSettings),
                MenuItem::separator(),
                MenuItem::action("发送到 Repeater", OpenInRepeater),
                MenuItem::action("新建标签页", NewTab),
                MenuItem::action("复制标签页", DuplicateActiveTab),
                MenuItem::action("发送请求", SendRequest),
                MenuItem::action("取消请求", CancelRequest),
                MenuItem::separator(),
                MenuItem::action("切换请求方法 (GET/POST)", ChangeRequestMethod),
                MenuItem::Submenu(Menu {
                    name: "转换请求体编码".into(),
                    items: vec![
                        MenuItem::action(
                            "循环切换请求体编码 (Toggle body encoding)",
                            ChangeBodyEncodingToggle,
                        ),
                        MenuItem::separator(),
                        MenuItem::action("转为 Form URL-encoded", ChangeBodyEncodingForm),
                        MenuItem::action("转为 Multipart/form-data", ChangeBodyEncodingMultipart),
                        MenuItem::action("转为 Application/JSON", ChangeBodyEncodingJson),
                    ],
                    disabled: false,
                }),
                MenuItem::action("粘贴 URL 为请求", PasteUrlAsRequest),
                MenuItem::separator(),
                MenuItem::action("关闭标签页", CloseTab),
                MenuItem::action("重新打开已关闭标签", RestoreTab),
            ],
            disabled: false,
        },
        Menu {
            name: "窗口".into(),
            items: vec![
                MenuItem::action("最小化", MinimizeWindow),
                MenuItem::action("缩放窗口", ZoomWindow),
                MenuItem::separator(),
                MenuItem::action("切换设置窗口", ToggleSettingsWindow),
                MenuItem::action("前置全部窗口", BringAllWindowsToFront),
            ],
            disabled: false,
        },
        Menu {
            name: "帮助".into(),
            items: vec![
                MenuItem::action("MIO 使用文档", OpenDocumentation),
                MenuItem::action("快捷键列表…", OpenKeyboardShortcuts),
                MenuItem::action("运行系统与代理诊断", RunSystemDiagnostics),
                MenuItem::separator(),
                MenuItem::action("关于 MIO", OpenAboutMio),
            ],
            disabled: false,
        },
    ]);
}

pub fn run(runtime: mio_core::Runtime, client: Client) -> anyhow::Result<()> {
    run_with_project(runtime, client, None)
}

pub(crate) fn fit_window_bounds(
    desired: gpui_kit::Size<gpui_kit::Pixels>,
    cx: &App,
) -> Bounds<gpui_kit::Pixels> {
    let Some(display) = cx.primary_display() else {
        return Bounds::centered(None, desired, cx);
    };
    let visible = display.visible_bounds();
    #[cfg(target_os = "macos")]
    let (top_inset, bottom_inset, side_inset) = (px(0.), px(0.), px(0.));
    #[cfg(not(target_os = "macos"))]
    let (top_inset, bottom_inset, side_inset) = (px(4.), px(4.), px(6.));

    let avail_w = (visible.size.width - side_inset * 2.).max(px(640.));
    let avail_h = (visible.size.height - top_inset - bottom_inset).max(px(480.));
    let fitted = size(desired.width.min(avail_w), desired.height.min(avail_h));
    let origin_x = visible.origin.x + side_inset + (avail_w - fitted.width) * 0.5;
    let origin_y = visible.origin.y + top_inset + (avail_h - fitted.height) * 0.5;
    Bounds {
        origin: gpui_kit::point(origin_x, origin_y),
        size: fitted,
    }
}

pub fn run_with_project(
    runtime: mio_core::Runtime,
    client: Client,
    initial_project: Option<PathBuf>,
) -> anyhow::Result<()> {
    let native = native_terminate::NativeTerminate::install()?;
    let error = Rc::new(RefCell::new(None));
    let startup_error = error.clone();
    let native_for_app = native.clone();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            cx.set_global(native_for_app.clone());
            gpui_kit::init(cx);
            theme::init(cx);
            init_actions(cx);
            let cancel = client.shutdown.clone();
            let joined = cx.background_spawn(async move {
                let result = runtime.join();
                #[cfg(target_os = "macos")]
                objc2::rc::autoreleasepool(|_| {
                    unsafe extern "C-unwind" {
                        #[link_name = "NSLog"]
                        fn ns_log(format: &objc2_foundation::NSString, ...);
                    }
                    let message = if result.is_ok() {
                        objc2_foundation::ns_string!("MIO native-exit joined ok=true")
                    } else {
                        objc2_foundation::ns_string!("MIO native-exit joined ok=false")
                    };
                    // SAFETY: These fixed format strings contain no conversion specifiers.
                    unsafe { ns_log(message) };
                });
                result
            });
            let window_size = if initial_project.is_some() {
                size(px(1280.), px(822. + chrome::CUSTOM_TITLE_BAR_HEIGHT))
            } else {
                size(px(902.), px(610. + chrome::CUSTOM_TITLE_BAR_HEIGHT))
            };
            let bounds = fit_window_bounds(window_size, cx);
            let opened = native_for_app.verify_delegate().and_then(|()| {
                gpui_kit::open_window(
                    chrome::mio_window_options(bounds, true, true),
                    cx,
                    |window, cx| {
                        cx.new(|cx| {
                            workbench::Workbench::new_startup(client, initial_project, window, cx)
                        })
                    },
                )
            });
            match opened {
                Ok((window, view)) => {
                    workbench::Workbench::observe_native(&view, window, cx);
                    workbench::Workbench::observe_runtime(&view, window, joined, cx);
                    cx.activate(true);
                }
                Err(failure) => {
                    eprintln!("MIO 无法打开原生窗口：{failure:#}");
                    *startup_error.borrow_mut() = Some(failure.context("无法打开 MIO 原生窗口"));
                    cancel.cancel();
                    cx.spawn(async move |cx| {
                        if let Err(error) = joined.await {
                            eprintln!("MIO 后台退出失败：{error:#}");
                        }
                        native_for_app.stop_failed_launch();
                        #[cfg(not(target_os = "macos"))]
                        let _ = cx.update(|cx| cx.quit());
                        #[cfg(target_os = "macos")]
                        let _ = cx;
                    })
                    .detach();
                }
            }
        });
    let failure = error.borrow_mut().take();
    failure.map_or(Ok(()), Err)
}
