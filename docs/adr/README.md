# 新版 MIO 决策记录

以下为历史架构决策记录（历史上下文快照）；现行模块规范、协议细节及已知限制以 [architecture.md](../architecture.md)、[plugin-api.md](../plugin-api.md) 与 [scripting-api.md](../scripting-api.md) 为准。

- [0081 — 采用 Rust + GPUI 从零实现](0081-rust-gpui-clean-start.md)：确立纯 Rust + GPUI + Rama 0.4 技术栈。
- [0082 — 外部项目归档兼容与质量硬约束](0082-archive-compatibility-and-quality.md)：确立外部二进制归档只读转换、原生 UI 逐像素精细度、同机性能与 ≤10 分钟全量测试预算（其中早期单平台限制已由 ADR-0084 扩展至三端）。
- [0083 — 常用工作流优先的交付顺序](0083-common-workflows-first.md)：确立核心三工具 → 辅助工具 → 兼容与质量矩阵 → 强插件平台的执行顺序。
- [0084 — 跨平台三端兼容、MIO Light 视觉统一与静态内联分发](0084-cross-platform-ui-and-static-packaging.md)：确立 macOS / Windows / Linux 六目标本地构建、自绘标题栏与 MIO Light 设计系统、持久化 CA 及全功能 `mio-cli`。
