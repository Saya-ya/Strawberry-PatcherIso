$ErrorActionPreference = "Stop"

# Run from the repository root in a Windows x64 Rust environment.
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc -p straw-patcher

$destination = Join-Path $PSScriptRoot "dist"
New-Item -ItemType Directory -Force -Path $destination | Out-Null
Copy-Item (Join-Path $PSScriptRoot "target/x86_64-pc-windows-msvc/release/straw-patcher.exe") `
    (Join-Path $destination "StrawPatcher.exe") -Force
Write-Host "Executable generado: $destination/StrawPatcher.exe"
