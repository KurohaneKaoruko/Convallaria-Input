# Convallaria Input 安装脚本（任务 5.1）
# 需要管理员权限：写入 HKLM 注册表并注册 TSF 文本服务。
#Requires -RunAsAdministrator

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

# 1) 构建 DLL
Push-Location $root
cargo build --release -p convallaria-windows
if ($LASTEXITCODE -ne 0) { throw "构建失败" }
Pop-Location

$dll = Join-Path $root "target\release\convallaria_windows.dll"
if (-not (Test-Path $dll)) { throw "未找到 DLL: $dll" }

# 1.5) 部署词典到用户配置目录（引擎加载路径之一）
$dict = Join-Path $root "assets\dicts\convallaria.dict.bin"
if (Test-Path $dict) {
    $dest = Join-Path $env:APPDATA "Convallaria"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Copy-Item $dict (Join-Path $dest "dictionary.bin") -Force
    Write-Host "✔ 词典已部署到 $dest\dictionary.bin"
} else {
    Write-Warning "未找到词典 $dict —— 请先执行 dict build（见 README），输入法将退化为纯原文上屏"
}

# 2) COM 自注册（DllRegisterServer 为占位，注册表由本脚本写入）
& regsvr32 /s $dll
if ($LASTEXITCODE -ne 0) { throw "regsvr32 注册失败（检查管理员权限）" }

# 3) TSF 文本服务注册表项
$clsid   = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C001}"
$profile = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C002}"
# GUID_TFCAT_TIP_KEYBOARD（TSF 官方类别常量）
$catKeyboard = "{34745CFF-BF55-4F84-9AD5-5B36CE96EF02}"

New-Item -Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid" -Force | Out-Null
Set-ItemProperty -Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid" -Name "(default)" -Value "Convallaria Input"
New-Item -Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32" -Force | Out-Null
Set-ItemProperty -Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32" -Name "(default)" -Value $dll
Set-ItemProperty -Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32" -Name "ThreadingModel" -Value "Apartment"

New-Item -Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid" -Force | Out-Null
Set-ItemProperty -Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid" -Name "(default)" -Value "Convallaria Input"

$langProfile = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid\LanguageProfile\0x00000804\$profile"
New-Item -Path $langProfile -Force | Out-Null
Set-ItemProperty -Path $langProfile -Name "(default)" -Value "Convallaria"
Set-ItemProperty -Path $langProfile -Name "Enable" -Value 1 -Type DWord

New-Item -Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid\Category\$catKeyboard\$clsid" -Force | Out-Null

Write-Host "✔ Convallaria Input 已注册。请在 Windows 设置 > 时间和语言 > 语言 > 中文 > 选项 > 键盘中添加「Convallaria Input」，或在语言栏选择。"
Write-Host "  （MVP 骨架仅演示吃键与上屏原文链路，组字/候选窗在后续任务交付）"
