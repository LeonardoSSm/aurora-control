<#
.SYNOPSIS
  Interactive Win32 validation of the rail with zero to four indicators.

.DESCRIPTION
  For each scenario, starts a debug build in demo mode (no account data, no
  provider file read) with an isolated %APPDATA% seeded with version 2
  preferences, then checks:
  - the rail is hidden with zero providers and the process keeps running;
  - the window height matches the number of indicators and is vertically
    centered in the work area;
  - the Win32 region: each indicator center hits the rail, the gap between
    two indicators and the corners fall through;
  - hovering each slot opens the summary inward, next to that slot, without
    taking focus;
  - disabling a provider while its summary is open (through the settings
    window, driven over the WebView2 DevTools endpoint) closes the card and
    re-lays the rail out with one indicator less.
  Requires an interactive desktop session, PowerShell 7 (pwsh) and Node 22+.

.EXAMPLE
  pwsh -File scripts/validate-providers.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Side right
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [ValidateSet('left', 'right')] [string] $Side = 'right',
    [int] $Port = 9334
)

$ErrorActionPreference = 'Stop'
# Debug builds: a separate single-instance name, so validation runs beside an
# installed copy that may be running, without stopping it.
$env:AURORA_INSTANCE = 'validation'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Rail {
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
    [DllImport("user32.dll")] public static extern bool SystemParametersInfoW(uint action, uint param, out RECT rect, uint ini);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
    public static List<IntPtr> WindowsOf(uint pid) {
        var found = new List<IntPtr>();
        EnumWindows((hwnd, _) => { uint owner; GetWindowThreadProcessId(hwnd, out owner); if (owner == pid) found.Add(hwnd); return true; }, IntPtr.Zero);
        return found;
    }
    public static string Title(IntPtr hwnd) { var text = new System.Text.StringBuilder(256); GetWindowTextW(hwnd, text, 256); return text.ToString(); }
    public static RECT WorkArea() { RECT rect; SystemParametersInfoW(0x30, 0, out rect, 0); return rect; }
    // The top-level window under a point: WindowFromPoint returns the WebView2
    // child window, so compare its root (GA_ROOT).
    public static IntPtr At(int x, int y) { var point = new POINT { X = x, Y = y }; return GetAncestor(WindowFromPoint(point), 2); }
}
'@

$results = [System.Collections.Generic.List[object]]::new()
function Check([string] $name, [bool] $ok, [string] $detail = '') {
    $results.Add([pscustomobject]@{ name = $name; ok = $ok })
    # Write-Host: a function that checks must not return its messages.
    Write-Host ("{0}  {1}  {2}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $name, $detail)
}

$Exe = (Resolve-Path $Exe).Path
$cdp = Join-Path $PSScriptRoot 'cdp-eval.mjs'
$all = @('codex', 'claudeCode', 'antigravity', 'grok')
$original = New-Object Rail+POINT
[Rail]::GetCursorPos([ref] $original) | Out-Null
$work = [Rail]::WorkArea()

function Preferences([string[]] $enabled) {
    $providers = [ordered]@{}
    foreach ($id in $all) { $providers[$id] = [ordered]@{ enabled = $enabled -contains $id; metric = $null; alerts = $true } }
    [ordered]@{
        version = 3; side = $Side; monitor = $null; providers = $providers
        launchAtStartup = $false; railVisible = $true
        notifications = [ordered]@{
            enabled = $false; thresholds = @(85, 95, 100); renewal = $true; auth = $true; reconnect = $false
            quiet = [ordered]@{ enabled = $false; start = 1320; end = 420 }
        }
    } | ConvertTo-Json -Depth 5
}

function Start-Rail([string[]] $enabled, [switch] $Settings) {
    $profile = Join-Path ([IO.Path]::GetTempPath()) ('aurora-providers-' + [guid]::NewGuid())
    # Debug builds read and write preferences here instead of the user's
    # %APPDATA% folder (Tauri ignores the APPDATA variable).
    $config = Join-Path $profile 'config'
    New-Item -ItemType Directory -Path $config | Out-Null
    Set-Content -Path (Join-Path $config 'preferences.json') -Value (Preferences $enabled) -Encoding utf8NoBOM
    $env:AURORA_CONFIG_DIR = $config
    $env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $profile 'webview'
    $env:AURORA_DEMO = '64,37,82,21'
    $env:AURORA_SIDE = $Side
    if ($Settings) {
        $env:AURORA_OPEN_SETTINGS = '1'
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
    } else {
        Remove-Item Env:AURORA_OPEN_SETTINGS, Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
    }
    $process = Start-Process -FilePath $Exe -PassThru -RedirectStandardError (Join-Path $profile 'stderr.log')
    $windows = @{ rail = [IntPtr]::Zero; summary = [IntPtr]::Zero }
    for ($i = 0; $i -lt 100; $i++) {
        Start-Sleep -Milliseconds 100
        foreach ($hwnd in [Rail]::WindowsOf([uint32] $process.Id)) {
            switch ([Rail]::Title($hwnd)) {
                'Aurora Control' { $windows.rail = $hwnd }
                'Aurora Control — Resumo' { $windows.summary = $hwnd }
            }
        }
        if ($windows.rail -ne [IntPtr]::Zero -and $windows.summary -ne [IntPtr]::Zero -and ($enabled.Count -eq 0 -or [Rail]::IsWindowVisible($windows.rail))) { break }
    }
    Start-Sleep -Milliseconds 800
    [pscustomobject]@{ process = $process; profile = $profile; rail = $windows.rail; summary = $windows.summary }
}

function Stop-Rail($run) {
    Stop-Process -Id $run.process.Id -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 400
    Remove-Item -Recurse -Force $run.profile -ErrorAction SilentlyContinue
}

function WaitVisible([IntPtr] $hwnd, [int] $limit, [bool] $want = $true) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $limit) {
        if ([Rail]::IsWindowVisible($hwnd) -eq $want) { return $watch.ElapsedMilliseconds }
        Start-Sleep -Milliseconds 10
    }
    return -1
}

# Moves the cursor to (x, y) from 96 px inside the screen, in small steps.
function Approach([int] $x, [int] $y) {
    $inward = if ($Side -eq 'right') { -1 } else { 1 }
    for ($step = 12; $step -ge 0; $step--) {
        [Rail]::SetCursorPos($x + $inward * $step * 8, $y) | Out-Null
        Start-Sleep -Milliseconds 12
    }
}

function Rect([IntPtr] $hwnd) { $rect = New-Object Rail+RECT; [Rail]::GetWindowRect($hwnd, [ref] $rect) | Out-Null; $rect }

function Check-Layout($run, [string[]] $enabled, [string] $tag) {
    $count = $enabled.Count
    $rect = Rect $run.rail
    $width = $rect.Right - $rect.Left
    $height = $rect.Bottom - $rect.Top
    $scale = $width / 60.0
    $expected = [math]::Round(($count * 66 - 14) * $scale)
    Check "$tag height" ([math]::Abs($height - $expected) -le 1) "${height}px, expected $expected"
    $above = $rect.Top - $work.Top
    $below = $work.Bottom - $rect.Bottom
    Check "$tag centered" ([math]::Abs($above - $below) -le 1) "above $above, below $below"
    $edgeX = if ($Side -eq 'right') { $rect.Right - [int](6 * $scale) } else { $rect.Left + [int](6 * $scale) }
    $hits = 0; $gaps = 0
    for ($slot = 0; $slot -lt $count; $slot++) {
        $centerY = $rect.Top + [int](($slot * 66 + 26) * $scale)
        if ([Rail]::At($edgeX, $centerY) -eq $run.rail) { $hits++ }
        if ($slot -lt $count - 1) {
            $gapY = $rect.Top + [int](($slot * 66 + 52 + 7) * $scale)
            if ([Rail]::At($edgeX, $gapY) -ne $run.rail) { $gaps++ }
        }
    }
    Check "$tag region" (($hits -eq $count) -and ($gaps -eq [math]::Max(0, $count - 1))) "$hits/$count circles hit, $gaps/$([math]::Max(0, $count - 1)) gaps pass through"
    return [pscustomobject]@{ rect = $rect; scale = $scale; edgeX = $edgeX }
}

$away = if ($Side -eq 'right') { $work.Right - 600 } else { $work.Left + 600 }

# The user's real preferences must come out of this run untouched.
$real = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'app.auroracontrol.desktop\preferences.json'
function Fingerprint { if (Test-Path $real) { (Get-FileHash $real).Hash } else { 'absent' } }
$realBefore = Fingerprint

try {
    # Zero providers: hidden rail, live process, tray still available.
    $run = Start-Rail @()
    try {
        Start-Sleep -Milliseconds 1500
        Check '0 · rail hidden' (-not [Rail]::IsWindowVisible($run.rail)) "hwnd $($run.rail)"
        Check '0 · process alive' (-not $run.process.HasExited)
    } finally { Stop-Rail $run }

    $scenarios = @(
        , @('codex')
        , @('codex', 'claudeCode')
        , @('codex', 'claudeCode', 'antigravity')
        , @('codex', 'claudeCode', 'antigravity', 'grok')
        , @('codex', 'grok')
    )
    foreach ($enabled in $scenarios) {
        $tag = "$($enabled.Count) [$($enabled -join ',')]"
        $run = Start-Rail $enabled
        try {
            Check "$tag · rail visible" ([Rail]::IsWindowVisible($run.rail))
            $layout = Check-Layout $run $enabled $tag
            $before = [Rail]::GetForegroundWindow()
            for ($slot = 0; $slot -lt $enabled.Count; $slot++) {
                $centerY = $layout.rect.Top + [int](($slot * 66 + 26) * $layout.scale)
                [Rail]::SetCursorPos($away, $centerY) | Out-Null
                Start-Sleep -Milliseconds 450
                [Rail]::SetCursorPos($layout.edgeX, $centerY) | Out-Null
                $opened = WaitVisible $run.summary 2500
                $card = Rect $run.summary
                $cardCenter = ($card.Top + $card.Bottom) / 2
                $inward = if ($Side -eq 'right') { $card.Right -le $layout.rect.Right } else { $card.Left -ge $layout.rect.Left }
                $inside = ($card.Top -ge $work.Top) -and ($card.Bottom -le $work.Bottom)
                # The card is centered on its slot unless the work area clamps it.
                $aligned = ([math]::Abs($cardCenter - $centerY) -le 2) -or ($card.Top -eq $work.Top) -or ($card.Bottom -eq $work.Bottom)
                Check "$tag · hover slot $slot ($($enabled[$slot]))" (($opened -ge 0) -and $inward -and $inside -and $aligned) "opened in $opened ms, card center $cardCenter vs slot $centerY"
            }
            Check "$tag · focus unchanged" ([Rail]::GetForegroundWindow() -eq $before)
            [Rail]::SetCursorPos($away, $layout.rect.Top) | Out-Null
            Check "$tag · closes after leaving" ((WaitVisible $run.summary 2000 $false) -ge 0)
        } finally { Stop-Rail $run }
    }

    # Disabling the hovered provider (Antigravity, slot 2) from the settings window.
    $enabled = @('codex', 'claudeCode', 'antigravity', 'grok')
    $run = Start-Rail $enabled -Settings
    try {
        $layout = Check-Layout $run $enabled 'live change · before'
        $slot = 2
        $centerY = $layout.rect.Top + [int](($slot * 66 + 26) * $layout.scale)
        [Rail]::SetCursorPos($layout.edgeX, $centerY) | Out-Null
        Check 'live change · antigravity card open' ((WaitVisible $run.summary 2500) -ge 0)
        $script = @'
(async () => {
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  for (let i = 0; i < 100 && !window.__TAURI_INTERNALS__; i++) await wait(100);
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const view = await invoke("get_settings");
  const preferences = structuredClone(view.preferences);
  preferences.providers.antigravity.enabled = false;
  const saved = await invoke("set_preferences", { preferences });
  return saved.preferences.providers.antigravity.enabled;
})()
'@
        $answer = $script | node $cdp $Port 'settings.html' - 2>&1
        Check 'live change · saved' ("$answer" -match 'false') "$answer"
        Check 'live change · card closed' ((WaitVisible $run.summary 2000 $false) -ge 0)
        Start-Sleep -Milliseconds 600
        $after = @('codex', 'claudeCode', 'grok')
        $layout = Check-Layout $run $after 'live change · after'
        # Slot 2 is now Grok: hovering it opens a card again.
        [Rail]::SetCursorPos($away, $centerY) | Out-Null
        Start-Sleep -Milliseconds 450
        $centerY = $layout.rect.Top + [int](($slot * 66 + 26) * $layout.scale)
        # A continuous approach, like a real mouse: right after a region change
        # the WebView2 consumes the first synthetic move (see the audit report).
        Approach $layout.edgeX $centerY
        Check 'live change · new slot 2 opens' ((WaitVisible $run.summary 2500) -ge 0)
        Check 'live change · process alive' (-not $run.process.HasExited)
    } finally { Stop-Rail $run }
} finally {
    [Rail]::SetCursorPos($original.X, $original.Y) | Out-Null
}
Check 'real preferences untouched' ((Fingerprint) -eq $realBefore)

$failed = @($results | Where-Object { -not $_.ok }).Count
Write-Output "Summary: $($results.Count - $failed)/$($results.Count) passed"
exit $failed
