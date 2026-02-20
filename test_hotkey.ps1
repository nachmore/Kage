# Test script for hotkey debugging
Write-Host "=== Kiro Assistant Hotkey Test ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "This script will help you test the global hotkey functionality." -ForegroundColor Yellow
Write-Host ""
Write-Host "Steps:" -ForegroundColor Green
Write-Host "1. Make sure the app is NOT already running"
Write-Host "2. This script will launch the app"
Write-Host "3. Watch the console output for hotkey registration messages"
Write-Host "4. Try pressing Alt+Space (or Alt+K if that fails)"
Write-Host "5. You should see 'HOTKEY TRIGGERED' messages when you press the hotkey"
Write-Host ""
Write-Host "Press any key to start the app..." -ForegroundColor Yellow
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
Write-Host ""

# Kill any existing instances
Write-Host "Checking for existing instances..." -ForegroundColor Cyan
$existing = Get-Process -Name "kiro-assistant" -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "Found existing instance, stopping it..." -ForegroundColor Yellow
    Stop-Process -Name "kiro-assistant" -Force
    Start-Sleep -Seconds 1
}

# Launch the app
Write-Host "Launching Kiro Assistant..." -ForegroundColor Cyan
Write-Host ""
Write-Host "=== APP OUTPUT BELOW ===" -ForegroundColor Magenta
Write-Host ""

& ".\target\debug\kiro-assistant.exe"
