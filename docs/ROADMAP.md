# MIO 演进路线图 (Roadmap)

本文档记录基础架构完成后的各项工程增强与功能演进交付状态。

## 1. 插件宿主与协议演进（解决 `512 KiB` / `1 MiB` 限制）
- [x] **零改插件的宿主侧 Hook 预过滤**：在扩展配置与设置面板中增加了按插件粒度的触发范围筛选（仅限 Scope、Host/Path 白黑名单、请求/响应阶段开关及静态资源跳过），在宿主机内存阶段直接短路无关流量，避免对全量流量做无意义的 Protobuf IPC 往返。
- [x] **可观测性与 Fail-Open 降级开关**：在扩展状态与诊断视图中统计展示“因超限旁路 / 过滤跳过 / 超时或错误次数”，并支持按插件配置 **Fail-Open**（插件崩溃或 `5s` 超时时记录诊断并自动放行流量）与 **Fail-Closed** 策略。
- [x] **协议 `v1.7+` 大报文支持**：为需要检查或改写 `> 512 KiB` 正文/响应的插件补充 `body_omitted`（仅传元数据与 HTTP Header）、`original_body_len` 及只读共享临时文件（`body_file_path`）机制，并保持 `v1.6`/`v1.5` 向下兼容。

## 2. 发布工程与分发体验
- [x] **代码签名与公证**：`scripts/package-macos.sh` 支持基于环境变量的 Apple Developer ID 签名、公证与 `stapler` 装订（未配置凭据时自动回退 ad-hoc 签名）；`scripts/package-windows.ps1` 支持 Authenticode `signtool` 签名；`.github/workflows/release-all.yml` 已接入可选签名/公证 secrets。
- [x] **自动更新交互增强**：在现有 Release 检查、`SHA256SUMS.txt` 校验与自替换更新器基础上，补充下载进度事件与 UI 进度条展示、HTTP `Range` 断点续传/重试与升级失败备份回滚保护。

## 3. 报文渲染、带外检测与 UI 模块化
- [x] **纯 Rust 静态预览**：遵循不内嵌 WebView 边界，增强 `Response -> Render` 的纯 Rust 离线静态 HTML/CSS 排版（表格、代码块、图片清单、样式表与安全色板预览）与格式化 XML 树预览。
- [x] **托管 / 远端 OAST 中继**：扩展 Collaborator 支持托管/自建远端 OAST HTTP/HTTPS/DNS 中继（含认证令牌、远端轮询关联与 `OastMode` 配置）。
- [x] **主 / 被动审计规则深化**：丰富内置被动漏洞指纹（PEM 私钥、GraphQL 自省、序列化对象、DOM XSS 特征、缺 CSRF 令牌 POST 表单、文件上传入口、HSTS `max-age=0`、空签名 JWT、Host 覆盖反射）与多步主动检测策略（布尔条件 SQLi 差分、路径遍历、SSTI 算术求值、Host 头覆盖探针、移除认证凭据越权对照）。
- [x] **大型 UI 模块拆分**：已将全部 10 个超过 `2,000` 行的界面文件（`tabs/render_pane/drawers.rs`、`settings/connections.rs`、`tools_extra/logger/table.rs`、`inspector/render.rs`、`wb_render_top.rs`、`tools_extra/logger/render.rs`、`wb_render_history.rs`、`wb_render_chrome.rs`、`editing/hex_grid.rs`、`tabs/render_pane.rs`）按职责拆分为浅层子模块，当前 `crates/mio-ui/src/` 下已无超过 `2,000` 行的源文件。

## 4. `mio-cli` 高频工作流增强（HTTP 抓包历史 & Repeater）与体验打磨
- [x] **HTTP 抓包历史（`flows`）序号与筛选增强**：
  - `flows list`（人类可读输出与 `--json`）暴露与 GUI 表格一致的自增流量序号，且 `flows get`、`repeater open --flow`、`repeater send --flow` 支持按数字序号或 UUID 寻址。
  - `flows list` 补充 `--hide-connect`、`--host`、`--source proxy|repeater|...`、`--state`（含 `abnormal` 快速筛选失败/丢弃/取消流量）及 `--order asc|desc`，并在文本输出中标示异常状态。
  - `flows get` 支持 `--request-only` / `--response-only` / `--headers-only` 与 `--save-body <path>`。
- [x] **Repeater（`repeater`）标签页管理与输入便利性**：
  - 补齐 `repeater list`、`repeater rename`、`repeater close`，并支持 `repeater send --tab <tab-id>` 直接向已有标签页发包并回写最近一次结果。
  - `repeater open` / `send` 支持 `--raw-file <path>`（含 stdin `-`）与 `--body-file <path>`（支持任意二进制字节），并支持 `--blank` 空白标签与仅提供 `--target` 生成默认 GET 模板标签页。
  - 修正 `repeater send` 携带头/体覆盖参数时的变更链来源标记为 `ProvenanceActor::CliRepeaterOverride`，不再复用 `ProvenanceActor::ManualInterceptEdit`。
- [x] **无头拦截、工作区与基础 CLI 细节**：
  - 修复无头 `mio-cli serve` 开启 `proxy intercept --responses on` 时默认响应规则集未启用导致普通响应未进入 `proxy pending` 的问题。
  - 强化 `workspace set` 的乐观锁 `revision` 校验与清晰冲突提示（包含当前服务端版本与应递增版本号）。
  - 补充 `--version` / `-V` 版本输出；无头 `serve --listen` / `proxy start` 端口冲突错误提示改用 CLI `--listen` 参数指引；`status` 与 `slim-save` 非 `--json` 输出附带 `KiB` / `MiB` / `GiB` 可读单位。
