# 固定依赖补丁

`gpui-base/` 是已锁定的 `gpui-base 0.9.0` crates.io 发布包，保留其完整源码、测试、Cargo 清单和 Apache-2.0 许可证。发布包 SHA-256 为 `cff6377e30f0c857681662e7edc7286bf30f5606033390401b0706d7bd930833`，与引入前的根 Cargo.lock 一致；没有修改 Cargo registry 缓存或升级依赖。

本地输入补丁用于同一次原生输入编辑携带多个 inline token，让 Intruder 的成对位置标记、Clear 和 Auto 使用组件自身的撤销/重做历史。新增 `InputState` / `TextareaState::replace_range_with_content`，沿用现有输入校验、Unicode/IME 边界和 `EditIntent::Atomic`；只读 `document_revision` 暴露已有同步编辑世代，让异步 Auto 结果不会覆盖后续输入。不另建文本历史，不为 Editor 模式启用 token。

`Radio`、`Checkbox` 与 `Switch` 的焦点补丁沿用该版本 `Button` 的处理：已聚焦控件因异步保存而禁用时，保持当前焦点在分派树中，但不作为新的 Tab 停靠点，避免焦点在等待 ACK 期间丢失 Escape／Tab 键盘路由。搜索补丁支持受限正则匹配（`SearchMatcher::update_regex_query`）、IME 组词同步（`has_marked_text` / `update_search`），并通过轻量 `RopeReader` 直接流式遍历 `Rope` 分块完成字面量搜索，避免每次搜索更新时将整份报文分配为连续 `String`。

补丁回归放在 `mio-ui` 测试中，随项目统一测试入口运行。此目录不是新的产品 workspace 成员；没有借依赖补丁开发其它操作系统。后续升级需先确认上游等价能力及全部标记回归，再移除本地 patch。

`gpui-component/` 同样固定为 crates.io 发布的 `gpui-component 0.9.0`，发布包 SHA-256 为 `45bbfdba0efa392190f2fd446ec311325302eb047d5ce45dc1bcf189cb18f5eb`，与引入前根 Cargo.lock 一致。保留发布包源码、测试、清单、build.rs、locales 和 Apache-2.0 许可证，不包含 Cargo 缓存标记；不修改 registry 缓存或升级其它依赖。

TableDelegate 新增默认关闭的 `row_selection_managed_by_delegate()`。仅显式接管的表格由既有 `render_tr` 提供每行 `aria_selected` 及 hover／选中配色，避免 Kit 将业务多选覆盖为单选、抹掉高亮颜色或重复设置 hover。行角色、行索引、主行的活动焦点、输入处理和虚拟化保持原路径；其它表格保持默认选择行为。最终行为由 HTTP History 的真实事件与原生视觉回归验证，不将此开关扩成另一套表格框架。

`TableState::set_selected_row_preserving_context` 仅供业务右键选择同步主行时使用，沿用原 setter 的选择、滚动与通知，但恢复原右键行／单元格目标；否则主行同步会清掉尚待显示的 Highlight 菜单。普通 setter 不变，不额外发出右键事件；真实右键菜单回归留在 `mio-ui`。

此外，`gpui-component` 的单行 `Input`、`Size::Size(_)` 与 `TableState` 单元格/表头行高补丁修正了高/低字号缩放时的垂直内边距与垂直居中裁切问题。

组件最小回归在 `crates/mio-ui/tests/table_row_selection.rs`，通过 Kit facade 从实际点击／鼠标移动读取普通单选与委托多选的原生 ARIA，带自定义 hover 的表必须正常绘制，随产品统一测试入口执行。像素颜色仍需产品原生验证；vendor crate 本身继续排除于产品 workspace。

`gpui-pre-windows/` 固定为 crates.io 发布的 `gpui-pre-windows 0.3.11`，发布包 SHA-256 为 `5141a3db59bab761a16ab19529fe4117a86e91922e8fc00af6dfc4829cb1744d`，与引入前根 Cargo.lock 一致。上游在未运行 Windows SDK `fxc.exe`（例如 Debug 构建、未装 Windows SDK 的 Windows 环境或跨平台构建）时，运行时调用 `D3DCompileFromFile(env!("CARGO_MANIFEST_DIR")/src/*.hlsl)`，导致已分发二进制在目标 Windows 机器上因找不到编译机源码路径而在 DirectX/DirectWrite 初始化阶段崩溃（`os error 3`）。本地补丁改为在无 `fxc.exe` 预编译产物时使用编译期内嵌 HLSL 源码配合系统 `d3dcompiler_47.dll` 的内存级 `D3DCompile`（Release 启用 `D3DCOMPILE_OPTIMIZATION_LEVEL3`），消除编译机本地路径依赖。

`gpui-pre-linux/` 固定为 crates.io 发布的 `gpui-pre-linux 0.3.11`，发布包 SHA-256 为 `79cac5cbd3485c1e3db8eafd5dc69019188f5cb0a39846946af772efa386f624`，与引入前根 Cargo.lock 一致。上游 X11 后端仅在检测到独立合成器时才允许 `_MOTIF_WM_HINTS` 隐藏系统标题栏；MIO 自绘标题栏使用不透明矩形边框（零阴影外扩 inset），本地补丁允许支持 `_GTK_FRAME_EXTENTS` 的 X11 窗口管理器（如未开合成器的 XFCE4）同样启用客户端自绘标题栏，保证跨平台标题栏一致。
