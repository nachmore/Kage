# Test script for Task 8: Command Mode for Application Launching
# This script tests the application launcher functionality

Write-Host "=== Task 8: Command Mode Test ===" -ForegroundColor Cyan
Write-Host ""

# Check if the application is built
if (-not (Test-Path "target\debug\kiro-assistant.exe")) {
    Write-Host "ERROR: Application not built. Run 'cargo build' first." -ForegroundColor Red
    exit 1
}

Write-Host "Test Plan:" -ForegroundColor Yellow
Write-Host "1. Launch the Kiro Assistant application"
Write-Host "2. Press the global hotkey (Alt+Space or Alt+K) to show the floating window"
Write-Host "3. Type an application name (e.g., 'notepad', 'word', 'chrome')"
Write-Host "4. Press Enter"
Write-Host "5. Verify that:"
Write-Host "   - The application launches if found"
Write-Host "   - The floating window closes after launching"
Write-Host "   - If no match, the chat window opens instead"
Write-Host ""

Write-Host "Starting Kiro Assistant..." -ForegroundColor Green
Write-Host ""

# Start the application
$process = Start-Process -FilePath "target\debug\kiro-assistant.exe" -PassThru

Write-Host "Application started (PID: $($process.Id))" -ForegroundColor Green
Write-Host ""

Write-Host "Manual Testing Steps:" -ForegroundColor Yellow
Write-Host "1. Press Alt+Space (or Alt+K) to show the floating window"
Write-Host "2. Try typing these commands:"
Write-Host "   - 'notepad' (should launch Notepad)"
Write-Host "   - 'calc' (should launch Calculator)"
Write-Host "   - 'word' (should launch Microsoft Word if installed)"
Write-Host "   - 'chrome' (should launch Chrome if installed)"
Write-Host "   - 'edge' (should launch Edge if installed)"
Write-Host "3. Try typing a non-app query like 'hello' (should open chat)"
Write-Host ""

Write-Host "Expected Behavior:" -ForegroundColor Cyan
Write-Host "✓ Application names are recognized and launched"
Write-Host "✓ Fuzzy matching works (e.g., 'note' matches 'notepad')"
Write-Host "✓ Floating window closes after launching an app"
Write-Host "✓ Non-app queries open the chat window"
Write-Host "✓ Error messages are shown if app launch fails"
Write-Host ""

Write-Host "Press Enter when you're done testing to stop the application..." -ForegroundColor Yellow
Read-Host

# Stop the application
Write-Host "Stopping application..." -ForegroundColor Yellow
Stop-Process -Id $process.Id -Force
Write-Host "Application stopped." -ForegroundColor Green
Write-Host ""

Write-Host "=== Test Complete ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Verification Checklist:" -ForegroundColor Yellow
Write-Host "[ ] Application launcher scans installed apps"
Write-Host "[ ] Command detection recognizes app names"
Write-Host "[ ] Apps launch directly from floating window"
Write-Host "[ ] Fuzzy matching works for app names"
Write-Host "[ ] Floating window closes after launching"
Write-Host "[ ] Non-app queries open chat mode"
Write-Host "[ ] Error handling works properly"
Write-Host ""
Write-Host "If all checks pass, commit with:" -ForegroundColor Green
Write-Host "  git commit -m `"Task 8: Add command mode for application launching`"" -ForegroundColor Gray
