#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"

build_profile="release"
build_flags=(--release)
rust_target="aarch64-pc-windows-msvc"
target_dir="$project_root/target/xwin"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)
      build_profile="debug"
      build_flags=()
      shift
      ;;
    --release)
      build_profile="release"
      build_flags=(--release)
      shift
      ;;
    --target)
      rust_target="${2:?missing target after --target}"
      shift 2
      ;;
    *)
      echo "Usage: bash scripts/package-windows.sh [--debug|--release] [--target <windows-rust-target>]" >&2
      exit 2
      ;;
  esac
done

source_commit="$(git rev-parse HEAD)"
source_branch="$(git symbolic-ref --quiet --short HEAD || printf 'detached')"
source_dirty=false
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  source_dirty=true
fi

if [[ "$rust_target" == *-msvc ]]; then
  if [[ -n "${MIO_LLVM_BIN_DIR:-}" ]]; then
    export PATH="$MIO_LLVM_BIN_DIR:$PATH"
  fi
  if [[ -d "$HOME/.cargo/bin" ]] && [[ ":$PATH:" != *":$HOME/.cargo/bin:"* ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
  cross_compiler="${XWIN_CROSS_COMPILER:-clang}"
  if ! command -v "$cross_compiler" >/dev/null 2>&1; then
    echo "Error: cross compiler '$cross_compiler' not found on PATH (set MIO_LLVM_BIN_DIR or XWIN_CROSS_COMPILER)." >&2
    exit 1
  fi
  export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
  XWIN_CROSS_COMPILER="$cross_compiler" cargo xwin build --locked -p mio-app -p mio-cli --bins --target "$rust_target" --target-dir "$target_dir" "${build_flags[@]}"
  bin_dir="$target_dir/$rust_target/$build_profile"
else
  cargo zigbuild --locked -p mio-app -p mio-cli --bins --target "$rust_target" "${build_flags[@]}"
  bin_dir="$project_root/target/$rust_target/$build_profile"
fi

if [[ "$(git rev-parse HEAD)" != "$source_commit" ]]; then
  echo "Source commit changed during the build; distribution was not updated." >&2
  exit 1
fi
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  source_dirty=true
fi

arch="${rust_target%%-*}"
dist_dir="$project_root/dist/windows-$arch"
mkdir -p "$dist_dir"
install -m 755 "$bin_dir/mio.exe" "$dist_dir/mio.exe"
install -m 755 "$bin_dir/mio-cli.exe" "$dist_dir/mio-cli.exe"

if command -v sha256sum >/dev/null 2>&1; then
  bin_sha="$(sha256sum "$dist_dir/mio.exe" | awk '{print $1}')"
else
  bin_sha="$(/usr/bin/shasum -a 256 "$dist_dir/mio.exe" | awk '{print $1}')"
fi

packaged_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
manifest="$(mktemp "$dist_dir/.preview-build.XXXXXX")"
cleanup() {
  if [[ -e "$manifest" ]]; then unlink "$manifest"; fi
}
trap cleanup EXIT
MIO_PKG_ARCH="$arch" \
MIO_PKG_PROFILE="$build_profile" \
MIO_PKG_BRANCH="$source_branch" \
MIO_PKG_COMMIT="$source_commit" \
MIO_PKG_DIRTY="$source_dirty" \
MIO_PKG_SHA256="$bin_sha" \
MIO_PKG_PACKAGED_AT="$packaged_at" \
python3 - "$manifest" <<'PY'
import json
import os
import sys

payload = {
    "app": "mio.exe",
    "platform": "windows",
    "architecture": os.environ["MIO_PKG_ARCH"],
    "profile": os.environ["MIO_PKG_PROFILE"],
    "branch": os.environ["MIO_PKG_BRANCH"],
    "sourceCommit": os.environ["MIO_PKG_COMMIT"],
    "sourceDirty": os.environ["MIO_PKG_DIRTY"] == "true",
    "binarySha256": os.environ["MIO_PKG_SHA256"],
    "packagedAt": os.environ["MIO_PKG_PACKAGED_AT"],
}
with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump(payload, fh, indent=2)
    fh.write("\n")
PY
mv "$manifest" "$dist_dir/preview-build.json"

echo "$dist_dir"
