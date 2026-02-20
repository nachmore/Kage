# OS Abstraction Layer - Developer Guide

## Quick Start

### Using OS Functions

Instead of writing platform-specific code with `#[cfg]` attributes, simply use the OS abstraction layer:

```rust
use crate::os;

// Get cursor position
if let Some((x, y)) = os::get_cursor_position() {
    println!("Cursor at: {}, {}", x, y);
}

// Open a URL
os::open_url("https://example.com")?;

// Open a file or folder
os::open_path("/path/to/file")?;

// Scan for installed applications
let apps = os::scan_applications()?;

// Launch an application
os::launch_application(&app_path)?;

// Kill a process
if os::kill_process(pid) {
    println!("Process terminated");
}

// Configure process spawning
let mut cmd = Command::new("program");
os::configure_process_spawn(&mut cmd);
let child = cmd.spawn()?;
```

## Adding New OS-Specific Functionality

### Step 1: Define the Cross-Platform API

Create or update a file in `src/os/` (e.g., `clipboard.rs`):

```rust
// src/os/clipboard.rs

/// Get text from the system clipboard
pub fn get_clipboard_text() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::clipboard::get_clipboard_text_impl()
    }
    
    #[cfg(target_os = "macos")]
    {
        crate::os::macos::clipboard::get_clipboard_text_impl()
    }
    
    #[cfg(target_os = "linux")]
    {
        crate::os::linux::clipboard::get_clipboard_text_impl()
    }
}

/// Set text to the system clipboard
pub fn set_clipboard_text(text: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        crate::os::windows::clipboard::set_clipboard_text_impl(text)
    }
    
    #[cfg(target_os = "macos")]
    {
        crate::os::macos::clipboard::set_clipboard_text_impl(text)
    }
    
    #[cfg(target_os = "linux")]
    {
        crate::os::linux::clipboard::set_clipboard_text_impl(text)
    }
}
```

### Step 2: Implement for Each Platform

#### Windows Implementation
```rust
// src/os/windows/clipboard.rs

use anyhow::Result;

pub fn get_clipboard_text_impl() -> Option<String> {
    // Use Windows clipboard API
    // ...
}

pub fn set_clipboard_text_impl(text: &str) -> Result<()> {
    // Use Windows clipboard API
    // ...
}
```

#### macOS Implementation
```rust
// src/os/macos/clipboard.rs

use anyhow::Result;

pub fn get_clipboard_text_impl() -> Option<String> {
    // Use macOS pasteboard API
    // ...
}

pub fn set_clipboard_text_impl(text: &str) -> Result<()> {
    // Use macOS pasteboard API
    // ...
}
```

#### Linux Implementation
```rust
// src/os/linux/clipboard.rs

use anyhow::Result;

pub fn get_clipboard_text_impl() -> Option<String> {
    // Use X11 or Wayland clipboard
    // ...
}

pub fn set_clipboard_text_impl(text: &str) -> Result<()> {
    // Use X11 or Wayland clipboard
    // ...
}
```

### Step 3: Export from Platform Modules

```rust
// src/os/windows/mod.rs
pub mod clipboard;

// src/os/macos/mod.rs
pub mod clipboard;

// src/os/linux/mod.rs
pub mod clipboard;
```

### Step 4: Re-export from Main OS Module

```rust
// src/os/mod.rs
pub mod clipboard;

pub use clipboard::{get_clipboard_text, set_clipboard_text};
```

### Step 5: Use in Application Code

```rust
// src/main.rs or any other file
use crate::os;

fn copy_to_clipboard() -> Result<()> {
    os::set_clipboard_text("Hello, World!")?;
    Ok(())
}

fn paste_from_clipboard() -> Option<String> {
    os::get_clipboard_text()
}
```

## Common Patterns

### Pattern 1: Simple Function Call

When the operation is straightforward:

```rust
// Cross-platform API
pub fn do_something() -> Result<()> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::module::do_something_impl() }
    
    #[cfg(target_os = "macos")]
    { crate::os::macos::module::do_something_impl() }
    
    #[cfg(target_os = "linux")]
    { crate::os::linux::module::do_something_impl() }
}
```

### Pattern 2: Returning Platform-Specific Data

When you need to return structured data:

```rust
// Define common types in the cross-platform module
pub struct SystemInfo {
    pub os_name: String,
    pub version: String,
    pub architecture: String,
}

pub fn get_system_info() -> SystemInfo {
    #[cfg(target_os = "windows")]
    { crate::os::windows::system::get_system_info_impl() }
    
    #[cfg(target_os = "macos")]
    { crate::os::macos::system::get_system_info_impl() }
    
    #[cfg(target_os = "linux")]
    { crate::os::linux::system::get_system_info_impl() }
}
```

### Pattern 3: Configuring Objects

When you need to modify an object based on platform:

```rust
pub fn configure_window(window: &mut Window) {
    #[cfg(target_os = "windows")]
    { crate::os::windows::window::configure_window_impl(window) }
    
    #[cfg(target_os = "macos")]
    { crate::os::macos::window::configure_window_impl(window) }
    
    #[cfg(target_os = "linux")]
    { crate::os::linux::window::configure_window_impl(window) }
}
```

### Pattern 4: Optional Features

When a feature might not be available on all platforms:

```rust
pub fn get_battery_level() -> Option<f32> {
    #[cfg(target_os = "windows")]
    { crate::os::windows::power::get_battery_level_impl() }
    
    #[cfg(target_os = "macos")]
    { crate::os::macos::power::get_battery_level_impl() }
    
    #[cfg(target_os = "linux")]
    { crate::os::linux::power::get_battery_level_impl() }
    
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    { None }
}
```

## Testing

### Unit Testing Platform-Specific Code

Test each platform implementation independently:

```rust
// src/os/windows/clipboard.rs

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_clipboard_roundtrip() {
        let text = "Test text";
        set_clipboard_text_impl(text).unwrap();
        assert_eq!(get_clipboard_text_impl(), Some(text.to_string()));
    }
}
```

### Integration Testing

Test the cross-platform API:

```rust
// tests/os_integration_test.rs

use kiro_assistant::os;

#[test]
fn test_clipboard_works() {
    let text = "Integration test";
    os::set_clipboard_text(text).unwrap();
    assert_eq!(os::get_clipboard_text(), Some(text.to_string()));
}
```

### Mocking for Tests

Create a mock module for testing:

```rust
// src/os/mock/mod.rs (only compiled in test mode)

#[cfg(test)]
pub mod clipboard {
    use std::sync::Mutex;
    
    static CLIPBOARD: Mutex<Option<String>> = Mutex::new(None);
    
    pub fn get_clipboard_text_impl() -> Option<String> {
        CLIPBOARD.lock().unwrap().clone()
    }
    
    pub fn set_clipboard_text_impl(text: &str) -> Result<()> {
        *CLIPBOARD.lock().unwrap() = Some(text.to_string());
        Ok(())
    }
}
```

## Best Practices

### DO ✅

1. **Keep platform code isolated**
   ```rust
   // Good: All Windows code in windows/ directory
   // src/os/windows/feature.rs
   ```

2. **Use descriptive function names**
   ```rust
   // Good
   pub fn get_cursor_position() -> Option<(i32, i32)>
   
   // Bad
   pub fn cursor() -> Option<(i32, i32)>
   ```

3. **Return Result for operations that can fail**
   ```rust
   // Good
   pub fn open_url(url: &str) -> Result<()>
   
   // Bad
   pub fn open_url(url: &str) -> bool
   ```

4. **Document platform-specific behavior**
   ```rust
   /// Get the cursor position in screen coordinates.
   /// 
   /// # Platform-specific behavior
   /// - Windows: Uses GetCursorPos from Win32 API
   /// - macOS: Uses CoreGraphics (TODO)
   /// - Linux: Uses X11/Wayland (TODO)
   /// 
   /// Returns None if the cursor position cannot be determined.
   pub fn get_cursor_position() -> Option<(i32, i32)>
   ```

5. **Use common types across platforms**
   ```rust
   // Good: Define shared types
   pub struct AppInfo {
       pub name: String,
       pub path: PathBuf,
       pub icon_path: Option<String>,
   }
   ```

### DON'T ❌

1. **Don't put platform-specific code in main application files**
   ```rust
   // Bad: Platform code in main.rs
   #[cfg(target_os = "windows")]
   fn do_something() { /* ... */ }
   
   // Good: Use OS abstraction
   fn do_something() {
       os::do_something()
   }
   ```

2. **Don't duplicate logic across platforms**
   ```rust
   // Bad: Same logic in multiple platform files
   
   // Good: Extract common logic to a shared function
   ```

3. **Don't use platform-specific types in cross-platform APIs**
   ```rust
   // Bad
   #[cfg(target_os = "windows")]
   pub fn get_window_handle() -> HWND
   
   // Good
   pub fn get_window_handle() -> WindowHandle
   ```

4. **Don't forget error handling**
   ```rust
   // Bad
   pub fn open_file(path: &str) {
       // What if it fails?
   }
   
   // Good
   pub fn open_file(path: &str) -> Result<()> {
       // Proper error handling
   }
   ```

## Troubleshooting

### Compilation Errors

**Error: "cannot find function in module `os`"**
- Make sure you've re-exported the function in `src/os/mod.rs`
- Check that all platform modules export the implementation

**Error: "no rules expected this token in macro call"**
- Check your `#[cfg]` attributes syntax
- Make sure you're using `target_os` not `target_platform`

### Runtime Issues

**Function returns None unexpectedly**
- Check if the platform implementation is complete
- Add logging to see which platform code is being called
- Verify the platform-specific API is working correctly

**Wrong platform code being called**
- Verify your `#[cfg(target_os = "...")]` attributes
- Check that you're compiling for the correct target

## Examples

See the existing implementations for reference:
- `src/os/cursor.rs` - Simple function returning Option
- `src/os/launcher.rs` - Complex operations with structured data
- `src/os/process.rs` - Object configuration pattern
- `src/os/shell.rs` - Simple operations with error handling

## Getting Help

- Check existing platform implementations for patterns
- Review the architecture documentation in `docs/OS_ARCHITECTURE.md`
- Look at the refactoring summary in `REFACTORING_SUMMARY.md`
