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

BIN="$1"; VERSION="${2#v}"; TARGET="$3"; OUT="$4"
APP_NAME="tdmcp.app"
BUNDLE_ID="com.verbalize.tdmcp"

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PLIST_TEMPLATE="$REPO_ROOT/packaging/macos/Info.plist.template"
ENTITLEMENTS="$REPO_ROOT/packaging/macos/entitlements.plist"
ICON_SRC="$REPO_ROOT/crates/tdmcp-gui/assets/icon-normal.png"

[[ -f "$BIN" ]] || { echo "binary not found: $BIN" >&2; exit 1; }

echo "== bundle $APP_NAME =="
rm -rf "$OUT/$APP_NAME"
mkdir -p "$OUT/$APP_NAME/Contents/MacOS" "$OUT/$APP_NAME/Contents/Resources"
cp "$BIN" "$OUT/$APP_NAME/Contents/MacOS/tdmcp-daemon"
chmod 755 "$OUT/$APP_NAME/Contents/MacOS/tdmcp-daemon"

echo "== icns from $(basename "$ICON_SRC") =="
ICONSET="$(mktemp -d)/tdmcp.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  dbl=$((size * 2))
  sips -z "$size" "$size" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z "$dbl" "$dbl" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$OUT/$APP_NAME/Contents/Resources/tdmcp.icns"

echo "== Info.plist =="
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUNDLE_ID@/$BUNDLE_ID/g" \
  "$PLIST_TEMPLATE" > "$OUT/$APP_NAME/Contents/Info.plist"

echo "== codesign =="
if [[ -n "${APPLE_DEVELOPER_ID_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$APPLE_DEVELOPER_ID_IDENTITY" "$OUT/$APP_NAME"
else
  echo "::warning::ad-hoc codesign (no APPLE_DEVELOPER_ID_IDENTITY) — Gatekeeper blocks downloaded copies until signed/notarized"
  codesign --force --deep -s - "$OUT/$APP_NAME"
fi

echo "== dmg =="
DMG="$OUT/tdmcp-rs-$VERSION-$TARGET.dmg"
rm -f "$DMG"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$OUT/$APP_NAME" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "td-mcp-rs $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG"

if [[ -n "${APPLE_DEVELOPER_ID_IDENTITY:-}" && -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
  echo "== notarize =="
  xcrun notarytool submit "$DMG" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
else
  echo "::warning::skipping notarization (needs APPLE_DEVELOPER_ID_IDENTITY + APPLE_NOTARY_PROFILE)"
fi

echo "built $DMG"
