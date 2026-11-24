<#
.SYNOPSIS
  Sends Aurora Control's test notification and captures the corner of the
  primary screen where Windows shows toasts.

.DESCRIPTION
  Starts the given build (demo mode, isolated preferences for debug builds,
  settings window open, WebView2 DevTools on 127.0.0.1), calls
  `send_test_notification` from the settings window and saves a PNG of the
  bottom-right corner of the primary screen. Only that corner is captured.
  For an installed build, pass its executable: the toast then carries the
  installed app identity (AppUserModelID set by the NSIS shortcut).

.EXAMPLE
  pwsh -File scripts/capture-toast.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Out toast.png
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Out,
    [int] $Port = 9341,
    [int] $Width = 460,
    [int] $Height = 300
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$cdp = Join-Path $PSScriptRoot 'cdp-eval.mjs'
$profile = Join-Path ([IO.Path]::GetTempPath()) ('aurora-toast-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $profile | Out-Null
$env:AURORA_CONFIG_DIR = Join-Path $profile 'config'
$env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $profile 'webview'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
$env:AURORA_OPEN_SETTINGS = '1'
$env:AURORA_DEMO = '64,37,82,21'
$process = Start-Process -FilePath (Resolve-Path $Exe).Path -PassThru
try {
    Start-Sleep -Seconds 6
    $result = '(async () => { try { await window.__TAURI_INTERNALS__.invoke("send_test_notification"); return "delivered to Windows"; } catch (e) { return "error: " + e; } })()' |
        node $cdp $Port 'settings.html' -
    Write-Output "send_test_notification: $result"
    Start-Sleep -Milliseconds 1800
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = New-Object System.Drawing.Bitmap $Width, $Height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($screen.Right - $Width, $screen.Bottom - $Height, 0, 0, $bitmap.Size)
    $bitmap.Save((Join-Path (Get-Location) $Out), [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose(); $bitmap.Dispose()
    Write-Output "captured $Out"
} finally {
    Stop-Process -Id $process.Id -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 400
    Remove-Item -Recurse -Force $profile -ErrorAction SilentlyContinue
}
