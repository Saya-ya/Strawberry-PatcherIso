$ErrorActionPreference = "Stop"

# Build the Windows executable for distribution.
#
# Option A (native, recommended): run this script on a Windows x64 machine
# with the Rust MSVC toolchain installed.
#
# Option B (cross-compile from Linux): the gnu target produces a standalone
# .exe with no extra DLLs, so it works without a Windows machine:
#
#   rustup target add x86_64-pc-windows-gnu
#   cargo build --release --target x86_64-pc-windows-gnu -p straw-patcher
#   cp target/x86_64-pc-windows-gnu/release/straw-patcher.exe dist/StrawPatcher.exe
#
$target = "x86_64-pc-windows-msvc"

rustup target add $target
cargo build --release --target $target -p straw-patcher

$destination = Join-Path $PSScriptRoot "dist"
New-Item -ItemType Directory -Force -Path $destination | Out-Null
Copy-Item (Join-Path $PSScriptRoot "target/$target/release/straw-patcher.exe") `
    (Join-Path $destination "StrawPatcher.exe") -Force
Write-Host "Executable generado: $destination/StrawPatcher.exe"
