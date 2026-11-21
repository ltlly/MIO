#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
if [[ "$(uname -s)" != Darwin ]]; then
  echo "package-macos.sh must be run on macOS (use package-linux.sh or package-windows.ps1/sh for Linux/Windows)." >&2
  exit 1
fi
build_profile="release"
build_flags=(--release)
rust_target=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug) build_profile=debug; build_flags=(); shift ;;
    --release) build_profile=release; build_flags=(--release); shift ;;
    --target) rust_target="${2:?missing target after --target}"; shift 2 ;;
    *) echo "Usage: bash scripts/package-macos.sh [--debug|--release] [--target <apple-darwin-target>]" >&2; exit 2 ;;
  esac
done

source_commit="$(git rev-parse HEAD)"
source_branch="$(git symbolic-ref --quiet --short HEAD || printf 'detached')"
source_dirty=false
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  source_dirty=true
fi
if [[ -n "$rust_target" ]]; then
  MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}" cargo build --locked -p mio-app -p mio-cli --bins --target "$rust_target" "${build_flags[@]}"
  bin_dir="$project_root/target/$rust_target/$build_profile"
  arch="${rust_target%%-*}"
  if [[ "$arch" == "aarch64" ]]; then
    arch="arm64"
  fi
  distribution_dir="$project_root/dist/macos-$arch"
else
  MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}" cargo build --locked -p mio-app -p mio-cli --bins "${build_flags[@]}"
  bin_dir="$project_root/target/$build_profile"
  arch="$(uname -m)"
  if [[ "$arch" == "aarch64" ]]; then
    arch="arm64"
  fi
  distribution_dir="$project_root/dist"
fi
version="$(awk '/^\[workspace\.package\]/{f=1; next} /^\[/{f=0} f && /^version *= *"/{sub(/^version *= *"/,""); sub(/".*/,""); print; exit}' Cargo.toml)"
if [[ -z "$version" ]]; then
  echo "Failed to determine workspace version from Cargo.toml" >&2
  exit 1
fi
if [[ "$(git rev-parse HEAD)" != "$source_commit" ]]; then
  echo "Source commit changed during the build; distribution was not updated." >&2
  exit 1
fi
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  source_dirty=true
fi
app_bundle="$bin_dir/MIO.app"
for bin_name in mio mio-cli; do
  if [[ ! -s "$bin_dir/$bin_name" ]]; then
    echo "Built binary is missing or empty: $bin_dir/$bin_name" >&2
    exit 1
  fi
  bin_arch="$(/usr/bin/lipo -archs "$bin_dir/$bin_name")"
  if [[ -z "$bin_arch" ]]; then
    echo "Built binary is not a valid Mach-O executable: $bin_dir/$bin_name" >&2
    exit 1
  fi
done
mkdir -p "$app_bundle/Contents/MacOS" "$app_bundle/Contents/Resources"
install -m 755 "$bin_dir/mio" "$app_bundle/Contents/MacOS/mio"
install -m 755 "$bin_dir/mio-cli" "$app_bundle/Contents/MacOS/mio-cli"
install -m 644 crates/mio-app/Info.plist "$app_bundle/Contents/Info.plist"
install -m 644 crates/mio-app/assets/AppIcon.icns "$app_bundle/Contents/Resources/AppIcon.icns"
/usr/bin/plutil -lint "$app_bundle/Contents/Info.plist"
/usr/bin/codesign --force --sign - "$app_bundle/Contents/MacOS/mio-cli"
/usr/bin/codesign --force --sign - "$app_bundle"
/usr/bin/codesign --verify --strict "$app_bundle"
distribution_bundle="$distribution_dir/MIO.app"
mkdir -p "$distribution_dir"
/usr/bin/ditto "$app_bundle" "$distribution_bundle"
install -m 755 "$bin_dir/mio-cli" "$distribution_dir/mio-cli"
/usr/bin/codesign --verify --strict "$distribution_bundle"
binary_arch="$(/usr/bin/lipo -archs "$distribution_bundle/Contents/MacOS/mio")"
if [[ -z "$binary_arch" ]]; then
  echo "Packaged binary is not a valid Mach-O executable: $distribution_bundle/Contents/MacOS/mio" >&2
  exit 1
fi
binary_sha256="$(/usr/bin/shasum -a 256 "$distribution_bundle/Contents/MacOS/mio" | awk '{print $1}')"
dmg_name="MIO-v${version}-macos-${arch}.dmg"
zip_name="MIO-v${version}-macos-${arch}.zip"
dmg_path="$distribution_dir/$dmg_name"
zip_path="$distribution_dir/$zip_name"
rm -f "$dmg_path" "$zip_path"
dmg_stage="$(mktemp -d "${TMPDIR:-/tmp}/mio-dmg.XXXXXX")"
manifest="$(mktemp "$distribution_dir/.preview-build.XXXXXX")"
cleanup() {
  if [[ -e "$manifest" ]]; then unlink "$manifest"; fi
  if [[ -d "$dmg_stage" ]]; then rm -rf "$dmg_stage"; fi
}
trap cleanup EXIT
/usr/bin/ditto "$distribution_bundle" "$dmg_stage/MIO.app"
ln -s /Applications "$dmg_stage/Applications"
/usr/bin/hdiutil create -volname "MIO" -srcfolder "$dmg_stage" -ov -format UDZO "$dmg_path" >/dev/null
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$distribution_bundle" "$zip_path"
packaged_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
/usr/bin/plutil -create xml1 "$manifest"
/usr/bin/plutil -insert version -string "$version" "$manifest"
/usr/bin/plutil -insert sourceCommit -string "$source_commit" "$manifest"
/usr/bin/plutil -insert sourceDirty -bool "$source_dirty" "$manifest"
/usr/bin/plutil -insert branch -string "$source_branch" "$manifest"
/usr/bin/plutil -insert profile -string "$build_profile" "$manifest"
/usr/bin/plutil -insert architecture -string "$binary_arch" "$manifest"
/usr/bin/plutil -insert app -string MIO.app "$manifest"
/usr/bin/plutil -insert dmg -string "$dmg_name" "$manifest"
/usr/bin/plutil -insert zip -string "$zip_name" "$manifest"
/usr/bin/plutil -insert binarySha256 -string "$binary_sha256" "$manifest"
/usr/bin/plutil -insert packagedAt -string "$packaged_at" "$manifest"
/usr/bin/plutil -insert validation -json '{"status":"not_run_by_package_script","plist":"passed","signature":"passed"}' "$manifest"
/usr/bin/plutil -convert json -r "$manifest"
mv "$manifest" "$distribution_dir/preview-build.json"
echo "$distribution_bundle"
echo "$dmg_path"
echo "$zip_path"
