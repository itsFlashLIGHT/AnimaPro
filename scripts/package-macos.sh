#!/usr/bin/env bash
set -euo pipefail

# Creates dist/AnimaPro.app as a universal macOS application.
# Pass a universal, redistributable FFmpeg binary with libx264 enabled:
#   ANIMAPRO_FFMPEG_MACOS=/absolute/path/to/ffmpeg ./scripts/package-macos.sh

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
ffmpeg_bin=${ANIMAPRO_FFMPEG_MACOS:-}
if [[ -z "$ffmpeg_bin" || ! -f "$ffmpeg_bin" ]]; then
  echo "Set ANIMAPRO_FFMPEG_MACOS to a redistributable universal FFmpeg binary." >&2
  exit 1
fi
ffmpeg_archs=$(lipo -archs "$ffmpeg_bin")
[[ "$ffmpeg_archs" == *"arm64"* && "$ffmpeg_archs" == *"x86_64"* ]] || { echo "FFmpeg must contain arm64 and x86_64 slices; got: $ffmpeg_archs" >&2; exit 1; }
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target list --installed | grep -qx "$target" || { echo "Missing Rust target: $target" >&2; exit 1; }
  cargo build --release --target "$target"
done
mkdir -p dist/AnimaPro.app/Contents/{MacOS,Resources}
lipo -create target/aarch64-apple-darwin/release/animapro target/x86_64-apple-darwin/release/animapro -output dist/AnimaPro.app/Contents/MacOS/animapro
cp "$ffmpeg_bin" dist/AnimaPro.app/Contents/Resources/ffmpeg
chmod +x dist/AnimaPro.app/Contents/{MacOS/animapro,Resources/ffmpeg}
cat > dist/AnimaPro.app/Contents/Info.plist <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>animapro</string><key>CFBundleIdentifier</key><string>org.animapro.editor</string><key>CFBundleName</key><string>AnimaPro</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>0.1.0</string></dict></plist>
PLIST
echo "Created dist/AnimaPro.app"
