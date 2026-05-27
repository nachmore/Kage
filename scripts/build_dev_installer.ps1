<#
.SYNOPSIS
    Build a Kage installer with relaxed release-profile settings for faster
    dev iteration.

.DESCRIPTION
    `cargo tauri build` defaults to the project's `[profile.release]` config,
    which in this repo sets `lto = true` + `codegen-units = 1`. That's the
    right setting for ship builds — smallest binary, fastest at runtime — but
    it means a clean rebuild on Windows is roughly 12-15 minutes, dominated
    by single-threaded LLVM ThinLTO link time. When you're iterating on a
    bug that only repros in the bundled installer (not `cargo tauri dev`),
    that round-trip becomes the limit.

    This wrapper sets:

      CARGO_PROFILE_RELEASE_LTO=false
      CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16

    via env vars (which Cargo honors per-invocation, overriding Cargo.toml)
    and runs the normal `cargo tauri build`. The env-var overrides flow to
    both the kage-computer-control-mcp build done by scripts/build_mcp.py
    and the main `kage` build that Tauri kicks off. Build time on this
    machine drops from ~13 minutes to ~3 minutes; the resulting binary is
    a few MB larger and slightly slower at runtime, neither of which
    matter for iteration.

    The Cargo.toml profile stays untouched, so CI ship builds and any
    plain `cargo tauri build` from a teammate's machine still get the
    optimized config.

    Tauri 2's CLI doesn't expose `--profile` so this is the cleanest way
    to override per-invocation.

.PARAMETER NoBundle
    Pass through to `cargo tauri build --no-bundle` to skip NSIS bundling
    entirely. About 30s saved at the end of the build. The unbundled
    binary lands at target\release\kage.exe and you can run it directly.

.PARAMETER Release
    Use the original full-LTO release profile (i.e. don't override). Useful
    for verifying a final build after fast iteration.

.PARAMETER Replace
    After a successful build, kill any running kage / kage-computer-control-mcp
    process and copy the freshly-built target\release\kage.exe over the
    installed binary at %LOCALAPPDATA%\Kage\kage.exe. Implies -NoBundle (no
    point producing an installer if you're hot-swapping the .exe). The
    install dir is auto-detected from the running process; if Kage isn't
    running we fall back to the standard NSIS install location.

.EXAMPLE
    pwsh scripts/build_dev_installer.ps1

.EXAMPLE
    pwsh scripts/build_dev_installer.ps1 -NoBundle

.EXAMPLE
    pwsh scripts/build_dev_installer.ps1 -Release

.EXAMPLE
    pwsh scripts/build_dev_installer.ps1 -Replace
#>

[CmdletBinding()]
param(
    [switch]$NoBundle,
    [switch]$Release,
    [switch]$Replace
)

if ($Replace) {
    # Replace mode is a hot-swap of target\release\kage.exe over the
    # installed binary; the NSIS installer step would just be wasted work
    # so force --no-bundle for the user.
    $NoBundle = $true
}

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    if (-not $Release) {
        Write-Host "[build_dev_installer] Using fast release profile (lto=false, codegen-units=16)" -ForegroundColor Cyan
        $env:CARGO_PROFILE_RELEASE_LTO = 'false'
        $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = '16'
    }
    else {
        Write-Host "[build_dev_installer] Using full release profile (Cargo.toml defaults)" -ForegroundColor Cyan
        Remove-Item Env:\CARGO_PROFILE_RELEASE_LTO -ErrorAction SilentlyContinue
        Remove-Item Env:\CARGO_PROFILE_RELEASE_CODEGEN_UNITS -ErrorAction SilentlyContinue
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
        $nsisDir = Join-Path $repoRoot 'target\release\bundle\nsis'
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
        $source = Join-Path $repoRoot 'target\release\kage.exe'

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
        Write-Host "[build_dev_installer] -Replace: stopping running kage processes…" -ForegroundColor Cyan
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
