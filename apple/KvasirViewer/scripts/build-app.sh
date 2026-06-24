#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$PACKAGE_DIR/../.." && pwd)"
BUILD_DIR="${KVASIR_VIEWER_BUILD_DIR:-$PACKAGE_DIR/.build/app-bundle}"
INTERMEDIATES_DIR="$BUILD_DIR/intermediates"
GEN_DIR="$INTERMEDIATES_DIR/generated/kvasir-client"
MODULE_DIR="$INTERMEDIATES_DIR/modules"
MODULE_CACHE_DIR="$INTERMEDIATES_DIR/clang-module-cache"
APP_DIR="$BUILD_DIR/Kvasir.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
FRAMEWORKS_DIR="$CONTENTS_DIR/Frameworks"
LAUNCH_AGENTS_DIR="$CONTENTS_DIR/Library/LaunchAgents"
RUST_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
RUST_RELEASE_DIR="$RUST_TARGET_DIR/release"
CLIENT_LIB="$RUST_RELEASE_DIR/libkvasir_client.dylib"
DAEMON_BIN="$RUST_RELEASE_DIR/kvasird"
BINDGEN_BIN="$RUST_RELEASE_DIR/uniffi-bindgen-swift"

mkdir -p "$GEN_DIR" "$MODULE_DIR" "$MODULE_CACHE_DIR" "$MACOS_DIR" "$FRAMEWORKS_DIR" "$LAUNCH_AGENTS_DIR"

(
    cd "$REPO_ROOT"
    cargo build --release -p kvasir-client -p kvasird -p kvasir-uniffi-bindgen
)

"$BINDGEN_BIN" --swift-sources "$CLIENT_LIB" "$GEN_DIR"
"$BINDGEN_BIN" --headers "$CLIENT_LIB" "$GEN_DIR"
"$BINDGEN_BIN" --modulemap --module-name kvasir_clientFFI "$CLIENT_LIB" "$GEN_DIR"
cp "$GEN_DIR/kvasir_client.modulemap" "$GEN_DIR/module.modulemap"

cp "$CLIENT_LIB" "$FRAMEWORKS_DIR/libkvasir_client.dylib"
install_name_tool -id "@rpath/libkvasir_client.dylib" "$FRAMEWORKS_DIR/libkvasir_client.dylib"

swiftc \
    -O \
    -parse-as-library \
    -module-cache-path "$MODULE_CACHE_DIR" \
    -emit-library \
    -emit-module \
    -module-name KvasirViewerCore \
    -emit-module-path "$MODULE_DIR/KvasirViewerCore.swiftmodule" \
    -Xlinker -install_name \
    -Xlinker "@rpath/libKvasirViewerCore.dylib" \
    -o "$FRAMEWORKS_DIR/libKvasirViewerCore.dylib" \
    "$PACKAGE_DIR"/Sources/KvasirViewerCore/*.swift

swiftc \
    -O \
    -parse-as-library \
    -module-cache-path "$MODULE_CACHE_DIR" \
    -emit-library \
    -emit-module \
    -module-name kvasir_client \
    -emit-module-path "$MODULE_DIR/kvasir_client.swiftmodule" \
    -I "$GEN_DIR" \
    -L "$FRAMEWORKS_DIR" \
    -lkvasir_client \
    -Xlinker -rpath \
    -Xlinker "@loader_path" \
    -Xlinker -install_name \
    -Xlinker "@rpath/libkvasir_client_swift.dylib" \
    -o "$FRAMEWORKS_DIR/libkvasir_client_swift.dylib" \
    "$GEN_DIR/kvasir_client.swift"

swiftc \
    -O \
    -emit-executable \
    -module-cache-path "$MODULE_CACHE_DIR" \
    -module-name KvasirViewer \
    -I "$MODULE_DIR" \
    -I "$GEN_DIR" \
    -L "$FRAMEWORKS_DIR" \
    -lKvasirViewerCore \
    -lkvasir_client \
    -lkvasir_client_swift \
    -Xlinker -rpath \
    -Xlinker "@executable_path/../Frameworks" \
    -o "$MACOS_DIR/KvasirViewer" \
    "$PACKAGE_DIR"/Sources/KvasirViewer/*.swift

cp "$DAEMON_BIN" "$MACOS_DIR/kvasird"
cp "$PACKAGE_DIR/LaunchAgents/dev.kvasir.kvasird.plist" \
    "$LAUNCH_AGENTS_DIR/dev.kvasir.kvasird.plist"

DAEMON_FINGERPRINT="$(shasum -a 256 "$MACOS_DIR/kvasird" | awk '{print $1}')"
PLIST_FINGERPRINT="$(shasum -a 256 "$LAUNCH_AGENTS_DIR/dev.kvasir.kvasird.plist" | awk '{print $1}')"
LAUNCH_AGENT_REGISTRATION_POLICY_VERSION="2"
LAUNCH_AGENT_FINGERPRINT="$(printf '%s\n%s\n%s\n' "$DAEMON_FINGERPRINT" "$PLIST_FINGERPRINT" "$LAUNCH_AGENT_REGISTRATION_POLICY_VERSION" | shasum -a 256 | awk '{print $1}')"

cat > "$CONTENTS_DIR/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>KvasirViewer</string>
    <key>CFBundleIdentifier</key>
    <string>dev.kvasir.viewer</string>
    <key>CFBundleName</key>
    <string>Kvasir</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>KvasirLaunchAgentFingerprint</key>
    <string>${LAUNCH_AGENT_FINGERPRINT}</string>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

plutil -lint "$CONTENTS_DIR/Info.plist" "$LAUNCH_AGENTS_DIR/dev.kvasir.kvasird.plist"
BUNDLE_PROGRAM="$(plutil -extract BundleProgram raw "$LAUNCH_AGENTS_DIR/dev.kvasir.kvasird.plist")"
if [ "$BUNDLE_PROGRAM" != "Contents/MacOS/kvasird" ]; then
    echo "LaunchAgent BundleProgram must point at Contents/MacOS/kvasird" >&2
    exit 1
fi
if [ ! -x "$APP_DIR/$BUNDLE_PROGRAM" ]; then
    echo "LaunchAgent BundleProgram does not resolve to an executable in Kvasir.app" >&2
    exit 1
fi
INFO_FINGERPRINT="$(plutil -extract KvasirLaunchAgentFingerprint raw "$CONTENTS_DIR/Info.plist")"
if [ "$INFO_FINGERPRINT" != "$LAUNCH_AGENT_FINGERPRINT" ]; then
    echo "Info.plist LaunchAgent fingerprint does not match packaged helper files" >&2
    exit 1
fi
codesign --force --deep --sign - "$APP_DIR" >/dev/null
codesign --verify --deep --strict "$APP_DIR"

echo "$APP_DIR"
