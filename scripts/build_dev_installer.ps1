<#
.SYNOPSIS
    Build a Kage installer fast for dev iteration.

.DESCRIPTION
    Default: `cargo tauri build --debug` (debug profile). Compile time
    is the smallest of any path, the binary's runtime is unoptimised
    but fine for testing, and the resulting binary is auto-classified
    by `cfg(debug_assertions)`-driven dependencies (notably
    tauri-plugin-aptabase, which tags every event `isDebug=true` so
    Aptabase routes them into the Debug bucket and your prod
    dashboard stays clean).

    Pass -Release for a release-profile build with relaxed LTO/
    codegen-units. That's slower to compile (~3 minutes vs <1 minute)
    but produces a binary that actually represents what users will
    experience. Use this when you need to verify perf or repro a
    bug that only shows up under optimisation. Aptabase events from
    a -Release build are tagged `isDebug=false` and land in your
    production bucket — same as a real CI release.

    Pass -Release together with the env var
    `CARGO_PROFILE_RELEASE_LTO=true` (or just remove this script's
    overrides) to verify a final ship-quality build (~13 minutes on
    Windows). Plain `cargo tauri build` is the canonical path for
    that case anyway.

    Profile env vars are exported per-invocation only; Cargo.toml
    stays untouched, so plain `cargo tauri build` from a teammate's
    machine still gets the optimised CI config.

.PARAMETER NoBundle
    Pass through to `cargo tauri build --no-bundle` to skip NSIS
    bundling entirely. About 30s saved at the end of the build. The
    unbundled binary lands at target\<profile>\kage.exe and you can
    run it directly.

.PARAMETER Release
    Build with the release profile (relaxed LTO, codegen-units=16)
    instead of the default debug profile. Slower compile, faster
    runtime, and Aptabase events tagged isDebug=false (production).

.PARAMETER Replace
    After a successful build, kill any running kage / kage-computer-
    control-mcp process and copy the freshly-built kage.exe over the
    installed binary at %LOCALAPPDATA%\Kage\kage.exe. Implies
    -NoBundle (no point producing an installer if you're hot-
    swapping the .exe). The install dir is auto-detected from the
    running process; if Kage isn't running we fall back to the
    standard NSIS install location.

.EXAMPLE
    pwsh scripts/build_dev_installer.ps1
    # Default: debug profile, full bundle.

.EXAMPLE
    pwsh scripts/build_dev_installer.ps1 -Replace
    # Default debug profile, hot-swap the installed exe.

.EXAMPLE
    pwsh scripts/build_dev_installer.ps1 -Release -NoBundle
    # Release-profile binary in target\release\kage.exe.
#>

[CmdletBinding()]
param(
    [switch]$NoBundle,
    [switch]$Release,
    [switch]$Replace
)

if ($Replace) {
    # Replace mode is a hot-swap of the .exe over the installed
    # binary; the NSIS installer step would just be wasted work
    # so force --no-bundle for the user.
    $NoBundle = $true
}

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    if ($Release) {
        Write-Host "[build_dev_installer] Using fast release profile (lto=false, codegen-units=16)" -ForegroundColor Cyan
        $env:CARGO_PROFILE_RELEASE_LTO = 'false'
        $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = '16'
        $profileDir = 'release'
    }
    else {
        Write-Host "[build_dev_installer] Using debug profile (fast compile; Aptabase classifies as Debug)" -ForegroundColor Cyan
        Remove-Item Env:\CARGO_PROFILE_RELEASE_LTO -ErrorAction SilentlyContinue
        Remove-Item Env:\CARGO_PROFILE_RELEASE_CODEGEN_UNITS -ErrorAction SilentlyContinue
        $profileDir = 'debug'
    }

    # Tag the binary as a local dev build so init_logger() opts it in
    # to trace-level logging. CI's release pipeline doesn't set this,
    # so beta/stable channels still cap at Debug.
    $env:KAGE_LOCAL_DEV_BUILD = '1'

    # Bumping rustc's stack avoids a STATUS_STACK_BUFFER_OVERRUN we've hit
    # twice during type analysis under the heavy generic-monomorphization
    # load Tauri's command-handler macro produces. 16 MB is well above the
    # default 8 MB Windows stack and well below anything that would matter.
    if (-not $env:RUST_MIN_STACK) {
        $env:RUST_MIN_STACK = '16777216'
        Write-Host "[build_dev_installer] RUST_MIN_STACK=16MB (avoid STATUS_STACK_BUFFER_OVERRUN)" -ForegroundColor Cyan
    }

    $args = @('tauri', 'build')
    if (-not $Release) {
        $args += '--debug'
    }
    if ($NoBundle) {
        $args += '--no-bundle'
    }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Write-Host "[build_dev_installer] Running: cargo $($args -join ' ')" -ForegroundColor Cyan
    & cargo @args
    $exitCode = $LASTEXITCODE
    $sw.Stop()

    Write-Host ""
    Write-Host "[build_dev_installer] cargo exit code: $exitCode (elapsed: $([math]::Round($sw.Elapsed.TotalMinutes, 1)) min)" -ForegroundColor Cyan

    if ($exitCode -eq 0 -and -not $NoBundle) {
        # Glob the installer name — version is read from Cargo.toml
        # (tauri.conf.json no longer pins it) so the filename suffix
        # depends on whatever the current Cargo.toml package.version is.
        $nsisDir = Join-Path $repoRoot ('target\' + $profileDir + '\bundle\nsis')
        $installer = Get-ChildItem -Path $nsisDir -Filter 'Kage_*_x64-setup.exe' -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1
        if ($installer) {
            Write-Host "[build_dev_installer] Installer: $($installer.FullName) ($([math]::Round($installer.Length / 1MB, 1)) MB)" -ForegroundColor Green
        }
    }

    if ($exitCode -eq 0 -and $Replace) {
        # Discover the install dir. Prefer the path of any running
        # kage.exe (handles users who installed somewhere non-default)
        # and fall back to the NSIS default per-user location.
        $running = Get-Process -Name kage -ErrorAction SilentlyContinue |
            Select-Object -First 1
        $installDir = if ($running -and $running.Path) {
            Split-Path -Parent $running.Path
        }
        else {
            Join-Path $env:LOCALAPPDATA 'Kage'
        }
        $target = Join-Path $installDir 'kage.exe'
        $source = Join-Path $repoRoot ('target\' + $profileDir + '\kage.exe')

        if (-not (Test-Path -LiteralPath $source)) {
            Write-Host "[build_dev_installer] -Replace: source missing at $source — skipping copy" -ForegroundColor Yellow
            exit $exitCode
        }
        if (-not (Test-Path -LiteralPath $installDir)) {
            Write-Host "[build_dev_installer] -Replace: install dir not found at $installDir — skipping copy" -ForegroundColor Yellow
            exit $exitCode
        }

        # Stop both processes — kage spawns kage-computer-control-mcp as a
        # sidecar. Killing only the parent leaves the MCP child holding a
        # file lock on its own .exe; harmless for our copy of kage.exe but
        # belt-and-braces for any future swap that touches the MCP binary
        # too. Stop-Process is best-effort; Wait briefly so the OS
        # actually releases the file handle before we Copy-Item.
        Write-Host "[build_dev_installer] -Replace: stopping running kage processes." -ForegroundColor Cyan
        Get-Process -Name kage,kage-computer-control-mcp -ErrorAction SilentlyContinue |
            Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500

        Write-Host "[build_dev_installer] -Replace: $source -> $target" -ForegroundColor Cyan
        Copy-Item -LiteralPath $source -Destination $target -Force
        Write-Host "[build_dev_installer] -Replace: done. Launch via Start Menu / tray to test." -ForegroundColor Green
    }

    exit $exitCode
}
finally {
    Pop-Location
}
