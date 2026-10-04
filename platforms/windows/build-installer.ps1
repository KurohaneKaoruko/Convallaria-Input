# 一键构建安装包：产出 dist\ConvallariaInput-Setup.exe（单文件，可直接分发）
$ErrorActionPreference = "Stop"
$root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))

# 1) 词典（不存在则构建）
$dict = Join-Path $root "assets\dicts\convallaria.dict.bin"
if (-not (Test-Path $dict)) {
    Write-Host "• 词典不存在，先构建…"
    Push-Location $root
    cargo run -p dict --release -- build `
        --luna assets\dicts\luna_pinyin.dict.yaml `
        --essay assets\dicts\essay.txt `
        --wubi assets\dicts\wubi86.dict.yaml `
        --t2s assets\dicts\t2s.txt `
        --out assets\dicts\convallaria.dict.bin
    if ($LASTEXITCODE -ne 0) { throw "词典构建失败" }
    Pop-Location
}

# 2) 输入法 DLL（release；独立 target 目录，规避已安装 DLL 的文件锁）
Write-Host "• 构建输入法 DLL…"
Push-Location $root
cargo build --release -p convallaria-windows --target-dir target\pack
if ($LASTEXITCODE -ne 0) { throw "DLL 构建失败" }
# 2.5) 后台服务进程（候选窗所在进程）
Write-Host "• 构建后台服务…"
cargo build --release -p convallaria-server --target-dir target\pack
if ($LASTEXITCODE -ne 0) { throw "服务构建失败" }
# 3) 安装器（嵌入 DLL 与词典）
Write-Host "• 构建安装器…"
cargo build --release -p convallaria-setup
if ($LASTEXITCODE -ne 0) { throw "安装器构建失败" }
Pop-Location

# 4) 产物
$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$setup = Join-Path $root "target\release\convallaria-setup.exe"
Copy-Item $setup (Join-Path $dist "ConvallariaInput-Setup.exe") -Force
$size = [math]::Round((Get-Item (Join-Path $dist "ConvallariaInput-Setup.exe")).Length / 1MB, 1)
Write-Host "✔ 安装包已生成：dist\ConvallariaInput-Setup.exe（$size MB）"
Write-Host "  双击运行 → UAC 确认 → 装完即用；卸载走控制面板「应用和功能」。"
