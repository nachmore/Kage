// Mock ACP server for testing
// Run this with: cargo test --test mock_acp_server -- --ignored --nocapture
// Then run the main application in another terminal

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

#[test]
#[ignore] // Run manually with --ignored flag
fn run_mock_acp_server() {
    println!("Starting mock ACP server on 127.0.0.1:8765...");

    let listener = TcpListener::bind("127.0.0.1:8765").expect("Failed to bind to port 8765");
    println!("Mock ACP server listening on 127.0.0.1:8765");
    println!("Press Ctrl+C to stop");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                println!("New connection from: {}", stream.peer_addr().unwrap());
                thread::spawn(|| handle_client(stream));
            }
            Err(e) => {
                eprintln!("Connection error: {}", e);
            }
        }
    }
}

fn handle_client(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                println!("Client disconnected");
                break;
            }
            Ok(_) => {
                println!("Received: {}", line.trim());

                // Parse the request
                if let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) {
                    let id = request["id"].as_str().unwrap_or("unknown");
                    let method = request["method"].as_str().unwrap_or("unknown");
                    let message = request["params"]["message"].as_str().unwrap_or("");

                    println!("Method: {}, Message: {}", method, message);

                    // Create a mock response
                    let response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": format!("Mock response to: {}", message)
                        }
                    });

                    let response_str = serde_json::to_string(&response).unwrap();
                    println!("Sending: {}", response_str);

                    if let Err(e) = writeln!(stream, "{}", response_str) {
                        eprintln!("Failed to send response: {}", e);
                        break;
                    }

                    if let Err(e) = stream.flush() {
                        eprintln!("Failed to flush stream: {}", e);
                        break;
                    }
                } else {
                    eprintln!("Failed to parse request as JSON");
                }
            }
            Err(e) => {
                eprintln!("Error reading from stream: {}", e);
                break;
            }
        }
    }
}
