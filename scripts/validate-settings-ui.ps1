<#
.SYNOPSIS
  Drives the real settings window of a debug build: saving through the UI,
  persistence across a restart, start with Windows and its rollback.

.DESCRIPTION
  Starts a debug build in demo mode (no account data) with the settings
  window open (AURORA_OPEN_SETTINGS=1) and the WebView2 DevTools endpoint on
  127.0.0.1, then clicks the actual controls through scripts/cdp-eval.mjs.
  Checks the saved file, the per-user Run value and the rail geometry.

  The preferences folder is backed up and restored. The Run value
  "Aurora Control" is recorded first and put back exactly as it was (absent
  or with its previous command). Run with PowerShell 7 (pwsh) and Node 22+
  in an interactive session.
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [int] $Port = 9333
)

$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class WinUi {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hwnd, System.Text.StringBuilder text, int max);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
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

$Exe = (Resolve-Path $Exe).Path
$cdp = Join-Path $PSScriptRoot 'cdp-eval.mjs'
$dir = Join-Path $env:APPDATA 'app.auroracontrol.desktop'
$file = Join-Path $dir 'preferences.json'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$value = 'Aurora Control'
$expected = '"{0}"' -f $Exe

function Read-Run { (Get-ItemProperty $runKey -Name $value -ErrorAction SilentlyContinue).$value }
function Read-Saved { Get-Content $file -Raw | ConvertFrom-Json }

$runBefore = Read-Run
$backup = Join-Path ([IO.Path]::GetTempPath()) ('aurora-prefs-backup-' + [guid]::NewGuid())
$existed = Test-Path $dir
if ($existed) { Copy-Item $dir $backup -Recurse }
$profile = Join-Path ([IO.Path]::GetTempPath()) ('aurora-settings-ui-' + [guid]::NewGuid())
$env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $profile 'webview'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
$env:AURORA_DEMO = '73,25'
$env:AURORA_OPEN_SETTINGS = '1'
Remove-Item Env:AURORA_SIDE -ErrorAction SilentlyContinue

# Helpers evaluated in the settings page.
$prelude = @'
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const control = (label) => document.querySelector(`[aria-label="${label}"]`);
const button = (text) => [...document.querySelectorAll("button")].find((b) => b.textContent === text);
const ready = async () => { for (let i = 0; i < 100 && !control("Iniciar com o Windows"); i++) await wait(100); };
const settled = async () => {
  await wait(150);
  for (let i = 0; i < 100 && control("Iniciar com o Windows")?.disabled; i++) await wait(100);
  await wait(250);
};
const state = () => ({
  version: document.querySelector(".chip")?.textContent ?? null,
  side: document.querySelector('[role="radio"][aria-checked="true"]')?.textContent ?? null,
  startup: control("Iniciar com o Windows")?.getAttribute("aria-checked") ?? null,
  codex: control("Mostrar Codex")?.getAttribute("aria-checked") ?? null,
  claude: control("Mostrar Claude Code")?.getAttribute("aria-checked") ?? null,
  error: document.querySelector(".settings-error")?.textContent ?? null,
  alerts: [...document.querySelectorAll('[role="alert"]')].map((e) => e.textContent).join(" | "),
  fix: Boolean(button("Corrigir")),
});
await ready();
'@

function Invoke-Page([string] $body) {
    $expression = "(async () => {`n$prelude`n$body`n})()"
    $output = $expression | node $cdp $Port 'settings' -
    if ($LASTEXITCODE -ne 0) { throw "cdp-eval failed: $output" }
    $output | ConvertFrom-Json
}

function Start-App {
    $process = Start-Process -FilePath $Exe -PassThru -WindowStyle Minimized
    $rail = [IntPtr]::Zero
    for ($i = 0; $i -lt 100 -and $rail -eq [IntPtr]::Zero; $i++) {
        Start-Sleep -Milliseconds 100
        foreach ($hwnd in [WinUi]::WindowsOf([uint32] $process.Id)) {
            if ([WinUi]::Title($hwnd) -eq 'Aurora Control' -and [WinUi]::IsWindowVisible($hwnd)) { $rail = $hwnd }
        }
    }
    [pscustomobject]@{ Process = $process; Rail = $rail }
}

function Get-RailRect($run) {
    Start-Sleep -Milliseconds 600
    $rect = New-Object WinUi+RECT
    [WinUi]::GetWindowRect($run.Rail, [ref] $rect) | Out-Null
    $rect
}

function Stop-App($run) {
    Stop-Process -Id $run.Process.Id -ErrorAction SilentlyContinue
    Wait-Process -Id $run.Process.Id -Timeout 5 -ErrorAction SilentlyContinue
}

Add-Type -AssemblyName System.Windows.Forms
$primary = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
$lock = $null

try {
    Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
    if ($null -ne $runBefore) { Remove-ItemProperty $runKey -Name $value }

    $run = Start-App
    $ui = Invoke-Page 'return state();'
    Check 'window: version and defaults' ($ui.version -eq 'v0.2.0' -and $ui.side -eq 'Direita' -and $ui.startup -eq 'false' -and $ui.codex -eq 'true') ("{0} {1} startup={2}" -f $ui.version, $ui.side, $ui.startup)

    # Side through the UI: saved and applied live.
    $ui = Invoke-Page 'button("Esquerda").click(); await settled(); return state();'
    $rect = Get-RailRect $run
    Check 'ui side: saved' ($ui.side -eq 'Esquerda' -and (Read-Saved).side -eq 'left')
    Check 'ui side: rail moved live' ($rect.Left -eq $primary.Left) ("x {0}" -f $rect.Left)

    # Hide Codex: saved and the rail shrinks to one circle.
    $ui = Invoke-Page 'control("Mostrar Codex").click(); await settled(); return state();'
    $rect = Get-RailRect $run
    $scale = ($rect.Right - $rect.Left) / 60.0
    Check 'ui provider: saved hidden' ($ui.codex -eq 'false' -and -not (Read-Saved).providers.codex)
    Check 'ui provider: single circle' ([math]::Abs(($rect.Bottom - $rect.Top) - 52 * $scale) -le 1) ("height {0}" -f ($rect.Bottom - $rect.Top))
    $ui = Invoke-Page 'return { disabled: control("Mostrar Claude Code").disabled };'
    Check 'ui provider: last one locked' ($ui.disabled -eq $true)

    # Start with Windows: the Run value and the preference agree.
    $ui = Invoke-Page 'control("Iniciar com o Windows").click(); await settled(); return state();'
    Check 'startup on: Run value' ((Read-Run) -eq $expected) ("{0}" -f (Read-Run))
    Check 'startup on: saved and shown' ($ui.startup -eq 'true' -and (Read-Saved).launchAtStartup -and -not $ui.fix)

    # The save fails (file held open without delete sharing): the Run value
    # is restored and the window shows the error and the real state.
    $lock = [IO.File]::Open($file, 'Open', 'Read', 'None')
    $ui = Invoke-Page 'control("Iniciar com o Windows").click(); await settled(); return state();'
    $lock.Dispose(); $lock = $null
    Check 'failed save: error shown' ([bool] $ui.error) ("{0}" -f $ui.error)
    Check 'failed save: Run value restored' ((Read-Run) -eq $expected)
    Check 'failed save: preference unchanged' ((Read-Saved).launchAtStartup -eq $true)
    Check 'failed save: window shows stored state' ($ui.startup -eq 'true' -and -not $ui.fix)
    $leftovers = @(Get-ChildItem $dir | Where-Object { $_.Name -ne 'preferences.json' })
    Check 'failed save: no temporary file left' ($leftovers.Count -eq 0) ($leftovers.Name -join ', ')

    # The Run value removed outside the app: the window reports the
    # disagreement and "Corrigir" restores it.
    Remove-ItemProperty $runKey -Name $value
    $ui = Invoke-Page 'location.reload(); return true;'
    Start-Sleep -Milliseconds 800
    $ui = Invoke-Page 'await wait(300); return state();'
    Check 'mismatch: reported' ($ui.fix -and $ui.alerts -match 'não tem a entrada') ("{0}" -f $ui.alerts)
    $ui = Invoke-Page 'button("Corrigir").click(); await settled(); return state();'
    Check 'mismatch: fixed' ((Read-Run) -eq $expected -and -not $ui.fix)

    # Start with Windows off.
    $ui = Invoke-Page 'control("Iniciar com o Windows").click(); await settled(); return state();'
    Check 'startup off: Run value removed' ($null -eq (Read-Run))
    Check 'startup off: saved' ($ui.startup -eq 'false' -and -not (Read-Saved).launchAtStartup)
    Stop-App $run

    # Persistence across a restart.
    $run = Start-App
    $rect = Get-RailRect $run
    $ui = Invoke-Page 'return state();'
    Check 'restart: rail on the saved side' ($rect.Left -eq $primary.Left) ("x {0}" -f $rect.Left)
    Check 'restart: single circle kept' (($rect.Bottom - $rect.Top) -lt 100) ("height {0}" -f ($rect.Bottom - $rect.Top))
    Check 'restart: window shows saved choices' ($ui.side -eq 'Esquerda' -and $ui.codex -eq 'false' -and $ui.startup -eq 'false')

    # Showing Codex again brings the second circle back.
    $ui = Invoke-Page 'control("Mostrar Codex").click(); await settled(); return state();'
    $rect = Get-RailRect $run
    Check 'ui provider: shown again' ($ui.codex -eq 'true' -and ($rect.Bottom - $rect.Top) -gt 100) ("height {0}" -f ($rect.Bottom - $rect.Top))
    Stop-App $run
} finally {
    if ($lock) { $lock.Dispose() }
    Get-Process aurora-control -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe } | Stop-Process
    Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
    if ($existed) { Copy-Item $backup $dir -Recurse; Remove-Item $backup -Recurse -Force }
    if ($null -eq $runBefore) {
        if ($null -ne (Read-Run)) { Remove-ItemProperty $runKey -Name $value }
    } else {
        Set-ItemProperty $runKey -Name $value -Value $runBefore
    }
    Remove-Item $profile -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS, Env:AURORA_OPEN_SETTINGS -ErrorAction SilentlyContinue
}

Check 'cleanup: Run value as before' ((Read-Run) -eq $runBefore)
$failed = @($results.Values | Where-Object { -not $_ }).Count
Write-Output "Summary: $($results.Count - $failed)/$($results.Count) passed"
exit $failed
