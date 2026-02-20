# Task 4 Testing Script
# This script helps test the system tray and hotkey functionality

Write-Host "=== Kiro Assistant - Task 4 Testing ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Starting the application..." -ForegroundColor Yellow
Write-Host ""

# Start the application
Start-Process "cargo" -ArgumentList "run" -NoNewWindow

Write-Host "Application started!" -ForegroundColor Green
Write-Host ""
Write-Host "Please test the following:" -ForegroundColor Cyan
Write-Host "1. Check system tray for Kiro Assistant icon" -ForegroundColor White
Write-Host "2. Press Alt+K to show/hide the window" -ForegroundColor White
Write-Host "3. Right-click tray icon and test 'Show' menu item" -ForegroundColor White
Write-Host "4. Left-click tray icon to show window" -ForegroundColor White
Write-Host "5. Use 'Quit' from tray menu to exit" -ForegroundColor White
Write-Host ""
Write-Host "Note: If Alt+Space doesn't work, the app will use Alt+K instead" -ForegroundColor Yellow
Write-Host ""
Write-Host "Press Ctrl+C to stop this script (app will continue running)" -ForegroundColor Gray
Write-Host ""

# Keep script running
while ($true) {
    Start-Sleep -Seconds 1
}
