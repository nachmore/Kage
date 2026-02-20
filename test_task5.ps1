# Test script for Task 5: Floating Ghost + Prompt Interface

Write-Host "=== Task 5 Test: Floating Ghost + Prompt Interface ===" -ForegroundColor Cyan
Write-Host ""

Write-Host "Test Steps:" -ForegroundColor Yellow
Write-Host "1. Launch the application (it should start minimized to system tray)"
Write-Host "2. Press Alt+Space (or Alt+K) to show the floating ghost window"
Write-Host "3. Verify the floating window appears with:"
Write-Host "   - Kiro ghost mascot (👻 icon)"
Write-Host "   - Speech bubble with text input"
Write-Host "   - No window decorations"
Write-Host "   - Transparent background"
Write-Host "   - Always on top of other windows"
Write-Host "4. Type a message in the input box"
Write-Host "5. Press Enter - the full chat window should open with your message"
Write-Host "6. Press Alt+Space again to show the floating window"
Write-Host "7. Press Escape - the floating window should dismiss"
Write-Host "8. Press Alt+Space again to show the floating window"
Write-Host "9. Click outside the floating window - it should dismiss"
Write-Host ""

Write-Host "Expected Results:" -ForegroundColor Green
Write-Host "✓ Floating window appears at screen center"
Write-Host "✓ Window has no decorations and transparent background"
Write-Host "✓ Window stays on top of other windows"
Write-Host "✓ Ghost mascot is visible and animated (floating effect)"
Write-Host "✓ Speech bubble contains text input"
Write-Host "✓ Pressing Enter opens full chat window with message"
Write-Host "✓ Pressing Escape dismisses the floating window"
Write-Host "✓ Clicking outside dismisses the floating window"
Write-Host "✓ Hotkey toggles floating window visibility"
Write-Host ""

Write-Host "Starting application..." -ForegroundColor Cyan
cargo run
