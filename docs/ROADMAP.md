# MIO 后续演进路线图 (Roadmap)

本文档记录当前已完成基础架构建设、但有意延后实施的工程增强与功能演进事项，作为后续迭代的统一规划参考。

## 1. 发布工程与分发体验
- **代码签名与公证**：为 macOS 产物接入官方开发者签名与系统公证流程，为 Windows 产物接入平台可执行文件代码签名，消除首次运行时的系统安全拦截提示。
- **CI 发布矩阵完善**：`.github/workflows/release-all.yml` 已支持自动构建 `macos-arm64`、`macos-x86_64`、`linux-x86_64`、`windows-x86_64` 与 `windows-aarch64` 压缩包并生成 `SHA256SUMS.txt` 校验清单（命名规范与 `mio-core` 自动更新器一致）；后续补充 `linux-arm64` 自动化 CI 矩阵与 PR 验证流水线。
- **自动更新交互增强**：在现有远端 Release 资产版本检查、`SHA-256` 校验与自替换更新能力基础上，补充下载进度指示、断点重试与升级失败自动回滚保护。

## 2. 报文渲染与离线预览能力（非内嵌浏览器）
- **纯 Rust 静态预览与外部浏览器联动**：遵循架构边界（不内嵌 WebView / 浏览器引擎），持续增强 `Response -> Render` 的纯 Rust 离线静态 HTML/CSS 结构化排版、安全资源预览与编码还原能力；需要完整 DOM/JS 执行时，通过配置的本地代理一键拉起系统外部浏览器进行交互验证。

## 3. 安全扫描与带外检测
- **托管 / 远端 OAST 中继**：扩展 Collaborator 带外探测能力，支持对接可控的远端多协议中继服务端（HTTP/HTTPS/DNS 监听与关联回传），适配内外网隔离及异步延迟触发场景。
- **主 / 被动审计规则深化**：持续丰富内置漏洞指纹库与多步交互式主动扫描策略，进一步提升复杂参数注入、鉴权绕过与业务逻辑缺陷的检出率与低误报率。

## 4. 扩展生态（本地优先）
- **扩展索引与生命周期管理**：在坚持本地显式授权、离线目录及单文件归档加载优先原则的前提下，规划可选的自建静态扩展索引清单支持，以及扩展版本升级、兼容协议检查与完整性校验管理。

## 5. `MioAT` 文案统一与模块持续拆分
- **`MioAT` / CLI 术语统一**：以 `MioAT` 作为 Agent IPC 与助手能力的统一命名，继续统一图形界面各工具视图、`crates/mio-cli` 帮助输出与示例文档中的术语表述。
- **大型 UI 模块持续拆分**：继拆出编辑、插件面板、设置子页等模块后，继续对剩余 `2,000–3,100` 行级 UI 模块按职责拆分为浅层组件（例如 `tabs/render_pane/drawers.rs`、`target/engagement.rs`、`audit_dashboard/workspace.rs`、`settings/connections.rs`、`tools_extra/logger/table.rs`、`inspector/render.rs`、`wb_render_history.rs`、`wb_render_chrome.rs`、`wb_render_top.rs`），进一步降低编译单元体积并改善可维护性。
