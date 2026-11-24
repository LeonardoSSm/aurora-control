<#
.SYNOPSIS
  Interactive Win32 validation of the rail and summary overlays.

.DESCRIPTION
  Starts a debug build in demo mode (no account data), drives the real cursor
  with SetCursorPos and reports window geometry, visibility, the foreground
  window, hit testing and the controller trace (AURORA_TRACE=1). Requires an
  interactive desktop session. Run with PowerShell 7 (pwsh).

.EXAMPLE
  pwsh -File scripts/validate-overlay.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Side right
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [ValidateSet('left', 'right')] [string] $Side = 'right',
    [string] $Demo = '73,25'
)

$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Win {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hwnd, System.Text.StringBuilder text, int max);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT point);
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
    $results[$name] = [pscustomobject]@{ ok = $ok; detail = $detail }
    $mark = if ($ok) { 'PASS' } else { 'FAIL' }
    Write-Output ("{0}  {1}  {2}" -f $mark, $name, $detail)
}

$profile = Join-Path ([IO.Path]::GetTempPath()) ("aurora-validate-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $profile | Out-Null
$log = Join-Path $profile 'stderr.log'
$env:AURORA_DEMO = $Demo
$env:AURORA_SIDE = $Side
$env:AURORA_TRACE = '1'
$env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $profile 'webview'
$env:AURORA_CONFIG_DIR = Join-Path $profile 'config'
$before = [Win]::GetForegroundWindow()
$original = New-Object Win+POINT
[Win]::GetCursorPos([ref] $original) | Out-Null
$process = Start-Process -FilePath $Exe -PassThru -RedirectStandardError $log
try {
    $rail = [IntPtr]::Zero; $summary = [IntPtr]::Zero
    for ($i = 0; $i -lt 100 -and ($rail -eq [IntPtr]::Zero -or -not [Win]::IsWindowVisible($rail)); $i++) {
        Start-Sleep -Milliseconds 100
        foreach ($hwnd in [Win]::WindowsOf([uint32] $process.Id)) {
            switch ([Win]::Title($hwnd)) {
                'Aurora Control' { $rail = $hwnd }
                'Aurora Control — Resumo' { $summary = $hwnd }
            }
        }
    }
    Check 'rail visible' ([Win]::IsWindowVisible($rail)) "hwnd $rail"
    # Baseline after start-up: a debug build also opens its console window.
    Start-Sleep -Milliseconds 500
    $before = [Win]::GetForegroundWindow()
    function Foreground { $hwnd = [Win]::GetForegroundWindow(); [uint32] $pid2 = 0; [Win]::GetWindowThreadProcessId($hwnd, [ref] $pid2) | Out-Null; "fg $hwnd pid $pid2 overlay=$($hwnd -eq $rail -or $hwnd -eq $summary)" }
    Check 'summary hidden at start' (-not [Win]::IsWindowVisible($summary))
    $rect = New-Object Win+RECT
    [Win]::GetWindowRect($rail, [ref] $rect) | Out-Null
    $width = $rect.Right - $rect.Left
    Check 'rail geometry' ($width -gt 0) ("({0},{1}) {2}x{3}" -f $rect.Left, $rect.Top, $width, ($rect.Bottom - $rect.Top))
    $exStyle = [Win]::GetWindowLongW($rail, -20)
    Check 'no-activate tool window' ((($exStyle -band 0x08000000) -ne 0) -and (($exStyle -band 0x80) -ne 0)) ('0x{0:X}' -f $exStyle)

    # Collapsed indicator center: on the screen edge, half visible.
    $scale = $width / 60.0
    $edgeX = if ($Side -eq 'right') { $rect.Right - [int](6 * $scale) } else { $rect.Left + [int](6 * $scale) }
    $codexY = $rect.Top + [int](26 * $scale)
    $away = if ($Side -eq 'right') { $edgeX - [int](400 * $scale) } else { $edgeX + [int](400 * $scale) }

    # Hover opens after the intent delay, without stealing focus.
    Check 'cursor moved' ([Win]::SetCursorPos($edgeX, $codexY)) "($edgeX,$codexY)"
    function WaitVisible([int] $limit) { $watch = [Diagnostics.Stopwatch]::StartNew(); while ($watch.ElapsedMilliseconds -lt $limit) { if ([Win]::IsWindowVisible($summary)) { return $watch.ElapsedMilliseconds }; Start-Sleep -Milliseconds 10 }; return -1 }
    $cold = WaitVisible 2000
    Check 'summary opens on hover (cold)' ($cold -ge 0) "$cold ms after the cursor arrived"
    Check 'focus unchanged on hover' ([Win]::GetForegroundWindow() -eq $before) (Foreground)
    $card = New-Object Win+RECT
    [Win]::GetWindowRect($summary, [ref] $card) | Out-Null
    $inside = if ($Side -eq 'right') { $card.Right -le $rect.Right } else { $card.Left -ge $rect.Left }
    Check 'summary opens inward' $inside ("({0},{1})-({2},{3})" -f $card.Left, $card.Top, $card.Right, $card.Bottom)

    # The card is click-through: hit testing finds the window beneath it.
    $point = New-Object Win+POINT
    $point.X = [int](($card.Left + $card.Right) / 2); $point.Y = [int](($card.Top + $card.Bottom) / 2)
    Check 'summary click-through' ([Win]::WindowFromPoint($point) -ne $summary)

    # Leaving closes after the 240 ms grace; measured, not assumed.
    [Win]::SetCursorPos($away, $codexY) | Out-Null
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ([Win]::IsWindowVisible($summary) -and $watch.ElapsedMilliseconds -lt 2000) { Start-Sleep -Milliseconds 10 }
    $closed = -not [Win]::IsWindowVisible($summary)
    Check 'closes after leaving' $closed "$($watch.ElapsedMilliseconds) ms after the cursor left"
    Start-Sleep -Milliseconds 200
    # Warm open: the WebViews are initialized.
    [Win]::SetCursorPos($edgeX, $codexY) | Out-Null
    $warm = WaitVisible 2000
    Check 'summary opens on hover (warm)' (($warm -ge 0) -and ($warm -lt 600)) "$warm ms after the cursor arrived"
    [Win]::SetCursorPos($away, $codexY) | Out-Null
    Start-Sleep -Milliseconds 700
    [Win]::SetCursorPos($edgeX, $codexY) | Out-Null
    Start-Sleep -Milliseconds 40
    [Win]::SetCursorPos($away, $codexY) | Out-Null
    Start-Sleep -Milliseconds 500
    Check 'sweep does not open' (-not [Win]::IsWindowVisible($summary))

    # Transparent rail corner: clicks fall through to the window beneath.
    $corner = New-Object Win+POINT
    $corner.X = if ($Side -eq 'right') { $rect.Left + 1 } else { $rect.Right - 2 }
    $corner.Y = $rect.Top + 1
    Check 'rail corner click-through' ([Win]::WindowFromPoint($corner) -ne $rail)

    # Switching indicators keeps one card open, for the second provider.
    [Win]::SetCursorPos($edgeX, $codexY) | Out-Null
    Start-Sleep -Milliseconds 600
    [Win]::SetCursorPos($edgeX, $codexY + [int](66 * $scale)) | Out-Null
    Start-Sleep -Milliseconds 700
    Check 'switch keeps the card open' ([Win]::IsWindowVisible($summary))
    Check 'focus unchanged after switch' ([Win]::GetForegroundWindow() -eq $before) (Foreground)
} finally {
    [Win]::SetCursorPos($original.X, $original.Y) | Out-Null
    Stop-Process -Id $process.Id -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 300
}

$trace = Get-Content $log -ErrorAction SilentlyContinue
# Only complete lines: the process is stopped by this script, which can cut
# the last line of stderr in the middle.
$batches = @($trace | Where-Object { $_ -match '^\[aurora\] batch .* depth \d+$' })
$foreign = @($batches | Where-Object { $_ -notmatch 'depth 1$' })
$owners = @($batches | ForEach-Object { if ($_ -match 'thread (ThreadId\(\d+\)) owner (ThreadId\(\d+\))') { $matches[1] -eq $matches[2] } })
Check 'controller batches traced' ($batches.Count -gt 0) "$($batches.Count) batches"
Check 'no re-entrant batch' ($foreign.Count -eq 0)
Check 'single owner thread' (-not ($owners -contains $false))
$waits = @($batches | ForEach-Object { if ($_ -match 'waited ([\d.]+)(µs|ms|s|ns)') { switch ($matches[2]) { 'ns' { [double]$matches[1] / 1e6 } 'µs' { [double]$matches[1] / 1e3 } 'ms' { [double]$matches[1] } 's' { [double]$matches[1] * 1e3 } } } })
if ($waits.Count) {
    $sorted = $waits | Sort-Object
    Check 'batch latency' $true ("median {0:N2} ms, max {1:N2} ms" -f $sorted[[int]($sorted.Count / 2)], $sorted[-1])
}
$failed = @($results.Values | Where-Object { -not $_.ok }).Count
Write-Output "Summary: $($results.Count - $failed)/$($results.Count) passed"
if ($failed) { Copy-Item $log (Join-Path ([IO.Path]::GetTempPath()) "aurora-validate-last.log") }
Remove-Item -Recurse -Force $profile -ErrorAction SilentlyContinue
exit $failed
