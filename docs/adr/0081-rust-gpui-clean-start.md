# ADR-0081：采用 Rust + GPUI，从空白工程开发

- 日期：2026-11-04
- 状态：已实施。
- 补充：[ADR-0082](0082-archive-compatibility-and-quality.md)、[ADR-0083](0083-common-workflows-first.md)。

## 用户决定

采用原生 Rust + GPUI 从空白工程实现 MIO，移除旧 Python/Qt/mitmproxy 代码，不写入原 `main` 工作树。

## 技术决定

1. 原生 UI 采用 GPUI + GPUI Kit，Rust 核心；不再以开发/调试便利自动改回 WebView。
2. GPUI 依赖通过 Kit 固定兼容版本，不混用不同 GPUI 快照。
3. HTTP(S) MITM 起步采用 Rama 0.4 + Rustls/ring；新独立探针补齐了此前缺少的基础修改、H2 与证书校验运行证据。具体 features/API 见 [SDK 决策](../research/rust-capture-decision.md)。
4. Hudsucker 保留为 Rust 回退，不忽略其已知分片、Stop 与路由问题；Go 仅为比较资料，改成双后端属于需要说明的重大变更。
5. 强插件使用原生 Panel SDK + 独立逻辑执行域，不以稳定 dylib ABI、同线程不可信脚本或仅表单替代。
6. 用户要求不继承旧代码、旧数据兼容和额外取证契约；零 Python、中文/英文专名、垂直工作标签、不做内嵌浏览器继续有效。

## 代价与风险

GPUI pre-1.0 升级、真实 IME、复杂编辑器、插件原生面板协议与跨平台打包是需要付出的工程成本。
Rama 的 Rustls 路径不等同其 BoringSSL 成对 relay；主动连接停止、WS/SSE、上游与协议异常仍需真实产品测试。
选择该方向体现用户的产品偏好及新增证据，不表示已经证明性能优于 WebView 或所有协议可用。

## 验收与交接

[当前路线](../native-rewrite-plan.md)定义模块边界，[ROADMAP](../../ROADMAP.md)维护模块状态，[当前状态](../current-status.md)记录实测命令与证据。
