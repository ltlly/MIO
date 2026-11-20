# MIO Repository Guidelines

## Generality & Content Neutrality
- **No proprietary plugin references**: Never place names, descriptions, workflows, icons, assets, or documentation of proprietary or third-party external plugins anywhere in tracked source files, comments, UI copy, tests, `AGENTS.md`, git commit messages, or GitHub release notes.
- **No plugin-specific or target-specific code**: Never implement host logic, UI special cases, protocol workarounds, or APIs tailored to a single external plugin, specific target domain/endpoint, or specific header/signing parameter name. Any extension host capability or UI improvement must be generic and broadly reusable across arbitrary plugins.
- **No third-party trademarks or PII**: Do not include third-party product trademarks, personal filesystem paths, credentials, or captured third-party traffic/hosts in repository files, fixtures, or commit history. Tests and fixtures must use neutral synthetic values (`127.0.0.1`, `example.invalid`, `lab.*`).

## Build & Test Conventions
- **Disk hygiene**: Always pass `CARGO_INCREMENTAL=0` when invoking `cargo check`, `cargo test`, or `cargo build`.
- **UI test stack size**: Set `RUST_MIN_STACK=33554432` when running `cargo test -p mio-ui` in unoptimized debug builds.
- **Test isolation**: Isolated core runtimes (`Runtime::start()`, `State::new()`) and UI tests must never read from or mutate the user's real home/config directory (`~`, preferences, or persistent plugin store). Use explicit temp paths or env overrides in tests.

## Extension & Release Guardrails
- **Read-only inspection**: Plugin inspection (`VerifiedPackage::read`) must validate manifests, relative entry paths, archive entries, and digests without executing any plugin binary. Both archive unpacking and persistent installation must reject path traversal, symlinks, and hardlinks.
- **User-level persistent plugins**: Persistent extensions live under `default_plugins_dir()` (`MIO_PLUGINS_DIR` override). `Disable extensions` must suppress automatic loading while still allowing uninstallation.
- **Release & updater compatibility**: Release bundles must include `SHA256SUMS.txt` and platform archives matching the suffixes expected by `crates/mio-core/src/updater.rs` (for example `MIO-v<version>-macos-arm64.zip`).

## Pre-Commit Verification
- Run relevant `mio-core` and `mio-ui` test suites after the final edit.
- Run `git diff --check` and grep across tracked and untracked files to verify zero proprietary plugin names, target-specific domains/parameters, third-party trademarks, or personal local paths exist before committing or tagging.
