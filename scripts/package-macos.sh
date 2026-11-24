#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
export CARGO_INCREMENTAL=0
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
version="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; meta = json.load(sys.stdin); print(next(p["version"] for p in meta["packages"] if p["name"] == "mio-app"))')"
if [[ -z "$version" ]]; then
  echo "Failed to determine workspace version from cargo metadata" >&2
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
/usr/bin/plutil -replace CFBundleShortVersionString -string "$version" "$app_bundle/Contents/Info.plist"
install -m 644 crates/mio-app/assets/AppIcon.icns "$app_bundle/Contents/Resources/AppIcon.icns"
/usr/bin/plutil -lint "$app_bundle/Contents/Info.plist"
sign_target() {
  local target="$1"
  if [[ -n "${MIO_MACOS_SIGN_IDENTITY:-}" ]]; then
    local sign_args=(--force --timestamp --options runtime --sign "$MIO_MACOS_SIGN_IDENTITY")
    if [[ -n "${MIO_MACOS_ENTITLEMENTS:-}" ]]; then
      sign_args+=(--entitlements "$MIO_MACOS_ENTITLEMENTS")
    fi
    /usr/bin/codesign "${sign_args[@]}" "$target"
  else
    /usr/bin/codesign --force --sign - "$target"
  fi
}

signing_mode="adhoc"
if [[ -n "${MIO_MACOS_SIGN_IDENTITY:-}" ]]; then
  signing_mode="developer_id"
fi
notarized=false

sign_target "$app_bundle/Contents/MacOS/mio-cli"
sign_target "$app_bundle/Contents/MacOS/mio"
sign_target "$app_bundle"
/usr/bin/codesign --verify --deep --strict "$app_bundle"
distribution_bundle="$distribution_dir/MIO.app"
mkdir -p "$distribution_dir"
/usr/bin/ditto "$app_bundle" "$distribution_bundle"
install -m 755 "$bin_dir/mio-cli" "$distribution_dir/mio-cli"
sign_target "$distribution_dir/mio-cli"
/usr/bin/codesign --verify --deep --strict "$distribution_bundle"
binary_arch="$(/usr/bin/lipo -archs "$distribution_bundle/Contents/MacOS/mio")"
if [[ -z "$binary_arch" ]]; then
  echo "Packaged binary is not a valid Mach-O executable: $distribution_bundle/Contents/MacOS/mio" >&2
  exit 1
fi
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
if [[ "$signing_mode" == "developer_id" ]]; then
  /usr/bin/codesign --force --timestamp --sign "$MIO_MACOS_SIGN_IDENTITY" "$dmg_path"
fi
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$distribution_bundle" "$zip_path"

notary_auth=()
if [[ "$signing_mode" == "developer_id" ]]; then
  if [[ -n "${MIO_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
    notary_auth=(--keychain-profile "$MIO_NOTARY_KEYCHAIN_PROFILE")
  elif [[ -n "${MIO_APPLE_ID:-}" && -n "${MIO_APPLE_TEAM_ID:-}" && -n "${MIO_APPLE_APP_PASSWORD:-}" ]]; then
    notary_auth=(--apple-id "$MIO_APPLE_ID" --team-id "$MIO_APPLE_TEAM_ID" --password "$MIO_APPLE_APP_PASSWORD")
  elif [[ -n "${MIO_ASC_KEY_PATH:-}" && -n "${MIO_ASC_KEY_ID:-}" && -n "${MIO_ASC_ISSUER_ID:-}" ]]; then
    notary_auth=(--key "$MIO_ASC_KEY_PATH" --key-id "$MIO_ASC_KEY_ID" --issuer "$MIO_ASC_ISSUER_ID")
  fi
fi

if [[ ${#notary_auth[@]} -gt 0 ]]; then
  xcrun notarytool submit "$zip_path" "${notary_auth[@]}" --wait
  xcrun stapler staple "$distribution_bundle"
  rm -f "$zip_path"
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$distribution_bundle" "$zip_path"
  xcrun notarytool submit "$dmg_path" "${notary_auth[@]}" --wait
  xcrun stapler staple "$dmg_path"
  /usr/sbin/spctl --assess --type exec --verbose "$distribution_bundle"
  notarized=true
fi

binary_sha256="$(/usr/bin/shasum -a 256 "$distribution_bundle/Contents/MacOS/mio" | awk '{print $1}')"
packaged_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
MIO_PKG_VERSION="$version" \
MIO_PKG_COMMIT="$source_commit" \
MIO_PKG_DIRTY="$source_dirty" \
MIO_PKG_BRANCH="$source_branch" \
MIO_PKG_PROFILE="$build_profile" \
MIO_PKG_ARCH="$binary_arch" \
MIO_PKG_DMG="$dmg_name" \
MIO_PKG_ZIP="$zip_name" \
MIO_PKG_SHA256="$binary_sha256" \
MIO_PKG_SIGNING_MODE="$signing_mode" \
MIO_PKG_NOTARIZED="$notarized" \
MIO_PKG_PACKAGED_AT="$packaged_at" \
python3 - "$manifest" <<'PY'
import json
import os
import sys

payload = {
    "version": os.environ["MIO_PKG_VERSION"],
    "sourceCommit": os.environ["MIO_PKG_COMMIT"],
    "sourceDirty": os.environ["MIO_PKG_DIRTY"] == "true",
    "branch": os.environ["MIO_PKG_BRANCH"],
    "profile": os.environ["MIO_PKG_PROFILE"],
    "architecture": os.environ["MIO_PKG_ARCH"],
    "app": "MIO.app",
    "dmg": os.environ["MIO_PKG_DMG"],
    "zip": os.environ["MIO_PKG_ZIP"],
    "binarySha256": os.environ["MIO_PKG_SHA256"],
    "signingMode": os.environ["MIO_PKG_SIGNING_MODE"],
    "notarized": os.environ["MIO_PKG_NOTARIZED"] == "true",
    "packagedAt": os.environ["MIO_PKG_PACKAGED_AT"],
    "validation": {
        "status": "not_run_by_package_script",
        "plist": "passed",
        "signature": "passed",
    },
}
with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump(payload, fh, indent=2)
    fh.write("\n")
PY
mv "$manifest" "$distribution_dir/preview-build.json"
echo "$distribution_bundle"
echo "$dmg_path"
echo "$zip_path"
