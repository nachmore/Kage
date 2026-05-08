# Shortcuts Guide

## Overview

Shortcuts allow you to execute commands directly from the Kage floating window without sending them to the LLM. This is perfect for quick access to frequently used tools, scripts, or applications.

## Features

- **Quick Execution**: Type a shortcut trigger and press Enter to run the command instantly
- **Argument Support**: Pass dynamic arguments to your shortcuts
- **Visual Feedback**: Shortcuts appear in the suggestion dropdown as you type
- **Import/Export**: Share shortcuts with your team via JSON files
- **Full Management UI**: Add, edit, delete shortcuts from the settings panel

## Configuration

### Accessing Shortcuts Settings

1. Open Kage settings (system tray → Settings)
2. Click on "Shortcuts" in the left sidebar (⚡ icon)
3. Click "Add Shortcut" to create a new one

### Shortcut Parameters

Each shortcut has the following fields:

| Field | Required | Description |
|-------|----------|-------------|
| **Name / Description** | Yes | A friendly name describing what the shortcut does |
| **Shortcut** | Yes | The trigger word you'll type (e.g., "code", "git", "google") |
| **Action Type** | Yes | One of: `run_program`, `open_url`, `prompt`, `text`, `script` |
| **Icon** | No | Emoji or PNG/JPG data URI shown next to the shortcut |

#### `run_program` — launch an executable

| Field | Required | Description |
|-------|----------|-------------|
| **Path** | Yes | Full path to the executable to run |
| **Working Directory** | No | Directory to run the command in |
| **Arguments** | No | Arguments to pass (supports `{*}` and `{0}`/`{1}`/...) |

#### `open_url` — open a URL in the default browser

| Field | Required | Description |
|-------|----------|-------------|
| **URL** | Yes | URL to open (arguments URL-encoded into `{*}`/`{N}` slots) |

#### `prompt` — send a templated prompt to the agent

| Field | Required | Description |
|-------|----------|-------------|
| **Prompt** | Yes | Prompt template; `{*}`/`{N}` substitute the user's args |

#### `text` — paste literal text

| Field | Required | Description |
|-------|----------|-------------|
| **Prompt** | Yes | Text to paste at the cursor; `{*}`/`{N}` substituted |

#### `script` — run a JS function body and feed the result to a follow-up action

| Field | Required | Description |
|-------|----------|-------------|
| **Script** | Yes | JS function body. The args are bound; return a string. |
| **Script Action** | Yes | What to do with the return value: `run_program`, `open_url`, `prompt`, or `text` |

### Argument Templates

The Arguments field (for Run Program) and URL field (for Open URL) support special placeholders:

- **`{*}`** - All arguments after the shortcut
  - Example: `google hello world` with URL `https://google.com/search?q={*}` → opens `https://google.com/search?q=hello world`
  
- **`{0}`, `{1}`, `{2}`, etc.** - Specific argument by position
  - Example: `gh rust lang` with URL `https://github.com/{0}/{1}` → opens `https://github.com/rust/lang`

**Note:** For URLs, arguments are automatically URL-encoded to handle special characters safely.

## Examples

The examples below use Windows paths for concreteness, but `run_program`
works the same on macOS and Linux — substitute your platform's executable
location. Representative equivalents:

| Program | Windows | macOS |
|---|---|---|
| VS Code | `C:\Program Files\Microsoft VS Code\Code.exe` | `/Applications/Visual Studio Code.app/Contents/MacOS/Electron` or `open -a "Visual Studio Code"` |
| Git | `C:\Program Files\Git\bin\git.exe` | `/usr/bin/git` (or `/opt/homebrew/bin/git`) |
| Terminal shell | `C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe` | `/bin/zsh` |

On macOS you can also use `open` with an `-a <app>` argument to launch
applications by name without hardcoding the bundle path.

### Example 1: Open VSCode in a Directory

```json
{
  "name": "Open VSCode",
  "shortcut": "code",
  "path": "C:\\Program Files\\Microsoft VS Code\\Code.exe",
  "working_directory": "C:\\Projects",
  "arguments": "{*}"
}
```

Usage: `code myproject` opens VSCode with the myproject folder

### Example 2: Google Search

```json
{
  "name": "Google Search",
  "shortcut": "google",
  "action_type": "open_url",
  "url": "https://www.google.com/search?q={*}"
}
```

Usage: `google rust programming` opens Google search for "rust programming"

### Example 3: GitHub Repository

```json
{
  "name": "GitHub Repo",
  "shortcut": "gh",
  "action_type": "open_url",
  "url": "https://github.com/{0}/{1}"
}
```

Usage: `gh microsoft vscode` opens `https://github.com/microsoft/vscode`

### Example 4: Git Commands

```json
{
  "name": "Git Status",
  "shortcut": "gs",
  "path": "C:\\Program Files\\Git\\bin\\git.exe",
  "working_directory": "C:\\Projects\\MyRepo",
  "arguments": "status"
}
```

Usage: `gs` runs `git status` in the specified directory

### Example 5: Custom Script with Arguments

```json
{
  "name": "Deploy Script",
  "shortcut": "deploy",
  "path": "C:\\Scripts\\deploy.bat",
  "arguments": "--env {0} --branch {1}"
}
```

Usage: `deploy prod main` runs `deploy.bat --env prod --branch main`

### Example 6: Stack Overflow Search

```json
{
  "name": "Stack Overflow Search",
  "shortcut": "so",
  "action_type": "open_url",
  "url": "https://stackoverflow.com/search?q={*}"
}
```

Usage: `so javascript promises` searches Stack Overflow for "javascript promises"

### Example 7: Open Terminal

```json
{
  "name": "Open PowerShell",
  "shortcut": "ps",
  "path": "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
  "working_directory": "C:\\Projects"
}
```

Usage: `ps` opens PowerShell in the Projects directory

## Import/Export

### Exporting Shortcuts

1. Go to Settings → Shortcuts
2. Click "Export to JSON"
3. Save the file to share with others

### Importing Shortcuts

1. Go to Settings → Shortcuts
2. Click "Import from JSON"
3. Select a shortcuts JSON file
4. Your shortcuts will be merged with existing ones

### JSON Format

```json
[
  {
    "name": "Example Shortcut",
    "shortcut": "ex",
    "path": "C:\\path\\to\\executable.exe",
    "working_directory": "C:\\optional\\work\\dir",
    "arguments": "{*}"
  }
]
```

## Tips

1. **Keep shortcuts short**: Use 2-4 character triggers for quick typing
2. **Use consistent naming**: Prefix related shortcuts (e.g., `git-status`, `git-pull`)
3. **Test arguments**: Try your shortcuts with different arguments to ensure they work
4. **Share with team**: Export and share shortcuts for common team workflows
5. **Use absolute paths**: Always use full paths to executables for reliability

## Troubleshooting

### Shortcut doesn't execute

- Verify the executable path is correct
- Check that the executable has proper permissions
- Ensure working directory exists (if specified)
- Check the application logs for error messages

### Arguments not working

- Make sure you're using the correct placeholder syntax (`{*}` or `{0}`, `{1}`, etc.)
- Test the command manually in a terminal first
- Check for special characters that might need escaping

### Shortcut not appearing in suggestions

- Verify the shortcut trigger matches what you're typing
- Check that the shortcut was saved (click "Save Settings")
- Restart the application if needed
