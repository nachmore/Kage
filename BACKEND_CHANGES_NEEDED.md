# Backend Changes Needed for Floating Window

## Configuration for Local ACP Server

The assistant now supports spawning a local ACP server process using a custom spawn command.

### Configuration

Add the `spawn_command` field to your configuration file (located at `~/.config/kiro-assistant/config.json` on Linux/macOS or `%APPDATA%\kiro-assistant\config.json` on Windows):

```json
{
  "version": 1,
  "hotkey": {
    "modifiers": ["Alt"],
    "key": "Space"
  },
  "acp": {
    "host": "127.0.0.1",
    "port": 8765,
    "timeout_ms": 30000,
    "spawn_command": "C:\\workplace\\kiro-cli\\target\\release\\chat_cli.exe acp"
  },
  "ui": {
    "theme": "dark",
    "floating_window_opacity": 1.0,
    "chat_window_width": 800,
    "chat_window_height": 600
  },
  "system": {
    "auto_start": false
  }
}
```

### How It Works

1. If `spawn_command` is set, the assistant will spawn the exact command specified
2. The spawned process should start an ACP server
3. The assistant will then connect to this process for chat functionality
4. When the assistant closes, it will automatically terminate the spawned process

### Example Usage

**Windows:**
```json
"spawn_command": "C:\\Program Files\\Kiro\\kiro.exe acp"
```

**Linux/macOS:**
```json
"spawn_command": "/usr/local/bin/kiro acp"
```

**Development build:**
```json
"spawn_command": "C:\\workplace\\kiro-cli\\target\\release\\chat_cli.exe acp"
```

**Leave unset to use external process:**
```json
"spawn_command": null
```

### Flexibility

The spawn command is executed as-is, allowing you to add any arguments:
- `chat_cli acp --verbose`
- `chat_cli acp --log-level debug`
- `kiro serve --mode acp --port 8765`

This means future CLI changes don't require code updates to the assistant.

## 1. Default Window Position (1/3 from top)

The floating window should appear at 1/3 down from the top of the screen, centered horizontally.

### Rust Implementation (src/main.rs or window management file)

```rust
use tauri::Manager;
use tauri::PhysicalPosition;

// When creating the floating window
fn create_floating_window(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let window = tauri::WindowBuilder::new(
        app,
        "floating",
        tauri::WindowUrl::App("floating.html".into())
    )
    .title("Kiro")
    .inner_size(500.0, 60.0) // Width x Height
    .decorations(false)
    .always_on_top(true)
    .resizable(false)
    .transparent(true)
    .visible(false)
    .build()?;
    
    // Get primary monitor size
    if let Some(monitor) = window.primary_monitor()? {
        let monitor_size = monitor.size();
        let window_size = window.outer_size()?;
        
        // Calculate position: 1/3 from top, centered horizontally
        let x = (monitor_size.width - window_size.width) / 2;
        let y = monitor_size.height / 3;
        
        window.set_position(PhysicalPosition::new(x as i32, y as i32))?;
    }
    
    Ok(())
}
```

## 2. Window Dragging

Add a Tauri command to enable window dragging when the ghost is clicked.

### Rust Implementation

```rust
use tauri::Window;

#[tauri::command]
async fn start_drag_window(window: Window) -> Result<(), String> {
    window.start_dragging().map_err(|e| e.to_string())
}

// Register the command in main.rs
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            start_drag_window,
            // ... other commands
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

### Alternative: Manual Drag Implementation

If `start_dragging()` doesn't work as expected, implement manual dragging:

```rust
use tauri::{Manager, PhysicalPosition};
use std::sync::Mutex;

struct DragState {
    is_dragging: bool,
    start_x: i32,
    start_y: i32,
    window_x: i32,
    window_y: i32,
}

#[tauri::command]
async fn start_drag(
    window: Window,
    state: tauri::State<'_, Mutex<DragState>>,
    client_x: i32,
    client_y: i32,
) -> Result<(), String> {
    let position = window.outer_position().map_err(|e| e.to_string())?;
    
    let mut drag_state = state.lock().unwrap();
    drag_state.is_dragging = true;
    drag_state.start_x = client_x;
    drag_state.start_y = client_y;
    drag_state.window_x = position.x;
    drag_state.window_y = position.y;
    
    Ok(())
}

#[tauri::command]
async fn drag_move(
    window: Window,
    state: tauri::State<'_, Mutex<DragState>>,
    client_x: i32,
    client_y: i32,
) -> Result<(), String> {
    let drag_state = state.lock().unwrap();
    
    if drag_state.is_dragging {
        let delta_x = client_x - drag_state.start_x;
        let delta_y = client_y - drag_state.start_y;
        
        let new_x = drag_state.window_x + delta_x;
        let new_y = drag_state.window_y + delta_y;
        
        window.set_position(PhysicalPosition::new(new_x, new_y))
            .map_err(|e| e.to_string())?;
    }
    
    Ok(())
}

#[tauri::command]
async fn stop_drag(state: tauri::State<'_, Mutex<DragState>>) -> Result<(), String> {
    let mut drag_state = state.lock().unwrap();
    drag_state.is_dragging = false;
    Ok(())
}
```

## 3. Save Window Position

Optionally save the window position so it remembers where the user moved it.

### Rust Implementation

```rust
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default)]
struct WindowPosition {
    x: i32,
    y: i32,
}

fn get_position_file_path() -> PathBuf {
    // Save to app config directory
    let mut path = tauri::api::path::config_dir().unwrap();
    path.push("kiro-assistant");
    path.push("window_position.json");
    path
}

fn save_window_position(x: i32, y: i32) -> Result<(), Box<dyn std::error::Error>> {
    let position = WindowPosition { x, y };
    let json = serde_json::to_string(&position)?;
    
    let path = get_position_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    
    fs::write(path, json)?;
    Ok(())
}

fn load_window_position() -> Option<WindowPosition> {
    let path = get_position_file_path();
    if path.exists() {
        let json = fs::read_to_string(path).ok()?;
        serde_json::from_str(&json).ok()
    } else {
        None
    }
}

// Use in window creation
fn create_floating_window(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let window = tauri::WindowBuilder::new(
        app,
        "floating",
        tauri::WindowUrl::App("floating.html".into())
    )
    // ... other settings
    .build()?;
    
    // Try to load saved position, otherwise use default
    if let Some(saved_pos) = load_window_position() {
        window.set_position(PhysicalPosition::new(saved_pos.x, saved_pos.y))?;
    } else {
        // Default position (1/3 from top)
        if let Some(monitor) = window.primary_monitor()? {
            let monitor_size = monitor.size();
            let window_size = window.outer_size()?;
            
            let x = (monitor_size.width - window_size.width) / 2;
            let y = monitor_size.height / 3;
            
            window.set_position(PhysicalPosition::new(x as i32, y as i32))?;
        }
    }
    
    Ok(())
}

// Save position when window is moved
#[tauri::command]
async fn save_position(window: Window) -> Result<(), String> {
    let position = window.outer_position().map_err(|e| e.to_string())?;
    save_window_position(position.x, position.y).map_err(|e| e.to_string())
}
```

## 4. Open Full Chat Window Command

Add a command to open the main chat window from the floating window.

### Rust Implementation

```rust
#[tauri::command]
async fn open_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    // Get or create the main chat window
    if let Some(window) = app.get_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    } else {
        // Create the main window if it doesn't exist
        tauri::WindowBuilder::new(
            &app,
            "main",
            tauri::WindowUrl::App("index.html".into())
        )
        .title("Kiro Assistant")
        .inner_size(800.0, 600.0)
        .build()
        .map_err(|e| e.to_string())?;
    }
    
    Ok(())
}
```

## Summary of Commands to Register

```rust
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            start_drag_window,      // Simple drag (recommended)
            // OR
            start_drag,             // Manual drag
            drag_move,              // Manual drag
            stop_drag,              // Manual drag
            save_position,          // Optional: save position
            open_chat_window,       // Open full chat
            // ... other existing commands
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

## Testing

1. Launch the app - floating window should appear 1/3 from top, centered
2. Click and drag the ghost - window should move with cursor
3. Release mouse - window stays in new position
4. Close and reopen app - window appears in last position (if save_position is implemented)
5. Click expand button - main chat window opens
6. Ask a question - ghost bounces and glows while thinking
