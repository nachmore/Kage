# Test script for Task 7: Settings Interface

Write-Host "=== Task 7: Settings Interface Test ===" -ForegroundColor Cyan
Write-Host ""

# Build the application
Write-Host "Building application..." -ForegroundColor Yellow
cargo build
if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}
Write-Host "Build successful!" -ForegroundColor Green
Write-Host ""

# Run the application
Write-Host "Starting Kiro Assistant..." -ForegroundColor Yellow
Write-Host ""
Write-Host "Test Instructions:" -ForegroundColor Cyan
Write-Host "1. The app should start in the system tray" -ForegroundColor White
Write-Host "2. Right-click the tray icon and select 'Settings'" -ForegroundColor White
Write-Host "3. Verify the settings window opens with all sections:" -ForegroundColor White
Write-Host "   - Hotkey customization" -ForegroundColor Gray
Write-Host "   - ACP connection settings (host, port, timeout)" -ForegroundColor Gray
Write-Host "   - UI preferences (theme, opacity, window size)" -ForegroundColor Gray
Write-Host "   - Auto-start toggle" -ForegroundColor Gray
Write-Host "4. Click 'Change' button for hotkey and press a new combination" -ForegroundColor White
Write-Host "5. Modify some settings and click 'Save Settings'" -ForegroundColor White
Write-Host "6. Verify success message appears" -ForegroundColor White
Write-Host "7. Close settings window" -ForegroundColor White
Write-Host "8. Press the hotkey (Alt+Space or Alt+K) to open floating window" -ForegroundColor White
Write-Host "9. Right-click on the floating window and select 'Settings'" -ForegroundColor White
Write-Host "10. Verify settings window opens again" -ForegroundColor White
Write-Host "11. Open the chat window (type something and press Enter)" -ForegroundColor White
Write-Host "12. Click the 'Settings' button in the chat window header" -ForegroundColor White
Write-Host "13. Verify settings window opens" -ForegroundColor White
Write-Host "14. Close the app and restart it" -ForegroundColor White
Write-Host "15. Verify settings are persisted (check config file)" -ForegroundColor White
Write-Host ""
Write-Host "Config file location: %APPDATA%\kiro-assistant\config.json" -ForegroundColor Yellow
Write-Host ""
Write-Host "Press Ctrl+C to stop the application when done testing" -ForegroundColor Yellow
Write-Host ""

.\target\debug\kiro-assistant.exe
