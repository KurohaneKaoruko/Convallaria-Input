# Convallaria Input 卸载脚本（任务 5.1）
#Requires -RunAsAdministrator

$ErrorActionPreference = "Continue"

$clsid   = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C001}"
$profile = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C002}"

$dll = Join-Path $PSScriptRoot "..\target\release\convallaria_windows.dll"
if (Test-Path $dll) {
    & regsvr32 /u /s $dll
}

Remove-Item -Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid" -Recurse -Force
Remove-Item -Path "HKLM:\SOFTWARE\Classes\CLSID\$clsid" -Recurse -Force

Write-Host "✔ Convallaria Input 已卸载。如语言列表仍有残留，请在 Windows 设置中手动移除后重新登录。"
