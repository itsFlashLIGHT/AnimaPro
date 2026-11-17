#!/usr/bin/env bash
set -euo pipefail

# Cross-build an experimental Windows encoder on macOS. It still requires
# execution, packaging and licensing checks on Windows before distribution.
if [[ $# -ne 4 ]]; then
  echo "Usage: bash $0 FFMPEG_SOURCE.tar.xz X264_SOURCE.tar.gz LLVM_MINGW_DIR NEW_OUTPUT_DIR" >&2
  exit 2
fi
ffmpeg_source=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
x264_source=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
toolchain=$(cd "$3" && pwd)
parent=$(cd "$(dirname "$4")" && pwd)
output="$parent/$(basename "$4")"
[[ ! -e "$output" ]] || { echo "Output already exists: $output" >&2; exit 1; }
[[ $(shasum -a 256 "$ffmpeg_source" | awk '{print $1}') == 8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e ]] || { echo "Unexpected FFmpeg source checksum" >&2; exit 1; }
[[ $(shasum -a 256 "$x264_source" | awk '{print $1}') == 142c4deb1f342f38033265b2ac37a764a2b6c914191859c72966dbce994e7e92 ]] || { echo "Unexpected x264 source checksum" >&2; exit 1; }
[[ -x "$toolchain/bin/x86_64-w64-mingw32-clang" && -x "$toolchain/bin/llvm-readobj" ]] || { echo "Incomplete llvm-mingw toolchain" >&2; exit 1; }
toolchain_version=$("$toolchain/bin/x86_64-w64-mingw32-clang" --version)
[[ "$toolchain_version" == *"clang version 23.1.5"* ]] || { echo "Expected llvm-mingw with clang 23.1.5" >&2; exit 1; }
pkg_config=$(command -v pkg-config)
work=$(mktemp -d "${TMPDIR:-/tmp}/animapro-ffmpeg-windows.XXXXXXXX")
stage=$(mktemp -d "$parent/.animapro-ffmpeg-stage.XXXXXXXX")
trap 'rm -rf "$work" "$stage"' EXIT
mkdir -p "$work/x264" "$work/prefix" "$stage/notices/mingw-runtime"
tar -xzf "$x264_source" -C "$work/x264"
tar -xJf "$ffmpeg_source" -C "$work"
export PATH="$toolchain/bin:$PATH"
(
  cd "$work/x264"
  CC=x86_64-w64-mingw32-clang ./configure --prefix="$work/prefix" --host=x86_64-w64-mingw32 --cross-prefix=x86_64-w64-mingw32- --enable-static --disable-cli --disable-asm --disable-opencl --enable-pic
  make -j2
  make install-lib-static
)
(
  cd "$work/ffmpeg-9.0.2"
  PKG_CONFIG_LIBDIR="$work/prefix/lib/pkgconfig" ./configure --prefix="$work/ffmpeg-prefix" --arch=x86_64 --target-os=mingw32 --enable-cross-compile --cross-prefix=x86_64-w64-mingw32- --cc=x86_64-w64-mingw32-clang --pkg-config="$pkg_config" --pkg-config-flags=--static --extra-ldflags=-static --disable-everything --disable-autodetect --disable-network --disable-doc --disable-ffplay --disable-ffprobe --disable-asm --enable-gpl --enable-libx264 --enable-encoder=libx264 --enable-decoder=rawvideo,wrapped_avframe --enable-demuxer=rawvideo --enable-muxer=mp4,null --enable-protocol=file,pipe,fd --enable-filter=scale,format,color --enable-indev=lavfi
  make -j2 ffmpeg.exe
  for setting in CONFIG_LIBX264_ENCODER CONFIG_RAWVIDEO_DEMUXER CONFIG_FD_PROTOCOL CONFIG_SCALE_FILTER CONFIG_MP4_MUXER; do
    grep -qx "#define $setting 1" config_components.h || { echo "Missing FFmpeg component: $setting" >&2; exit 1; }
  done
)
cp "$work/ffmpeg-9.0.2/ffmpeg.exe" "$stage/ffmpeg.exe"
"$toolchain/bin/llvm-readobj" --file-headers "$stage/ffmpeg.exe" > "$stage/HEADERS.txt"
grep -q 'IMAGE_FILE_MACHINE_AMD64' "$stage/HEADERS.txt" || { echo 'Encoder is not Windows x64' >&2; exit 1; }
"$toolchain/bin/llvm-readobj" --coff-imports "$stage/ffmpeg.exe" > "$stage/IMPORTS.txt"
while IFS= read -r dependency; do
  case "$(printf '%s' "$dependency" | tr '[:upper:]' '[:lower:]')" in
    kernel32.dll|shell32.dll|bcrypt.dll|api-ms-win-crt-*.dll) ;;
    *) echo "Unexpected Windows DLL dependency: $dependency" >&2; exit 1 ;;
  esac
done < <(awk '/^[[:space:]]*Name:/ {print $2}' "$stage/IMPORTS.txt")
cp "$ffmpeg_source" "$stage/notices/ffmpeg-9.0.2.tar.xz"
cp "$x264_source" "$stage/notices/x264-b35605ac.tar.gz"
cp "$work/ffmpeg-9.0.2/COPYING.GPLv2" "$stage/notices/FFMPEG-LICENSE.txt"
cp "$work/x264/COPYING" "$stage/notices/X264-LICENSE.txt"
cp "$toolchain/LICENSE.TXT" "$stage/notices/LLVM-LICENSE.txt"
printf '%s\n' "$toolchain_version" > "$stage/notices/COMPILER.txt"
cp "$toolchain/x86_64-w64-mingw32/share/mingw32/"COPYING* "$stage/notices/mingw-runtime/"
cp "$work/ffmpeg-9.0.2/ffbuild/config.mak" "$stage/notices/FFMPEG-CONFIG.mak"
cat > "$stage/notices/FFMPEG-SOURCE.txt" <<'EOF'
Experimental Windows x64 cross-build; not tested on Windows and not a release.
FFmpeg 9.0.2 source: ffmpeg-9.0.2.tar.xz, https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz
x264 source: x264-b35605ac.tar.gz, commit b35605ace3ddf7c1a5d67a2eb553f034aef41d55, https://code.videolan.org/videolan/x264.git
Build commands are in scripts/build-ffmpeg-windows-cross.sh; FFmpeg configuration is in FFMPEG-CONFIG.mak.
Toolchain: the supplied llvm-mingw directory; compiler version is in COMPILER.txt.
The build expects llvm-mingw 20261104 UCRT: https://github.com/mstorsjo/llvm-mingw/releases/tag/20261104
Review the exact toolchain runtime sources and all license obligations before distribution.
EOF
(cd "$stage" && shasum -a 256 ffmpeg.exe notices/*.tar.* > SHA256SUMS.txt)
mv "$stage" "$output"
echo "Created experimental encoder: $output"
