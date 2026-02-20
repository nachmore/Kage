# Pattern Recognition for Floating Window Input

The floating window input now supports intelligent pattern recognition to automatically detect and handle different types of input.

## Supported Patterns

### 1. Application Names
When you type an application name, the system will search for matching applications and display suggestions.

**Example:**
- Type: `chrome`
- Result: Shows Chrome browser in suggestions
- Action: Press Enter to launch

### 2. URLs
When you type a URL, the system will automatically detect it and offer to open it in your default browser.

**Supported URL formats:**
- `http://example.com`
- `https://example.com`
- `ftp://example.com`
- `file:///path/to/file`
- `www.example.com` (automatically adds https://)

**Example:**
- Type: `https://github.com`
- Result: Shows "Open URL: https://github.com" with globe icon 🌐
- Action: Press Enter to open in default browser

### 3. File and Folder Paths
When you type a file or folder path, the system will detect it and offer to open it with the appropriate application.

**Windows paths:**
- Absolute: `C:\Users\Username\Documents`
- UNC: `\\server\share\folder`
- Relative: `.\folder\file.txt`

**Linux/macOS paths:**
- Absolute: `/home/username/documents`
- Home: `~/documents`
- Relative: `./folder/file.txt`

**Example:**
- Type: `C:\Users\Documents`
- Result: Shows "Open Folder: C:\Users\Documents" with folder icon 📁
- Action: Press Enter to open in File Explorer (Windows) / Finder (macOS) / File Manager (Linux)

**File detection:**
- Files are detected by the presence of a file extension (e.g., `.txt`, `.pdf`)
- Shows file icon 📄 instead of folder icon

## How It Works

The pattern recognition follows this priority order:

1. **URL Detection** - Checks if input starts with `http://`, `https://`, `ftp://`, `file://`, or `www.`
2. **Path Detection** - Checks if input matches OS-specific path patterns
3. **Application Search** - Searches for matching applications in the registry
4. **Chat Mode** - If no pattern matches, opens the full chat interface

## User Interface

When a pattern is detected:
- An auto-suggest dropdown appears below the input
- The suggestion shows an appropriate icon (🌐 for URLs, 📁 for folders, 📄 for files)
- The suggestion is automatically selected
- Press Enter to execute the action
- Press Escape to cancel

## Implementation Details

### Rust Backend (`src/main.rs`)
- `is_url()` - Detects URL patterns
- `is_path()` - Detects file/folder paths (OS-specific)
- `handle_floating_input()` - Main pattern recognition logic
- `open_url()` - Opens URLs in default browser
- `open_path()` - Opens paths in default file manager/application

### JavaScript Frontend (`ui/floating.html`)
- `renderUrlSuggestion()` - Displays URL suggestions
- `renderPathSuggestion()` - Displays path suggestions
- `openUrl()` - Invokes Rust command to open URL
- `openPath()` - Invokes Rust command to open path

## Platform-Specific Behavior

### Windows
- Uses `explorer` to open paths
- Uses `cmd /C start` to open URLs
- Supports both forward slashes and backslashes in paths

### macOS
- Uses `open` command for both URLs and paths
- Supports `~` for home directory

### Linux
- Uses `xdg-open` for both URLs and paths
- Supports `~` for home directory
