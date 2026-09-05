#!/usr/bin/env bash
# Bundle the macOS .app, sign it (Developer ID or ad-hoc), build the DMG,
# optionally notarize + staple. Run on macOS (CI macos-* runners have every tool).
#
# usage: make_app.sh <daemon-binary> <version> <target-triple> <out-dir>
#
# Signing escalation ladder (all automatic):
#   APPLE_DEVELOPER_ID_IDENTITY set  -> real Developer ID sign (+ hardened runtime)
#   otherwise                        -> ad-hoc (-), works locally; downloads get
#                                       blocked by Gatekeeper until signed.
#   APPLE_NOTARY_PROFILE set (+ identity) -> notarytool submit --wait + stapler.

set -euo pipefail

usage() { echo "usage: $0 <daemon-binary> <version> <target-triple> <out-dir>" >&2; exit 2; }
[[ $# -eq 4 ]] || usage

BIN="$1"
VERSION="${2#v}"
TARGET="$3"
OUT="$4"
APP_NAME="tdmcp.app"
BUNDLE_ID="com.verbalize.tdmcp"

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PLIST_TEMPLATE="$REPO_ROOT/packaging/macos/Info.plist.template"
ENTITLEMENTS="$REPO_ROOT/packaging/macos/entitlements.plist"
ICON_SRC="$REPO_ROOT/crates/tdmcp-gui/assets/icon-normal.png"

[[ -f "$BIN" ]] || { echo "binary not found: $BIN" >&2; exit 1; }
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "invalid release version: $VERSION" >&2; exit 1; }
[[ "$TARGET" == aarch64-apple-darwin || "$TARGET" == x86_64-apple-darwin ]] || { echo "unsupported macOS target: $TARGET" >&2; exit 1; }
[[ -n "$OUT" && "$OUT" != / ]] || { echo "choose a non-root output directory" >&2; exit 1; }
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
PACKAGE_WORK="$(mktemp -d "$OUT/.tdmcp-macos.XXXXXX")"
readonly PACKAGE_WORK
APP="$PACKAGE_WORK/$APP_NAME"
APP_PUBLISHED=false
COMMITTED=false

cleanup() {
  local status=$?
  if [[ "$COMMITTED" != true ]]; then
    if [[ "$APP_PUBLISHED" == true ]]; then
      if ! mv "$OUT/$APP_NAME" "$PACKAGE_WORK/failed.app"; then
        echo "Cannot roll back new app; recovery files retained at $PACKAGE_WORK" >&2
        return 1
      fi
    fi
    if [[ -e "$PACKAGE_WORK/previous.app" || -L "$PACKAGE_WORK/previous.app" ]]; then
      if ! mv "$PACKAGE_WORK/previous.app" "$OUT/$APP_NAME"; then
        echo "Cannot restore previous app; recovery files retained at $PACKAGE_WORK" >&2
        return 1
      fi
    fi
  fi
  # This is the private directory returned by mktemp above, never OUT itself.
  rm -rf "$PACKAGE_WORK"
  return "$status"
}
trap cleanup EXIT

echo "== bundle $APP_NAME =="
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/tdmcp-daemon"
cp "$REPO_ROOT/LICENSE" "$APP/Contents/Resources/LICENSE"
chmod 755 "$APP/Contents/MacOS/tdmcp-daemon"

echo "== icns from $(basename "$ICON_SRC") =="
ICONSET="$PACKAGE_WORK/tdmcp.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  dbl=$((size * 2))
  sips -z "$size" "$size" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z "$dbl" "$dbl" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/tdmcp.icns"

echo "== Info.plist =="
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUNDLE_ID@/$BUNDLE_ID/g" \
  "$PLIST_TEMPLATE" > "$APP/Contents/Info.plist"

echo "== codesign =="
if [[ -n "${APPLE_DEVELOPER_ID_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$APPLE_DEVELOPER_ID_IDENTITY" "$APP"
else
  echo "::warning::ad-hoc codesign (no APPLE_DEVELOPER_ID_IDENTITY) — Gatekeeper blocks downloaded copies until signed/notarized"
  codesign --force --deep -s - "$APP"
fi
codesign --verify --deep --strict "$APP"

echo "== dmg =="
DMG_NAME="tdmcp-rs-$VERSION-$TARGET.dmg"
DMG="$PACKAGE_WORK/$DMG_NAME"
STAGE="$PACKAGE_WORK/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "td-mcp-rs $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG"

if [[ -n "${APPLE_DEVELOPER_ID_IDENTITY:-}" && -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
  echo "== notarize =="
  xcrun notarytool submit "$DMG" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
else
  echo "::warning::skipping notarization (needs APPLE_DEVELOPER_ID_IDENTITY + APPLE_NOTARY_PROFILE)"
fi
hdiutil verify "$DMG"

# Keep previous artifacts until every native tool (including notarization)
# succeeds. Rename on the same filesystem, and restore the old app on error.
if [[ -e "$OUT/$APP_NAME" || -L "$OUT/$APP_NAME" ]]; then
  mv "$OUT/$APP_NAME" "$PACKAGE_WORK/previous.app"
fi
mv "$APP" "$OUT/$APP_NAME"
APP_PUBLISHED=true
mv -f "$DMG" "$OUT/$DMG_NAME"
COMMITTED=true

echo "built $OUT/$DMG_NAME"
