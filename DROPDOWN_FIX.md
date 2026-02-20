# Dropdown Menu Fix

## Problem
The app suggestions dropdown was hidden behind the window boundary. When typing to search for apps, the dropdown would appear but be cut off because the floating window had a fixed height of 60px.

## Solution
Implemented dynamic window resizing that grows the window to accommodate content:

### 1. Frontend Changes (ui/floating.html)

#### Added `resizeWindow()` function
```javascript
async function resizeWindow() {
    try {
        const container = document.querySelector('.floating-container');
        const contentHeight = container.offsetHeight;
        const width = 500;
        
        // Add some padding and ensure minimum height
        const height = Math.max(60, Math.min(400, contentHeight + 10));
        
        await invoke('resize_floating_window', { width, height });
    } catch (error) {
        console.error('Error resizing window:', error);
    }
}
```

#### Calls `resizeWindow()` when:
- App suggestions are shown
- App suggestions are hidden
- Content area is shown (response)
- Content area is hidden
- Input is cleared
- Error is displayed

#### CSS Changes
- Removed `max-height: 400px` from `.speech-bubble`
- Changed to `min-height: 60px` to allow growth
- Removed `margin-top: 8px` from `.app-suggestions` (now flush with input)
- Changed dropdown `border-radius` to `0 0 12px 12px` (rounded bottom only)
- Added `max-height: 300px` to `.content-area` for response scrolling

### 2. Backend Changes (src/main.rs)

#### Added `resize_floating_window` command
```rust
#[tauri::command]
async fn resize_floating_window(window: Window, width: u32, height: u32) -> Result<(), String> {
    info!("Resizing floating window to {}x{}", width, height);
    window.set_size(tauri::Size::Physical(tauri::PhysicalSize { width, height }))
        .map_err(|e| {
            error!("Failed to resize window: {}", e);
            e.to_string()
        })
}
```

#### Registered in invoke_handler
Added `resize_floating_window` to the list of available commands.

## How It Works

1. **Initial state**: Window is 500x60px (compact)

2. **User types "word"**: 
   - Dropdown appears with matching apps
   - `resizeWindow()` is called
   - Window grows to fit dropdown (e.g., 500x150px)

3. **User selects app or clears input**:
   - Dropdown disappears
   - `resizeWindow()` is called
   - Window shrinks back to 500x60px

4. **User asks a question**:
   - Response appears in content area
   - `resizeWindow()` is called
   - Window grows to fit response (up to 400px max)

## Benefits

- Dropdown is fully visible
- No scrolling within the window itself
- Window size adapts to content
- Smooth transitions
- Maximum height prevents window from becoming too large
- Minimum height ensures window never disappears

## Testing

1. **Test dropdown**:
   - Type "word" or any app name
   - Dropdown should appear below input
   - Window should grow to show all matches
   - Press down arrow - should see selection
   - Input box should remain visible

2. **Test response**:
   - Ask a question
   - Response should appear
   - Window should grow to fit response
   - Should scroll if response is very long

3. **Test transitions**:
   - Clear input - window should shrink
   - Type again - window should grow
   - All transitions should be smooth
