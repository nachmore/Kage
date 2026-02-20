# Rust Backend Changes Summary

## Files Modified

### 1. src/main.rs

#### New Commands Added

1. **`start_drag_window`** - Enables window dragging
   ```rust
   #[tauri::command]
   async fn start_drag_window(window: Window) -> Result<(), String>
   ```
   - Called when user clicks and drags the ghost icon
   - Uses Tauri's built-in `start_dragging()` method
   - Allows the floating window to be moved anywhere on screen

2. **`open_chat_window`** - Opens the full chat experience
   ```rust
   #[tauri::command]
   async fn open_chat_window(app: tauri::AppHandle) -> Result<(), String>
   ```
   - Hides the floating window
   - Shows and focuses the main chat window
   - Called when user clicks the expand button

#### Modified Functions

1. **Hotkey positioning** - Changed from center to 1/3 from top
   - Primary hotkey handler: Position at `y = monitor_height / 3`
   - Fallback hotkey handler: Position at `y = monitor_height / 3`
   - Window width updated to 500px (from 400px)

2. **`test_floating_window`** - Updated positioning
   - Now positions at 1/3 from top instead of center
   - Uses 500px width for calculations

#### Command Registration

Updated `invoke_handler` to include new commands:
```rust
.invoke_handler(tauri::generate_handler![
    // ... existing commands
    start_drag_window,      // NEW
    open_chat_window        // NEW
])
```

### 2. tauri.conf.json

#### Floating Window Configuration

Updated the floating window dimensions and behavior:
```json
{
  "label": "floating",
  "width": 500,      // Changed from 400
  "height": 60,      // Changed from 450 (compact design)
  "center": false    // Changed from true (we position manually)
}
```

## How It Works

### Window Positioning (1/3 from top)

When the hotkey is pressed or test button is clicked:
1. Get the current monitor size
2. Calculate X position: `(monitor_width - 500) / 2` (centered horizontally)
3. Calculate Y position: `monitor_height / 3` (1/3 from top)
4. Set window position using `window.set_position()`

### Window Dragging

When user clicks the ghost:
1. Frontend calls `invoke('start_drag_window')`
2. Backend calls `window.start_dragging()`
3. Tauri handles the drag operation natively
4. Window moves with the cursor until mouse is released

### Opening Full Chat

When user clicks expand button:
1. Frontend calls `invoke('open_chat_window')`
2. Backend hides floating window
3. Backend shows and focuses main window
4. User sees full chat interface with conversation history

## Testing

To test the changes:

1. **Build and run**:
   ```bash
   cargo build --release
   cargo run
   ```

2. **Test positioning**:
   - Press the hotkey (Alt+Space or Alt+K)
   - Window should appear 1/3 from top, centered horizontally
   - Window should be 500px wide and compact (60px height initially)

3. **Test dragging**:
   - Click and hold on the ghost icon
   - Move mouse - window should follow
   - Release mouse - window stays in new position

4. **Test expand**:
   - Ask a question in floating window
   - Click the expand button (🔍 icon)
   - Main chat window should open
   - Floating window should hide

## Notes

- Window position is calculated dynamically based on monitor size
- Works with multiple monitors (uses current monitor)
- Dragging uses Tauri's native implementation (smooth and performant)
- Window dimensions match the new compact horizontal design
- All positioning happens at 1/3 from top as requested
