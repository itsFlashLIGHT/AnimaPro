param(
  [Parameter(Mandatory=$true)] [string]$FfmpegPath,
  [Parameter(Mandatory=$true)] [string]$FfmpegNoticesPath
)
$ErrorActionPreference = 'Stop'
$ffmpeg = (Resolve-Path -LiteralPath $FfmpegPath).Path
$notices = (Resolve-Path -LiteralPath $FfmpegNoticesPath).Path
foreach ($fileName in @('FFMPEG-LICENSE.txt', 'FFMPEG-SOURCE.txt')) {
  $file = Get-Item -LiteralPath (Join-Path $notices $fileName)
  if ($file.PSIsContainer -or $file.Length -eq 0) { throw "Missing or empty FFmpeg notice: $fileName" }
}
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$expectedHash = (Get-Content -LiteralPath 'docs\dependencies-lock.sha256' -Raw).Trim().ToLowerInvariant()
$actualHash = (Get-FileHash -LiteralPath 'Cargo.lock' -Algorithm SHA256).Hash.ToLowerInvariant()
if ($expectedHash -ne $actualHash) { throw 'Dependency report is stale; run scripts/update-dependency-report.py.' }
$packageId = & cargo pkgid --locked
if ($LASTEXITCODE -ne 0) { throw 'Could not read package version' }
$version = ($packageId -split '@')[-1]
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "Unsupported release version: $version" }
function Assert-X64([string]$Path) {
  $file = [System.IO.File]::OpenRead($Path)
  try {
    $reader = [System.IO.BinaryReader]::new($file)
    if ($reader.ReadUInt16() -ne 0x5a4d) { throw "Not a PE executable: $Path" }
    $file.Seek(0x3c, [System.IO.SeekOrigin]::Begin) | Out-Null
    $offset = $reader.ReadInt32()
    if ($offset -lt 0 -or $offset -gt $file.Length - 6) { throw "Invalid PE header: $Path" }
    $file.Seek($offset, [System.IO.SeekOrigin]::Begin) | Out-Null
    if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) { throw "Executable must be Windows x64: $Path" }
  } finally { $file.Dispose() }
}
Assert-X64 $ffmpeg
& $ffmpeg -v error -f lavfi -i 'color=size=16x16:rate=1' -frames:v 1 -c:v libx264 -pix_fmt yuv420p -f null -
if ($LASTEXITCODE -ne 0) { throw "FFmpeg could not encode a test frame with libx264" }
cargo build --release --locked --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw "Cargo build failed with exit code $LASTEXITCODE" }
$binary = Join-Path $root 'target\x86_64-pc-windows-msvc\release\animapro.exe'
Assert-X64 $binary
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$stage = Join-Path $dist ('.animapro-stage-' + [Guid]::NewGuid().ToString())
$backup = "$stage-previous"
$out = Join-Path $dist 'AnimaPro-windows-x64'
try {
  New-Item -ItemType Directory -Path $stage | Out-Null
  Copy-Item -LiteralPath $binary -Destination (Join-Path $stage 'AnimaPro.exe')
  Copy-Item -LiteralPath $ffmpeg -Destination (Join-Path $stage 'ffmpeg.exe')
  Copy-Item -LiteralPath 'docs\THIRD_PARTY_DEPENDENCIES.tsv','docs\dependencies-lock.sha256' -Destination $stage
  Copy-Item -LiteralPath 'LICENSE' -Destination (Join-Path $stage 'AnimaPro-LICENSE.txt')
  Copy-Item -LiteralPath $notices -Destination (Join-Path $stage 'FFmpeg-notices') -Recurse
  $packagedFfmpeg = Join-Path $stage 'ffmpeg.exe'
  $binaryHash = (Get-FileHash -LiteralPath $packagedFfmpeg -Algorithm SHA256).Hash.ToLowerInvariant()
  $buildInfo = Join-Path $stage 'BUILD-INFO.txt'
  "AnimaPro: $version`nFFmpeg SHA-256: $binaryHash`n" | Set-Content -LiteralPath $buildInfo -Encoding UTF8
  & rustc --version | Add-Content -LiteralPath $buildInfo -Encoding UTF8
  if ($LASTEXITCODE -ne 0) { throw 'Could not get Rust version' }
  foreach ($flag in @('-version','-buildconf','-L')) {
    & $packagedFfmpeg $flag 2>&1 | Add-Content -LiteralPath $buildInfo -Encoding UTF8
    if ($LASTEXITCODE -ne 0) { throw "Could not get FFmpeg information: $flag" }
  }
  @"
AnimaPro portable release
=========================
Run AnimaPro.exe. FFmpeg is included beside the application for MP4 export.
"@ | Set-Content -LiteralPath (Join-Path $stage 'README.txt') -Encoding UTF8
  if (Test-Path -LiteralPath $out) { Move-Item -LiteralPath $out -Destination $backup }
  try { Move-Item -LiteralPath $stage -Destination $out }
  catch {
    if (Test-Path -LiteralPath $backup) { Move-Item -LiteralPath $backup -Destination $out }
    throw
  }
  if (Test-Path -LiteralPath $backup) { Remove-Item -LiteralPath $backup -Recurse -Force }
} finally {
  if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
Write-Host "Created $out"
