param(
  [Parameter(Mandatory=$true)] [string]$FfmpegPath
)
$ErrorActionPreference = 'Stop'
# Run this script on Windows x64 with the MSVC Rust target and a redistributable
# ffmpeg.exe that has libx264 enabled.
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (!(Test-Path $FfmpegPath)) { throw "FFmpeg was not found: $FfmpegPath" }
cargo build --release --target x86_64-pc-windows-msvc
$out = Join-Path $root 'dist\AnimaPro-windows-x64'
New-Item -ItemType Directory -Force -Path $out | Out-Null
Copy-Item 'target\x86_64-pc-windows-msvc\release\animapro.exe' (Join-Path $out 'AnimaPro.exe') -Force
Copy-Item $FfmpegPath (Join-Path $out 'ffmpeg.exe') -Force
@"
AnimaPro portable release
=========================
Run AnimaPro.exe. FFmpeg is included beside the application for MP4 export.
"@ | Set-Content (Join-Path $out 'README.txt') -Encoding UTF8
Write-Host "Created $out"
