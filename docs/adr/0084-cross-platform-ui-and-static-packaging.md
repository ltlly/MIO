# ADR-0084：跨平台三端兼容、MIO Light 视觉统一与静态内联分发

- 状态：已实施
- 日期：2026-11-15

## 背景

在完成桌面端核心能力闭环后，项目确立以下跨平台与分发演进目标：

1. 原生支持 **macOS、Windows、Linux** 三大桌面系统（覆盖 `arm64` 与 `x86_64`），禁止强制依赖外部构建环境，能够在本地主力机或标准交叉编译工具链上完成构建与真机 UI 验收。
2. 三端统一采用自绘标题栏与品牌标识（**MIO — MITM Inspection & Orchestration**，蓝白条纹碗图标），建立浅色优先的 **MIO Light** 视觉规范，消除无边框按钮、强调色混杂及口语化提示。
3. 保证根证书（`MIO local CA`）按操作系统用户持久化复用，提供 `http://mio/cert` 代理下载门户，并确保网络协议栈细节与设置项实时生效行为一致。
4. 扩展 `mio-cli` 支持独立上下文 AI Agent 完成实时拦截决策、主动 HTTP 重放与工作区配置编排，且发布的 Release 产物必须内联第三方运行时依赖。

## 决策

1. **跨平台窗口与 MIO Light 设计系统**：
   - 启用客户端自绘窗口装饰（Client-Side Decorations），在 macOS、Windows、Linux 上统一渲染 MIO 自定义标题栏与蓝白碗图标。
   - 默认采用浅色 **MIO Light** 主题：主强调色统一为 `#DB612F`，按钮与输入控件统一 `1px` `#B5BCC7` 描边与 `4px`/`6px` 圆角，全套工具与高频动作按钮配备 Lucide 图标与简洁准确的悬浮说明。
2. **持久化 CA 与代理门户**：
   - `authority.json` 固定存储于用户配置目录（macOS `~/Library/Application Support/MIO/identity/`、Linux `${XDG_DATA_HOME:-~/.local/share}/mio/identity/`、Windows `%APPDATA%\MIO\identity\`），20 年根证书跨项目持久复用；代理端口内置 `http://mio/cert` / `http://mio/cert.pem` 下载门户。
3. **跨平台静态内联打包**：
   - Linux 通过 `cargo-zigbuild` 配合静态归档组内联 SQLite、`libxcb`、`libxcb-xkb`、`libxkbcommon`、`libxkbcommon-x11`、`libXau`、`libXdmcp`、`libbsd`、`libmd`，使 ELF `DT_NEEDED` 仅依赖系统基础 `libc.so.6` / `libm.so.6` / `ld-linux`（`glibc >= 2.34`，Wayland/GPU/Fontconfig 由桌面图形栈运行时按需 `dlopen`）。
   - Windows 通过 `cargo-xwin` 配合 `-C target-feature=+crt-static` 静态内联 MSVC CRT，消除 `VCRUNTIME140.dll` 依赖。
   - macOS 通过 `scripts/package-macos.sh` 生成带标准化 `Applications -> /Applications` 拖拽链接的 `.dmg` 与 `.zip` 签名产物。
4. **多平台实机与虚拟机验证**：
   - 在 macOS 原生桌面及 Ubuntu 24.04 ARM64 / Windows 11 ARM64 本地 `UTM.app` 持久化虚拟机环境下安装系统级根证书，通过真实桌面 UI 交互与全协议抓包完成验证。
