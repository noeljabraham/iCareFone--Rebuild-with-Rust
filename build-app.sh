#!/bin/sh
set -eu

PROJECT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_DIR="$PROJECT_DIR/dist/iCareFone Clone.app"
RUSTUP_BIN=${RUSTUP_BIN:-/opt/homebrew/opt/rustup/bin}

PATH="$RUSTUP_BIN:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$HOME/.cargo/bin:$HOME/Library/Android/sdk/platform-tools"
export PATH

cd "$PROJECT_DIR"
cargo build --release
mkdir -p "$APP_DIR/Contents/MacOS"
cp "$PROJECT_DIR/target/release/icarefone-clone" "$APP_DIR/Contents/MacOS/icarefone-clone"
cp "$PROJECT_DIR/packaging/Info.plist" "$APP_DIR/Contents/Info.plist"
chmod 755 "$APP_DIR/Contents/MacOS/icarefone-clone"
codesign --force --sign - "$APP_DIR"
echo "Successfully built bundle at: $APP_DIR"

