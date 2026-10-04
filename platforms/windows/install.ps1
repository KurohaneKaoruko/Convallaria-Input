# Convallaria Input 安装脚本（任务 5.1）
# 双击或普通 PowerShell 运行即可：自动弹出 UAC 提权，注册后自动启用语言配置，
# 无需手动到系统设置里添加键盘。
$ErrorActionPreference = "Stop"

# —— 自提升：非管理员时以管理员身份重启自身 ——
$identity = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $identity.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Start-Process powershell.exe "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Verb RunAs
    exit
}

# —— 工作区根目录（脚本位于 platforms\windows\ 下）——
$root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))

# 1) 构建 DLL（workspace 共享根 target\）
Push-Location $root
cargo build --release -p convallaria-windows
if ($LASTEXITCODE -ne 0) { throw "构建失败" }
Pop-Location

$dll = Join-Path $root "target\release\convallaria_windows.dll"
if (-not (Test-Path $dll)) { throw "未找到 DLL: $dll" }

# 2) 部署词典到用户配置目录（引擎加载路径之一）
$dict = Join-Path $root "assets\dicts\convallaria.dict.bin"
if (Test-Path $dict) {
    $dest = Join-Path $env:APPDATA "Convallaria"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Copy-Item $dict (Join-Path $dest "dictionary.bin") -Force
    Write-Host "✔ 词典已部署到 $dest\dictionary.bin"
} else {
    Write-Warning "未找到词典 $dict —— 请先执行 dict build（见 README），输入法将退化为纯原文上屏"
}

# 3) COM 自注册
& regsvr32 /s $dll
if ($LASTEXITCODE -ne 0) { throw "regsvr32 注册失败" }
Write-Host "✔ 文本服务已注册"

# 4) TSF 注册表项（CLSID / InprocServer32 / CTF\TIP / 类别）
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
Write-Host "✔ 注册表项已写入"

# 5) 为当前用户启用语言配置（免去手动到设置里添加键盘）
& rundll32.exe "$dll,EnableProfileForCurrentUser"
Write-Host ""
Write-Host "✔ Convallaria Input 安装完成！按 Win+空格 或点击任务栏语言「中」图标即可切换。"
Write-Host "  卸载：platforms\windows\uninstall.ps1"
