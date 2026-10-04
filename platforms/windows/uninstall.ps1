# Convallaria Input 卸载脚本（任务 5.1）
# 卸载脚本：自动弹出 UAC 提权
$identity = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $identity.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Start-Process powershell.exe "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Verb RunAs
    exit
}

$ErrorActionPreference = "Continue"

$clsid   = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C001}"
$profile = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C002}"

$dll = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\..\target\release\convallaria_windows.dll"))
if (Test-Path $dll) {
    & regsvr32 /u /s $dll
}

Remove-Item -Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid" -Recurse -Force
Remove-Item -Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid" -Recurse -Force

Write-Host "✔ Convallaria Input 已卸载。如语言列表仍有残留，请在 Windows 设置中手动移除后重新登录。"
