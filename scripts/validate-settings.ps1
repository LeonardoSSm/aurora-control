<#
.SYNOPSIS
  Start-up validation of preferences, monitor fallback, provider visibility
  and the single-instance guard, against a real build.

.DESCRIPTION
  Writes preference files in the format 0.2.0 saved (version 1), so every
  start also exercises the migration to version 2, starts the app in demo
  mode and measures the rail through Win32. The files live in a temporary
  AURORA_CONFIG_DIR (debug builds only), never in the user's %APPDATA%. Run
  with PowerShell 7 (pwsh) in an interactive session.
#>
param([Parameter(Mandatory)] [string] $Exe)

$ErrorActionPreference = 'Stop'
# Debug builds: a separate single-instance name, so validation runs beside an
# installed copy that may be running, without stopping it.
$env:AURORA_INSTANCE = 'validation'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Win {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hwnd, System.Text.StringBuilder text, int max);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr hwnd, int index);
    public static List<IntPtr> WindowsOf(uint pid) {
        var found = new List<IntPtr>();
        EnumWindows((hwnd, _) => { uint owner; GetWindowThreadProcessId(hwnd, out owner); if (owner == pid) found.Add(hwnd); return true; }, IntPtr.Zero);
        return found;
    }
    public static string Title(IntPtr hwnd) { var text = new System.Text.StringBuilder(256); GetWindowTextW(hwnd, text, 256); return text.ToString(); }
}
'@

$results = [ordered]@{}
function Check([string] $name, [bool] $ok, [string] $detail = '') {
    $results[$name] = $ok
    Write-Output ("{0}  {1}  {2}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $name, $detail)
}

# Debug builds keep preferences in AURORA_CONFIG_DIR: the user's real
# %APPDATA% folder is never read or written by this script.
$dir = Join-Path ([IO.Path]::GetTempPath()) ('aurora-config-' + [guid]::NewGuid())
$env:AURORA_CONFIG_DIR = $dir
$file = Join-Path $dir 'preferences.json'
$backup = $null
$existed = $false

$profile = Join-Path ([IO.Path]::GetTempPath()) ('aurora-settings-' + [guid]::NewGuid())
$env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $profile 'webview'
$env:AURORA_DEMO = '73,25'
Remove-Item Env:AURORA_SIDE -ErrorAction SilentlyContinue

function Write-Preferences([string] $side, $monitor, [bool] $codex, [bool] $claude) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    [ordered]@{
        version = 1; side = $side; monitor = $monitor
        providers = [ordered]@{ codex = $codex; claudeCode = $claude }
        launchAtStartup = $false
    } | ConvertTo-Json -Depth 4 | Set-Content -Path $file -Encoding utf8NoBOM
}

function Start-Rail {
    $process = Start-Process -FilePath $Exe -PassThru -WindowStyle Minimized
    $rail = [IntPtr]::Zero
    for ($i = 0; $i -lt 100; $i++) {
        Start-Sleep -Milliseconds 100
        foreach ($hwnd in [Win]::WindowsOf([uint32] $process.Id)) {
            if ([Win]::Title($hwnd) -eq 'Aurora Control' -and [Win]::IsWindowVisible($hwnd)) { $rail = $hwnd }
        }
        if ($rail -ne [IntPtr]::Zero) { break }
    }
    Start-Sleep -Milliseconds 300
    $rect = New-Object Win+RECT
    [Win]::GetWindowRect($rail, [ref] $rect) | Out-Null
    [pscustomobject]@{ Process = $process; Rail = $rail; Rect = $rect }
}

function Stop-Rail($run) {
    Stop-Process -Id $run.Process.Id -ErrorAction SilentlyContinue
    Wait-Process -Id $run.Process.Id -Timeout 5 -ErrorAction SilentlyContinue
}

Add-Type -AssemblyName System.Windows.Forms
$primary = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea

try {
    # 1. Defaults without a file: right side, both providers.
    Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
    $run = Start-Rail
    Check 'defaults: right edge' ($run.Rect.Right -eq $primary.Right) ("x {0}..{1}" -f $run.Rect.Left, $run.Rect.Right)
    Check 'defaults: two indicators' (($run.Rect.Bottom - $run.Rect.Top) -gt 100) ("height {0}" -f ($run.Rect.Bottom - $run.Rect.Top))

    # 2. Single instance: a second launch exits and leaves one rail.
    $second = Start-Process -FilePath $Exe -PassThru -WindowStyle Minimized
    $exited = $second.WaitForExit(5000)
    Check 'second instance exits' $exited ("exit code {0}" -f $(if ($exited) { $second.ExitCode } else { 'running' }))
    $rails = @([Win]::WindowsOf([uint32] $run.Process.Id) | Where-Object { [Win]::Title($_) -eq 'Aurora Control' })
    Check 'first instance keeps running' (-not $run.Process.HasExited -and $rails.Count -eq 1)
    Stop-Rail $run
    if (-not $exited) { Stop-Process -Id $second.Id -ErrorAction SilentlyContinue }

    # 3. Saved side survives a restart.
    Write-Preferences 'left' $null $true $true
    $run = Start-Rail
    Check 'saved side: left edge' ($run.Rect.Left -eq $primary.Left) ("x {0}" -f $run.Rect.Left)
    Stop-Rail $run

    # 4. One visible provider: a single-circle rail.
    Write-Preferences 'right' $null $false $true
    $run = Start-Rail
    $height = $run.Rect.Bottom - $run.Rect.Top
    $scale = ($run.Rect.Right - $run.Rect.Left) / 60.0
    Check 'one provider: single circle' ([math]::Abs($height - 52 * $scale) -le 1) "height $height"
    Stop-Rail $run

    # 5. Disconnected monitor: primary fallback, preference kept.
    $ghost = [ordered]@{ id = 'm-00000000deadbeef'; name = 'Monitor removido' }
    Write-Preferences 'right' $ghost $true $true
    $run = Start-Rail
    Check 'monitor fallback: primary' ($run.Rect.Right -eq $primary.Right) ("x {0}..{1}" -f $run.Rect.Left, $run.Rect.Right)
    Stop-Rail $run
    $kept = (Get-Content $file -Raw | ConvertFrom-Json).monitor.id
    Check 'monitor fallback: choice kept' ($kept -eq 'm-00000000deadbeef')

    # 6. Corrupted file: defaults, file set aside, app runs.
    Set-Content -Path $file -Value '{"version": 1, "side": "lef' -Encoding utf8NoBOM
    $run = Start-Rail
    Check 'corrupted: app starts on defaults' ($run.Rect.Right -eq $primary.Right)
    Stop-Rail $run
    $aside = @(Get-ChildItem $dir -Filter 'preferences.corrupt-*.json')
    Check 'corrupted: original kept aside' ($aside.Count -eq 1) ($aside.Name -join ', ')
    Check 'corrupted: no partial write' (-not (Test-Path $file) -or ((Get-Content $file -Raw) -notmatch '"lef$'))

    # 7. Newer version: left untouched.
    $future = '{"version": 9, "side": "left", "theme": "neon"}'
    Set-Content -Path $file -Value $future -Encoding utf8NoBOM -NoNewline
    $run = Start-Rail
    Check 'newer version: defaults in use' ($run.Rect.Right -eq $primary.Right)
    Stop-Rail $run
    Check 'newer version: file untouched' ((Get-Content $file -Raw) -eq $future)

    # 8. Environment override beats the preference for the session only.
    Write-Preferences 'right' $null $true $true
    $env:AURORA_SIDE = 'left'
    $run = Start-Rail
    Check 'AURORA_SIDE overrides' ($run.Rect.Left -eq $primary.Left)
    Stop-Rail $run
    Remove-Item Env:AURORA_SIDE
    Check 'override not persisted' ((Get-Content $file -Raw | ConvertFrom-Json).side -eq 'right')
} finally {
    Get-Process aurora-control -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Resolve-Path $Exe).Path } | Stop-Process
    Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
    if ($existed) { Copy-Item $backup $dir -Recurse; Remove-Item $backup -Recurse -Force }
    Remove-Item $profile -Recurse -Force -ErrorAction SilentlyContinue
}

$failed = @($results.Values | Where-Object { -not $_ }).Count
Write-Output "Summary: $($results.Count - $failed)/$($results.Count) passed"
exit $failed
