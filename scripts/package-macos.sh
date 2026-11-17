#!/usr/bin/env bash
set -euo pipefail

# Requires a redistributable universal FFmpeg with libx264:
# ANIMAPRO_FFMPEG_MACOS=/path/to/ffmpeg ANIMAPRO_FFMPEG_NOTICE_DIR=/path/to/notices ./scripts/package-macos.sh
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
package_name=${ANIMAPRO_PACKAGE_NAME:-AnimaPro.app}
if [[ "$package_name" != *.app || "$package_name" == */* || "$package_name" == .* ]]; then
  echo "ANIMAPRO_PACKAGE_NAME must be a visible .app name without path separators." >&2
  exit 1
fi
package_path="dist/$package_name"
ffmpeg_bin=${ANIMAPRO_FFMPEG_MACOS:-}
if [[ -z "$ffmpeg_bin" || ! -f "$ffmpeg_bin" ]]; then
  echo "Set ANIMAPRO_FFMPEG_MACOS to a redistributable universal FFmpeg binary." >&2
  exit 1
fi
notices=${ANIMAPRO_FFMPEG_NOTICE_DIR:-}
if [[ -z "$notices" || ! -s "$notices/FFMPEG-LICENSE.txt" || ! -s "$notices/FFMPEG-SOURCE.txt" ]]; then
  echo "Set ANIMAPRO_FFMPEG_NOTICE_DIR to a directory containing nonempty FFMPEG-LICENSE.txt and FFMPEG-SOURCE.txt." >&2
  exit 1
fi
expected_hash=$(tr -d '\r\n' < docs/dependencies-lock.sha256)
actual_hash=$(shasum -a 256 Cargo.lock | awk '{print $1}')
[[ "$expected_hash" == "$actual_hash" ]] || { echo "Dependency report is stale; run scripts/update-dependency-report.py." >&2; exit 1; }
package_id=$(cargo pkgid --locked)
version=${package_id##*@}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Unsupported release version: $version" >&2; exit 1; }
ffmpeg_archs=$(lipo -archs "$ffmpeg_bin")
[[ " $ffmpeg_archs " == *" arm64 "* && " $ffmpeg_archs " == *" x86_64 "* ]] || { echo "FFmpeg must contain arm64 and x86_64 slices; got: $ffmpeg_archs" >&2; exit 1; }
# Only the executable is copied into the app. Reject links to third-party dylibs
# that would be missing on a clean machine, checking both architecture slices.
dependency_listing=$(otool -arch all -L "$ffmpeg_bin") || { echo "Could not inspect FFmpeg libraries." >&2; exit 1; }
while IFS= read -r dependency; do
  case "$dependency" in
    /usr/lib/*|/System/Library/*) ;;
    *) echo "FFmpeg has an unbundled library dependency: $dependency" >&2; exit 1 ;;
  esac
done < <(printf '%s\n' "$dependency_listing" | awk '/^[[:space:]]/ {print $1}')
"$ffmpeg_bin" -v error -f lavfi -i 'color=size=16x16:rate=1' -frames:v 1 -c:v libx264 -pix_fmt yuv420p -f null - || { echo "FFmpeg could not encode a test frame with libx264." >&2; exit 1; }
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target list --installed | grep -qx "$target" || { echo "Missing Rust target: $target" >&2; exit 1; }
  cargo build --release --locked --target "$target"
done
mkdir -p dist
stage=$(mktemp -d "${root}/dist/.animapro-stage.XXXXXXXX")
backup="${stage}.previous"
trap 'rm -rf "$stage"' EXIT
app="$stage/AnimaPro.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create target/aarch64-apple-darwin/release/animapro target/x86_64-apple-darwin/release/animapro -output "$app/Contents/MacOS/animapro"
app_archs=$(lipo -archs "$app/Contents/MacOS/animapro")
[[ " $app_archs " == *" arm64 "* && " $app_archs " == *" x86_64 "* ]] || { echo "Application is not universal: $app_archs" >&2; exit 1; }
cp "$ffmpeg_bin" "$app/Contents/Resources/ffmpeg"
cp assets/AnimaPro.icns "$app/Contents/Resources/AnimaPro.icns"
chmod +x "$app/Contents/MacOS/animapro" "$app/Contents/Resources/ffmpeg"
cp docs/THIRD_PARTY_DEPENDENCIES.tsv docs/dependencies-lock.sha256 "$app/Contents/Resources/"
cp LICENSE "$app/Contents/Resources/AnimaPro-LICENSE.txt"
mkdir -p "$app/Contents/Resources/FFmpeg-notices"
cp -R "$notices/." "$app/Contents/Resources/FFmpeg-notices/"
{
  printf 'AnimaPro: %s\nFFmpeg SHA-256: %s\n\n' "$version" "$(shasum -a 256 "$app/Contents/Resources/ffmpeg" | awk '{print $1}')"
  rustc --version
  "$app/Contents/Resources/ffmpeg" -version
  "$app/Contents/Resources/ffmpeg" -buildconf
  "$app/Contents/Resources/ffmpeg" -L
} > "$app/Contents/Resources/BUILD-INFO.txt"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>animapro</string><key>CFBundleIdentifier</key><string>org.animapro.editor</string><key>CFBundleName</key><string>AnimaPro</string><key>CFBundleIconFile</key><string>AnimaPro</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>$version</string><key>CFBundleVersion</key><string>$version</string></dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
if [[ -e "$package_path" ]]; then mv "$package_path" "$backup"; fi
if ! mv "$app" "$package_path"; then
  if [[ -e "$backup" ]]; then mv "$backup" "$package_path" || echo "Previous package remains at $backup" >&2; fi
  exit 1
fi
if [[ -e "$backup" ]]; then rm -rf "$backup"; fi
echo "Created $package_path"
