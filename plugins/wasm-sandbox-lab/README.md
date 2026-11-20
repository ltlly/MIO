# Legacy WASM Fixture (`lab.wasm`)

WASM plugin sandboxing is **out of scope and unmaintained** (security isolation is not a design goal of MIO; Native worker plugins are the sole supported extension architecture).

`manifest.json` and `sandbox.wasm` in this directory are retained **only** as legacy test fixtures for `crates/mio-core/src/plugins/wasm/tests_envelope_worker.rs`. For active plugin development, see [docs/plugin-api.md](../../docs/plugin-api.md) and [plugins/protocol-lab/](../protocol-lab/README.md).
