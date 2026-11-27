#!/bin/bash
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ "$(uname -m)" != arm64 ]]; then
    echo "The VR renderer helper currently requires Apple silicon." >&2
    exit 1
fi

VR_SOURCE="$PROJECT_ROOT/vr-runtime"
VR_BUILD="$PROJECT_ROOT/build/vr-runtime"
RENDERER_SOURCE="$VR_SOURCE/renderer/main.swift"
RENDERER_BIN="$VR_BUILD/ObeliskVRRenderer"
SDK_PATH="$(xcrun --sdk macosx --show-sdk-path)"
MINIMUM_MACOS="${MACOSX_DEPLOYMENT_TARGET:-26.4}"
BUILD_SETTINGS="$SDK_PATH|$MINIMUM_MACOS|${CONFIGURATION:-Debug}"
PREVIOUS_SETTINGS="$(cat "$VR_BUILD/settings" 2>/dev/null || true)"

mkdir -p "$VR_BUILD"
if [[ ! -x "$RENDERER_BIN" \
    || "$RENDERER_SOURCE" -nt "$RENDERER_BIN" \
    || "$0" -nt "$RENDERER_BIN" \
    || "$BUILD_SETTINGS" != "$PREVIOUS_SETTINGS" ]]; then
    echo "Building ObeliskVRRenderer..."
    xcrun --sdk macosx swiftc -target "arm64-apple-macos$MINIMUM_MACOS" -O \
        "$RENDERER_SOURCE" -o "$RENDERER_BIN"
    printf '%s\n' "$BUILD_SETTINGS" > "$VR_BUILD/settings"
fi

HELPERS_DEST="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Helpers/VR"
RESOURCES_DEST="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/VRRuntime"
mkdir -p "$HELPERS_DEST" "$RESOURCES_DEST/openxr" "$RESOURCES_DEST/opencomposite" "$RESOURCES_DEST/steamvr"

cp "$RENDERER_BIN" "$HELPERS_DEST/ObeliskVRRenderer"
chmod +x "$HELPERS_DEST/ObeliskVRRenderer"

# Copy Windows VR runtime payloads (OpenXR, OpenComposite, SteamVR driver_vrmin)
cp "$VR_SOURCE/openxr/questvrbridge_openxr.dll" \
   "$VR_SOURCE/openxr/questvrbridge_openxr.json" \
   "$VR_SOURCE/openxr/questvrbridge_openxr32.dll" \
   "$VR_SOURCE/openxr/questvrbridge_openxr32.json" \
   "$RESOURCES_DEST/openxr/"
cp "$VR_SOURCE/opencomposite/vrclient_x64.dll" \
   "$VR_SOURCE/opencomposite/vrclient.dll" \
   "$VR_SOURCE/opencomposite/LICENSE-OpenComposite.txt" \
   "$RESOURCES_DEST/opencomposite/"
rm -rf "$RESOURCES_DEST/steamvr/driver_vrmin"
cp -R "$VR_SOURCE/steamvr/driver_vrmin" "$RESOURCES_DEST/steamvr/driver_vrmin"
cp "$VR_SOURCE/NOTICE.md" "$RESOURCES_DEST/NOTICE.md"

IDENTITY="${EXPANDED_CODE_SIGN_IDENTITY:?A stable Apple signing identity is required}"
if [[ "${CONFIGURATION:-Debug}" == Release ]]; then TIMESTAMP=--timestamp; else TIMESTAMP=--timestamp=none; fi
/usr/bin/codesign --force --sign "$IDENTITY" "$TIMESTAMP" --options runtime \
    --identifier com.zagrios.obelisk.vr-renderer \
    --entitlements "$PROJECT_ROOT/packaging/vr-renderer.entitlements" \
    "$HELPERS_DEST/ObeliskVRRenderer"
