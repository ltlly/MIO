#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"

build_profile="release"
build_flags=(--release)
target_spec=""

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
      target_spec="${2:?missing target after --target}"
      shift 2
      ;;
    *)
      echo "Usage: bash scripts/package-linux.sh [--debug|--release] [--target <rust-or-zigbuild-target>]" >&2
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

host_os="$(uname -s)"
resolve_static_lib_dir() {
  local target_arch="$1"
  if [[ -n "${MIO_LINUX_STATIC_LIB_DIR:-}" ]]; then
    printf '%s' "$MIO_LINUX_STATIC_LIB_DIR"
    return
  fi
  if [[ -d "$project_root/.local/linux-static-libs/$target_arch" ]]; then
    printf '%s' "$project_root/.local/linux-static-libs/$target_arch"
    return
  fi
  if [[ "$host_os" == "Linux" && -f "/usr/lib/${target_arch}-linux-gnu/libxkbcommon-x11.a" ]]; then
    printf '%s' "/usr/lib/${target_arch}-linux-gnu"
    return
  fi
  printf '%s' "$project_root/.local/linux-static-libs/$target_arch"
}

require_static_libs() {
  local dir="$1"
  local missing=()
  for lib in libxkbcommon-x11.a libxkbcommon.a libxcb-xkb.a libxcb.a libXau.a libXdmcp.a libbsd.a libmd.a; do
    if [[ ! -f "$dir/$lib" ]]; then
      missing+=("$lib")
    fi
  done
  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "Error: missing required Linux static archives in '$dir': ${missing[*]}" >&2
    echo "Populate .local/linux-static-libs/<arch>/ (or set MIO_LINUX_STATIC_LIB_DIR) before packaging." >&2
    exit 1
  fi
}

ensure_isoc23_compat() {
  local target_arch="$1"
  local out_archive="$2"
  if [[ -f "$out_archive" ]]; then
    return
  fi
  local tmp_dir
  tmp_dir="$(mktemp -d)"
  cat > "$tmp_dir/isoc23_compat.c" <<'EOF'
#include <inttypes.h>
#include <stdlib.h>
long __isoc23_strtol(const char *s, char **e, int b) { return strtol(s, e, b); }
unsigned long __isoc23_strtoul(const char *s, char **e, int b) { return strtoul(s, e, b); }
intmax_t __isoc23_strtoimax(const char *s, char **e, int b) { return strtoimax(s, e, b); }
uintmax_t __isoc23_strtoumax(const char *s, char **e, int b) { return strtoumax(s, e, b); }
EOF
  zig cc -target "${target_arch}-linux-gnu.2.34" -O2 -fPIC -c "$tmp_dir/isoc23_compat.c" -o "$tmp_dir/isoc23_compat.o"
  zig ar rcs "$out_archive" "$tmp_dir/isoc23_compat.o"
  unlink "$tmp_dir/isoc23_compat.c"
  unlink "$tmp_dir/isoc23_compat.o"
  rmdir "$tmp_dir"
}

prepare_static_link_dir() {
  local src_dir="$1"
  local target_arch="$2"
  local use_isoc23="$3"
  local link_dir="$project_root/.local/linux-static-link/$target_arch"
  mkdir -p "$link_dir"
  for lib in libxkbcommon-x11.a libxkbcommon.a libxcb-xkb.a libxcb.a libXau.a libXdmcp.a libbsd.a libmd.a; do
    ln -sf "$src_dir/$lib" "$link_dir/$lib"
  done
  local compat_entry=""
  if [[ "$use_isoc23" == "true" ]]; then
    ensure_isoc23_compat "$target_arch" "$link_dir/libisoc23_compat.a"
    compat_entry="libisoc23_compat.a"
  fi
  printf 'INPUT(libxcb.a libXau.a libXdmcp.a libbsd.a libmd.a)\n' > "$link_dir/libxcb.so"
  printf 'INPUT(libxkbcommon.a %s)\n' "$compat_entry" > "$link_dir/libxkbcommon.so"
  printf 'INPUT(libxkbcommon-x11.a libxkbcommon.a libxcb-xkb.a libxcb.a libXau.a libXdmcp.a libbsd.a libmd.a %s)\n' "$compat_entry" > "$link_dir/libxkbcommon-x11.so"
  printf '%s' "$link_dir"
}

if [[ "$host_os" == "Linux" && -z "$target_spec" ]]; then
  arch="$(uname -m)"
  case "$arch" in
    arm64) arch="aarch64" ;;
    amd64) arch="x86_64" ;;
  esac
  static_lib_dir="$(resolve_static_lib_dir "$arch")"
  require_static_libs "$static_lib_dir"
  link_dir="$(prepare_static_link_dir "$static_lib_dir" "$arch" "false")"
  export RUSTFLAGS="${RUSTFLAGS:-} -L native=$link_dir -C link-arg=-Wl,--start-group -C link-arg=$link_dir/libxkbcommon-x11.a -C link-arg=$link_dir/libxkbcommon.a -C link-arg=$link_dir/libxcb-xkb.a -C link-arg=$link_dir/libxcb.a -C link-arg=$link_dir/libXau.a -C link-arg=$link_dir/libXdmcp.a -C link-arg=$link_dir/libbsd.a -C link-arg=$link_dir/libmd.a -C link-arg=-Wl,--end-group"
  RUST_FONTCONFIG_DLOPEN=1 cargo build --locked -p mio-app -p mio-cli --bins "${build_flags[@]}"
  bin_dir="$project_root/target/$build_profile"
else
  if [[ -z "$target_spec" ]]; then
    target_spec="aarch64-unknown-linux-gnu.2.34"
  fi
  rust_target="${target_spec%%.*}"
  arch="${rust_target%%-*}"
  static_lib_dir="$(resolve_static_lib_dir "$arch")"
  require_static_libs "$static_lib_dir"
  link_dir="$(prepare_static_link_dir "$static_lib_dir" "$arch" "true")"
  export RUSTFLAGS="${RUSTFLAGS:-} -L native=$link_dir -C link-arg=-Wl,--start-group -C link-arg=$link_dir/libxkbcommon-x11.a -C link-arg=$link_dir/libxkbcommon.a -C link-arg=$link_dir/libxcb-xkb.a -C link-arg=$link_dir/libxcb.a -C link-arg=$link_dir/libXau.a -C link-arg=$link_dir/libXdmcp.a -C link-arg=$link_dir/libbsd.a -C link-arg=$link_dir/libmd.a -C link-arg=$link_dir/libisoc23_compat.a -C link-arg=-Wl,--end-group"
  RUST_FONTCONFIG_DLOPEN=1 cargo zigbuild --locked -p mio-app -p mio-cli --bins --target "$target_spec" "${build_flags[@]}"
  bin_dir="$project_root/target/$rust_target/$build_profile"
fi

if [[ "$(git rev-parse HEAD)" != "$source_commit" ]]; then
  echo "Source commit changed during the build; distribution was not updated." >&2
  exit 1
fi
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  source_dirty=true
fi

dist_dir="$project_root/dist/linux-$arch"
mkdir -p "$dist_dir/bin" "$dist_dir/share/applications" \
  "$dist_dir/share/icons/hicolor/64x64/apps" \
  "$dist_dir/share/icons/hicolor/256x256/apps"
install -m 755 "$bin_dir/mio" "$dist_dir/bin/mio"
install -m 755 "$bin_dir/mio-cli" "$dist_dir/bin/mio-cli"
install -m 644 "$project_root/crates/mio-app/assets/mio-icon-64.png" "$dist_dir/share/icons/hicolor/64x64/apps/mio.png"
install -m 644 "$project_root/crates/mio-app/assets/mio-icon-256.png" "$dist_dir/share/icons/hicolor/256x256/apps/mio.png"

cat > "$dist_dir/share/applications/mio.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=MIO
Comment=Native Rust + GPUI Web Security Workbench
Exec=mio
Icon=mio
Terminal=false
Categories=Development;Security;Network;
StartupWMClass=mio
EOF

if command -v sha256sum >/dev/null 2>&1; then
  bin_sha="$(sha256sum "$dist_dir/bin/mio" | awk '{print $1}')"
else
  bin_sha="$(/usr/bin/shasum -a 256 "$dist_dir/bin/mio" | awk '{print $1}')"
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
    "app": "mio",
    "platform": "linux",
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
