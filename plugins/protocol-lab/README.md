# Protocol Lab (`lab.protocol`)

Reference Native worker plugin (`crates/mio-plugin-api` protocol `v1.3`) implementing all 11 supported extension contributions and all 10 `HostCall` operations.

## Source Map

| File | Responsibility |
|---|---|
| [Cargo.toml](Cargo.toml) / [manifest.json](manifest.json) | Native binary `protocol-lab` (`entry: "bin/protocol-lab"`), all 8 permissions, all 11 contributions |
| [src/main.rs](src/main.rs) | `WorkerSession::accept` handshake, main envelope dispatch, `HostCall` roundtrips (`PendingService`) |
| [src/lab_state.rs](src/lab_state.rs) | Core state (`Lab`), `OpenRequest`/`OpenResponse`/`CloseRequest`/`CommitEditor`, KV & preferences state |
| [src/lab_hooks.rs](src/lab_hooks.rs) | `http_hook`, `ws_hook`, `session_action`, `payload_generator`, `payload_processor`, `context_menu`, `passive_check`, `active_check` |
| [src/lab_panel.rs](src/lab_panel.rs) | Declarative UI snapshot & patch builder (`Split`, `Tabs`, `VirtualTable`, `Editor`, `Canvas` + drag hit regions, `Checkbox`, `Select`, `Badge`, `Progress`) |
| [src/lab_wires.rs](src/lab_wires.rs) | Wire serialization & Base64 / UTF-8 / Binary codec helpers |
| [src/tests_state.rs](src/tests_state.rs) / [src/tests_hooks.rs](src/tests_hooks.rs) / `tests/` | Unit and real-process integration suites |

## Build, Check, Load & Test

```sh
# 1. Build standalone package directory (outside git)
CARGO_TARGET_DIR=target cargo build --release -p mio-protocol-lab --bin protocol-lab
mkdir -p dist/protocol-lab/bin
cp plugins/protocol-lab/manifest.json dist/protocol-lab/manifest.json
cp target/release/protocol-lab dist/protocol-lab/bin/protocol-lab

# 2. Validate package via mio_plugin_cli
cargo run --release --locked -p mio-core --example mio_plugin_cli -- check dist/protocol-lab

# 3. Load in MIO GUI: Extensions -> enter dist/protocol-lab -> 检查插件包 -> 确认并加载

# 4. Run protocol & worker test suites
cargo test -p mio-plugin-api -p mio-protocol-lab
cargo test -p mio-core plugins::tests
```

Protocol details & envelope schemas: see [docs/plugin-api.md](../../docs/plugin-api.md).
