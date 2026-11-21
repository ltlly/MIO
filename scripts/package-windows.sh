#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"

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
  export PATH="/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$HOME/.cargo/bin:$PATH"
  export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
  XWIN_CROSS_COMPILER="${XWIN_CROSS_COMPILER:-clang}" cargo xwin build --locked -p mio-app -p mio-cli --bins --target "$rust_target" --target-dir "$target_dir" "${build_flags[@]}"
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

cat > "$dist_dir/preview-build.json" <<EOF
{
  "app": "mio.exe",
  "platform": "windows",
  "architecture": "$arch",
  "profile": "$build_profile",
  "branch": "$source_branch",
  "sourceCommit": "$source_commit",
  "sourceDirty": $source_dirty,
  "binarySha256": "$bin_sha",
  "packagedAt": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

echo "$dist_dir"
