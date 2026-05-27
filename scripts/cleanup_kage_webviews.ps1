<#
.SYNOPSIS
    Find and kill orphan WebView2 child processes left behind by Kage.

.DESCRIPTION
    Kage's webview runs in msedgewebview2.exe child processes that
    point at %LOCALAPPDATA%\com.kage.launcher\EBWebView as their
    user-data-dir (Tauri 2 derives the path from the bundle identifier
    in tauri.conf.json, not the productName).
    WebView2 enforces single-writer semantics on that folder, so if a
    previous kage.exe was force-killed and its children outlived it,
    the next launch fails to render: the floating window never
    appears, and the WebView2 process exits with a directory-lock
    error. The user-visible symptom is "I pressed the hotkey and
    nothing happened" — the tray icon shows up but no window.

    The app itself does this same cleanup automatically on launch (see
    src/startup.rs::ensure_webview_directory_writable). This script is
    the manual escape hatch — useful when:

      - You want to clean up before launching kage (paranoid sweep).
      - The auto-cleanup fails for some reason and you need a
        different code path to compare against.
      - You're investigating a bug and want to inspect the orphans
        before killing them (use -DryRun).

.PARAMETER DryRun
    List matching processes without killing them.

.PARAMETER UserDataDir
    Override the user-data-dir to match against. Default:
    %LOCALAPPDATA%\com.kage.launcher\EBWebView. Useful if you run a
    portable build or have a custom Tauri user-data-dir override.

.EXAMPLE
    .\scripts\cleanup_kage_webviews.ps1
    Kill any matching orphan WebView2 processes.

.EXAMPLE
    .\scripts\cleanup_kage_webviews.ps1 -DryRun
    Show what would be killed without actually killing.

.NOTES
    Safe to run while kage.exe is alive. The match logic uses the
    full user-data-dir path as a substring discriminator, so other
    apps that use WebView2 (VS Code, Slack, Teams, etc.) are not
    touched even though they share the msedgewebview2.exe image
    name. The same matching contract is exercised by the unit tests
    in tests/startup_test.rs.
#>

[CmdletBinding()]
param(
    [switch]$DryRun,
    [string]$UserDataDir = (Join-Path $env:LOCALAPPDATA 'com.kage.launcher\EBWebView')
)

$ErrorActionPreference = 'Stop'

# Normalise once so the per-process comparison is cheap.
$needle = $UserDataDir.ToLower()
Write-Host "Looking for msedgewebview2.exe processes pinned to:"
Write-Host "  $UserDataDir"
Write-Host ""

# Get-CimInstance is the modern replacement for the (deprecated)
# Get-WmiObject. It surfaces CommandLine for every process the user
# can read — which includes anything we spawned ourselves.
$candidates = Get-CimInstance -ClassName Win32_Process -Filter "Name = 'msedgewebview2.exe'" `
    -ErrorAction Stop
if (-not $candidates) {
    Write-Host "No msedgewebview2.exe processes are running on this system."
    return
}

# Filter to processes whose command line names *our* user-data-dir.
# CommandLine can be $null for protected processes — guard before
# .ToLower() to avoid a noisy null-method exception.
$matches = $candidates | Where-Object {
    $_.CommandLine -and ($_.CommandLine.ToLower().Contains($needle))
}

if (-not $matches) {
    Write-Host "Found $($candidates.Count) msedgewebview2.exe process(es), but none belong to Kage."
    Write-Host "(They probably belong to VS Code / Slack / Teams / Outlook / etc.)"
    return
}

Write-Host "Found $($matches.Count) Kage-owned WebView2 process(es):"
$matches | ForEach-Object {
    $shortCmd = if ($_.CommandLine.Length -gt 160) {
        $_.CommandLine.Substring(0, 160) + '...'
    } else {
        $_.CommandLine
    }
    Write-Host ("  PID {0,-7} (parent {1,-7}) {2}" -f $_.ProcessId, $_.ParentProcessId, $shortCmd)
}

if ($DryRun) {
    Write-Host ""
    Write-Host "[DryRun] Skipping the kill step. Re-run without -DryRun to clean up." -ForegroundColor Yellow
    return
}

Write-Host ""
Write-Host "Killing $($matches.Count) process(es)..."
$killed = 0
$failed = 0
foreach ($p in $matches) {
    try {
        # Stop-Process -Force is a clean SIGTERM-equivalent on Windows.
        # We don't need taskkill /T here — child processes of each
        # msedgewebview2.exe are themselves msedgewebview2.exe instances
        # with the same user-data-dir, so they're already in our match
        # list and will be stopped on the same iteration.
        Stop-Process -Id $p.ProcessId -Force -ErrorAction Stop
        Write-Host "  ✓ killed PID $($p.ProcessId)"
        $killed++
    } catch {
        Write-Host "  ✗ failed PID $($p.ProcessId): $($_.Exception.Message)" -ForegroundColor Red
        $failed++
    }
}

Write-Host ""
if ($failed -eq 0) {
    Write-Host "Cleanup complete: $killed process(es) killed." -ForegroundColor Green
} else {
    Write-Host "Cleanup partial: $killed killed, $failed failed." -ForegroundColor Yellow
    Write-Host "Failed kills are usually permission errors — try running as Administrator."
}
