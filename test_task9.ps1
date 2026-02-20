# Test script for Task 9: Error Handling and Polish

Write-Host "=== Task 9 Test: Error Handling and Polish ===" -ForegroundColor Cyan
Write-Host ""

Write-Host "This test will verify:" -ForegroundColor Yellow
Write-Host "1. Logging to file with rotation" -ForegroundColor White
Write-Host "2. User-friendly error notifications" -ForegroundColor White
Write-Host "3. Reconnection logic with exponential backoff" -ForegroundColor White
Write-Host "4. Edge case handling (kiro-cli not running)" -ForegroundColor White
Write-Host ""

Write-Host "Test Steps:" -ForegroundColor Yellow
Write-Host "1. Build the application" -ForegroundColor White
Write-Host "2. Start the application" -ForegroundColor White
Write-Host "3. Check log file creation" -ForegroundColor White
Write-Host "4. Test error handling when kiro-cli is not running" -ForegroundColor White
Write-Host "5. Test reconnection when kiro-cli becomes available" -ForegroundColor White
Write-Host ""

# Step 1: Build
Write-Host "[Step 1] Building application..." -ForegroundColor Green
cargo build --release
if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}
Write-Host "Build successful!" -ForegroundColor Green
Write-Host ""

# Step 2: Check log directory
Write-Host "[Step 2] Checking log file location..." -ForegroundColor Green
$logDir = "$env:LOCALAPPDATA\kiro-assistant\logs"
Write-Host "Log directory: $logDir" -ForegroundColor Cyan
if (Test-Path $logDir) {
    Write-Host "Log directory exists" -ForegroundColor Green
    $logFile = Join-Path $logDir "kiro-assistant.log"
    if (Test-Path $logFile) {
        Write-Host "Log file exists: $logFile" -ForegroundColor Green
        Write-Host "Log file size: $((Get-Item $logFile).Length) bytes" -ForegroundColor Cyan
    }
} else {
    Write-Host "Log directory will be created on first run" -ForegroundColor Yellow
}
Write-Host ""

# Step 3: Start application
Write-Host "[Step 3] Starting application..." -ForegroundColor Green
Write-Host "The application will start in the background." -ForegroundColor Yellow
Write-Host ""

$appPath = ".\target\release\kiro-assistant.exe"
if (Test-Path $appPath) {
    Write-Host "Starting: $appPath" -ForegroundColor Cyan
    Start-Process $appPath
    Write-Host "Application started!" -ForegroundColor Green
    Write-Host ""
    
    # Wait a moment for the app to start
    Start-Sleep -Seconds 3
    
    # Check log file again
    if (Test-Path $logFile) {
        Write-Host "Log file created successfully!" -ForegroundColor Green
        Write-Host "Recent log entries:" -ForegroundColor Cyan
        Get-Content $logFile -Tail 10
        Write-Host ""
    }
} else {
    Write-Host "Application executable not found at: $appPath" -ForegroundColor Red
    Write-Host "Please run 'cargo build --release' first" -ForegroundColor Yellow
    exit 1
}

Write-Host "=== Manual Testing Instructions ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "1. Press Alt+Space (or Alt+K) to open the floating window" -ForegroundColor White
Write-Host "2. Type a message and press Enter" -ForegroundColor White
Write-Host "3. If kiro-cli is NOT running, you should see:" -ForegroundColor Yellow
Write-Host "   - A user-friendly error message" -ForegroundColor White
Write-Host "   - A 'Reconnect' button" -ForegroundColor White
Write-Host "   - Connection status showing 'Disconnected'" -ForegroundColor White
Write-Host ""
Write-Host "4. Start kiro-cli in another terminal:" -ForegroundColor Yellow
Write-Host "   kiro-cli --acp-server" -ForegroundColor Cyan
Write-Host ""
Write-Host "5. Click the 'Reconnect' button" -ForegroundColor Yellow
Write-Host "   - Should show 'Reconnecting...' message" -ForegroundColor White
Write-Host "   - Should connect with exponential backoff retry logic" -ForegroundColor White
Write-Host "   - Should show 'Successfully reconnected!' message" -ForegroundColor White
Write-Host "   - Connection status should change to 'Connected'" -ForegroundColor White
Write-Host ""
Write-Host "6. Try sending a message again - it should work now" -ForegroundColor Yellow
Write-Host ""
Write-Host "7. Check the log file for detailed logging:" -ForegroundColor Yellow
Write-Host "   $logFile" -ForegroundColor Cyan
Write-Host ""
Write-Host "8. To test log rotation:" -ForegroundColor Yellow
Write-Host "   - The log file will rotate when it exceeds 10MB" -ForegroundColor White
Write-Host "   - Old logs are kept as .log.1, .log.2, etc." -ForegroundColor White
Write-Host "   - Maximum of 5 log files are retained" -ForegroundColor White
Write-Host ""

Write-Host "Press any key to view the current log file..." -ForegroundColor Yellow
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")

if (Test-Path $logFile) {
    Write-Host ""
    Write-Host "=== Current Log File Contents ===" -ForegroundColor Cyan
    Get-Content $logFile
} else {
    Write-Host "Log file not found yet. Start the application first." -ForegroundColor Yellow
}
