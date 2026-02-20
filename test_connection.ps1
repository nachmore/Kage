# Simple PowerShell script to test ACP connection
# This sends a test message to the mock server

$client = New-Object System.Net.Sockets.TcpClient
try {
    Write-Host "Connecting to 127.0.0.1:8765..."
    $client.Connect("127.0.0.1", 8765)
    Write-Host "Connected!"
    
    $stream = $client.GetStream()
    $writer = New-Object System.IO.StreamWriter($stream)
    $reader = New-Object System.IO.StreamReader($stream)
    
    $request = @{
        jsonrpc = "2.0"
        id = "test-123"
        method = "chat"
        params = @{
            message = "Hello from PowerShell test"
        }
    } | ConvertTo-Json -Compress
    
    Write-Host "Sending: $request"
    $writer.WriteLine($request)
    $writer.Flush()
    
    $response = $reader.ReadLine()
    Write-Host "Received: $response"
    
    $writer.Close()
    $reader.Close()
    $stream.Close()
    Write-Host "Test successful!"
} catch {
    Write-Host "Error: $_"
} finally {
    $client.Close()
}
