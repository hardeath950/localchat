# Build do LocalChat no Windows: binario + instalador (dist\LocalChat-Setup.exe).
# Uso:  powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
# Precisa: Rust (rustup), MSYS2 em C:\msys64 com mingw-w64-x86_64-gcc, Inno Setup 6.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

# o gcc do MSYS2 (linker e compilador do `ring`, usado no TLS do update) so acha
# as proprias DLLs com a pasta dele no PATH
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
# toolchain GNU (sem Visual Studio). Vai por variavel porque o rustup novo recusa
# "stable-x86_64-pc-windows-gnu" dentro do rust-toolchain.toml
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal
if ($LASTEXITCODE) { throw "rustup falhou" }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
Write-Host "==> LocalChat $version"

cargo build --release
if ($LASTEXITCODE) { throw "cargo build falhou" }
cargo test --release
if ($LASTEXITCODE) { throw "testes falharam" }

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 nao encontrado (instale de jrsoftware.org)" }
& $iscc "/DAppVersion=$version" setup.iss
if ($LASTEXITCODE) { throw "ISCC falhou" }
Write-Host "Pronto: dist\LocalChat-Setup.exe ($version)"
