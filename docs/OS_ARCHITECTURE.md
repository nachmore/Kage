# OS Abstraction Layer Architecture

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                    Application Layer                         │
│  (main.rs, app_launcher.rs, process_manager.rs, etc.)      │
└────────────────────┬────────────────────────────────────────┘
                     │
                     │ Uses clean API
                     ▼
┌─────────────────────────────────────────────────────────────┐
│                  OS Abstraction Layer                        │
│                     (src/os/)                                │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │  cursor.rs   │  │ launcher.rs  │  │ process.rs   │     │
│  │              │  │              │  │              │     │
│  │ - get_cursor │  │ - scan_apps  │  │ - kill_proc  │     │
│  │   _position  │  │ - launch_app │  │ - configure  │     │
│  └──────────────┘  └──────────────┘  └──────────────┘     │
│                                                              │
│  ┌──────────────┐                                           │
│  │   shell.rs   │                                           │
│  │              │                                           │
│  │ - open_url   │                                           │
│  │ - open_path  │                                           │
│  └──────────────┘                                           │
└────────────────────┬────────────────────────────────────────┘
                     │
                     │ Dispatches to platform
                     ▼
┌─────────────────────────────────────────────────────────────┐
│              Platform Implementations                        │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │   Windows    │  │    macOS     │  │    Linux     │     │
│  │              │  │              │  │              │     │
│  │ • Win32 API  │  │ • CoreGraphics│ │ • X11/Wayland│     │
│  │ • Registry   │  │ • /Applications│ │ • .desktop   │     │
│  │ • taskkill   │  │ • signals    │  │ • signals    │     │
│  │ • cmd/explorer│ │ • open cmd   │  │ • xdg-open   │     │
│  └──────────────┘  └──────────────┘  └──────────────┘     │
└─────────────────────────────────────────────────────────────┘
```

## Call Flow Example

### Opening a URL

```
User Action
    │
    ▼
main.rs::open_url()
    │
    ▼
os::open_url(url)  ◄─── Clean, platform-agnostic call
    │
    ├─── Windows? ──► os::windows::shell::open_url_impl()
    │                     │
    │                     └─► cmd /C start <url>
    │
    ├─── macOS? ───► os::macos::shell::open_url_impl()
    │                     │
    │                     └─► open <url>
    │
    └─── Linux? ───► os::linux::shell::open_url_impl()
                          │
                          └─► xdg-open <url>
```

### Scanning Applications

```
AppLauncher::refresh_registry()
    │
    ▼
os::scan_applications()  ◄─── Returns Vec<AppInfo>
    │
    ├─── Windows? ──► os::windows::launcher::scan_applications_impl()
    │                     │
    │                     ├─► Scan Start Menu (.lnk files)
    │                     ├─► Scan Registry (HKLM\Software\...)
    │                     └─► Return Vec<AppInfo>
    │
    ├─── macOS? ───► os::macos::launcher::scan_applications_impl()
    │                     │
    │                     ├─► Scan /Applications (*.app)
    │                     └─► Return Vec<AppInfo>
    │
    └─── Linux? ───► os::linux::launcher::scan_applications_impl()
                          │
                          ├─► Scan .desktop files
                          └─► Return Vec<AppInfo>
```

## Key Design Principles

### 1. Single Responsibility
Each module has one clear purpose:
- `cursor.rs` - Only cursor position
- `launcher.rs` - Only app discovery and launching
- `process.rs` - Only process management
- `shell.rs` - Only shell operations

### 2. Consistent Interface
All platforms implement the same function signatures:
```rust
// Every platform must implement:
pub fn get_cursor_position_impl() -> Option<(i32, i32)>
pub fn scan_applications_impl() -> Result<Vec<AppInfo>>
pub fn launch_application_impl(path: &PathBuf) -> Result<()>
pub fn kill_process_impl(pid: u32) -> bool
// etc.
```

### 3. Compile-Time Selection
Platform selection happens at compile time using `#[cfg]`:
```rust
#[cfg(target_os = "windows")]
{
    crate::os::windows::cursor::get_cursor_position_impl()
}
```

### 4. Zero Runtime Overhead
- No dynamic dispatch
- No trait objects
- Direct function calls
- Compiler optimizes away the abstraction

### 5. Easy Extension
To add a new platform:
1. Create `src/os/newplatform/` directory
2. Implement required modules
3. Add `#[cfg(target_os = "newplatform")]` in `src/os/mod.rs`
4. Done! No changes to application code needed

## File Organization

```
src/os/
├── mod.rs                    # Platform selection & re-exports
├── cursor.rs                 # Cross-platform cursor API
├── launcher.rs               # Cross-platform launcher API
├── process.rs                # Cross-platform process API
├── shell.rs                  # Cross-platform shell API
│
├── windows/
│   ├── mod.rs               # Windows module exports
│   ├── cursor.rs            # Win32 GetCursorPos
│   ├── launcher.rs          # Start Menu + Registry scanning
│   ├── process.rs           # taskkill, CREATE_NO_WINDOW
│   └── shell.rs             # cmd, explorer
│
├── macos/
│   ├── mod.rs               # macOS module exports
│   ├── cursor.rs            # CoreGraphics (TODO)
│   ├── launcher.rs          # /Applications scanning
│   ├── process.rs           # POSIX signals, setsid
│   └── shell.rs             # open command
│
└── linux/
    ├── mod.rs               # Linux module exports
    ├── cursor.rs            # X11/Wayland (TODO)
    ├── launcher.rs          # .desktop file parsing
    ├── process.rs           # POSIX signals, setsid
    └── shell.rs             # xdg-open
```

## Benefits Summary

| Aspect | Before | After |
|--------|--------|-------|
| **Code Location** | Scattered across files | Centralized in `src/os/` |
| **Platform Logic** | Mixed with business logic | Isolated in platform modules |
| **Readability** | `#[cfg]` blocks everywhere | Clean function calls |
| **Maintainability** | Hard to find platform code | Easy to locate and update |
| **Testing** | Difficult to mock | Easy to mock OS layer |
| **New Platforms** | Touch many files | Add one directory |
| **Code Duplication** | High | Low |

## Future Enhancements

### Short Term
- [ ] Implement macOS cursor position (CoreGraphics)
- [ ] Implement Linux cursor position (X11/Wayland)
- [ ] Add icon extraction for all platforms
- [ ] Add process priority management

### Medium Term
- [ ] Add window management utilities
- [ ] Add file association queries
- [ ] Add system tray abstractions
- [ ] Add clipboard operations

### Long Term
- [ ] Add BSD support
- [ ] Add Android/iOS support (if needed)
- [ ] Add WebAssembly stubs (for testing)
