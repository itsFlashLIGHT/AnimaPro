#!/usr/bin/env bash
set -euo pipefail

# Exercise packaging failures without touching the real dist directory or FFmpeg.
root=$(cd "$(dirname "$0")/.." && pwd)
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/animapro-package-test.XXXXXXXX")
trap 'rm -rf "$test_dir"' EXIT
mkdir -p "$test_dir/project/scripts" "$test_dir/project/docs" "$test_dir/project/dist/AnimaPro.app" "$test_dir/bin" "$test_dir/notices"
cp "$root/scripts/package-macos.sh" "$test_dir/project/scripts/"
printf 'lock fixture\n' > "$test_dir/project/Cargo.lock"
shasum -a 256 "$test_dir/project/Cargo.lock" | awk '{print $1}' > "$test_dir/project/docs/dependencies-lock.sha256"
printf 'original\n' > "$test_dir/project/dist/AnimaPro.app/keep"
printf 'notice\n' > "$test_dir/notices/FFMPEG-LICENSE.txt"
printf 'source\n' > "$test_dir/notices/FFMPEG-SOURCE.txt"
printf '#!/bin/sh\nexit 0\n' > "$test_dir/ffmpeg"
cat > "$test_dir/bin/cargo" <<'EOF'
#!/bin/sh
if [ "$1" = pkgid ]; then printf 'path+file:///test#animapro@0.1.0\n'; exit 0; fi
exit 1
EOF
cat > "$test_dir/bin/lipo" <<'EOF'
#!/bin/sh
printf '%s\n' "$TEST_ARCHS"
EOF
cat > "$test_dir/bin/otool" <<'EOF'
#!/bin/sh
printf 'ffmpeg:\n\t/opt/homebrew/lib/libexample.dylib (compatibility version 1.0.0)\n'
EOF
chmod +x "$test_dir/ffmpeg" "$test_dir/bin/"*
export PATH="$test_dir/bin:$PATH" ANIMAPRO_FFMPEG_MACOS="$test_dir/ffmpeg" ANIMAPRO_FFMPEG_NOTICE_DIR="$test_dir/notices"
if ANIMAPRO_PACKAGE_NAME='../outside.app' "$test_dir/project/scripts/package-macos.sh" >"$test_dir/output" 2>&1; then
  echo 'Expected rejection of a package name containing a path' >&2; exit 1
fi
grep -q 'visible .app name without path separators' "$test_dir/output"
export TEST_ARCHS='arm64e x86_64'
if "$test_dir/project/scripts/package-macos.sh" >"$test_dir/output" 2>&1; then
  echo 'Expected rejection of arm64e without arm64' >&2; exit 1
fi
grep -q 'must contain arm64 and x86_64' "$test_dir/output"
export TEST_ARCHS='arm64 x86_64'
if "$test_dir/project/scripts/package-macos.sh" >"$test_dir/output" 2>&1; then
  echo 'Expected rejection of unbundled dynamic library' >&2; exit 1
fi
grep -q 'unbundled library dependency' "$test_dir/output"
[[ $(cat "$test_dir/project/dist/AnimaPro.app/keep") == original ]]
echo 'macOS packaging failure checks passed'
