# Kage user guide

Everything Kage can do, in more detail than the [README](../README.md). On a Mac, <kbd>Ctrl</kbd> in the in-app shortcuts below means <kbd>⌘</kbd>, and <kbd>Alt</kbd> means <kbd>⌥</kbd>.

- [The launcher](#the-launcher)
- [Agents](#agents)
- [Chat and tool calls](#chat-and-tool-calls)
- [Tool permissions](#tool-permissions)
- [Inline assist and your selection](#inline-assist-and-your-selection)
- [Calendar](#calendar)
- [Commands, prompts and automations](#commands-prompts-and-automations)
- [Computer control](#computer-control)
- [Voice and read-aloud](#voice-and-read-aloud)
- [Appearance, hotkeys and the cat](#appearance-hotkeys-and-the-cat)
- [Languages](#languages)
- [Updates](#updates)
- [Everything else](#everything-else)

## The launcher

Press <kbd>Alt</kbd>+<kbd>Space</kbd> (<kbd>Option</kbd>+<kbd>Space</kbd> on a Mac) over any app. What you type is matched on your machine first; only text that matches nothing goes to the agent.

- **Apps.** Type part of a name. Results show the app's icon and learn which one you pick for each thing you type. New installs show up as soon as the installer finishes, because Kage watches the folders installers write to, with an hourly rescan as a fallback.
- **Instant answers** from the extensions setup installs: math and unit conversions (`sqrt(2) * 15`, `12 km to mi`), timers (`timer 25m`, `stopwatch`), UUIDs, base64, hashes, epoch times, JSON formatting and color conversions run locally. Word definitions (`dict serendipity`) are looked up online; see [Privacy](../README.md#privacy-and-security).
- **Folders.** `down` opens Downloads. The same works for Documents, Desktop, Pictures, Videos, Music, your home folder and a few others; prefixes are enough. Settings → System Integration lists every keyword.
- **Files and links.** Paste a path or a URL to open it. To search for files, type a name with an extension, a wildcard (`*.pdf`) or `>find report`. Kage uses Spotlight on macOS and Windows Search on Windows.
- **System commands.** `lock`, `sleep`, `screenshot`, `mute`, `unmute`, `emoji`, `trash` (Trash or Recycle Bin), `taskmanager` (Activity Monitor or Task Manager), `terminal`, and `finder` or `explorer`. Sleep, restart, shut down and sign out ask before they act. On Windows, <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Enter</kbd> runs a system command, such as a terminal, as administrator.
- **Clipboard history (Windows).** `>cb` searches Windows' clipboard history and pastes the item you pick into the app you were using. Images are listed, but only text items can be pasted. Turn on clipboard history in Windows Settings → System → Clipboard first.
- **Sending to the AI.** <kbd>Enter</kbd> on text that isn't a match sends it to your agent. <kbd>Ctrl</kbd>+<kbd>Enter</kbd> always skips the suggestions and sends straight to the agent.

### Keyboard shortcuts

| Keys | What they do |
|---|---|
| <kbd>Alt</kbd>+<kbd>Space</kbd> | Open Kage from any app (change it in Settings → Hotkey & Shortcuts) |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Space</kbd> | [Inline assist](#inline-assist-and-your-selection) on the selected text (<kbd>⌃</kbd><kbd>⇧</kbd><kbd>Space</kbd> on a Mac) |
| <kbd>Enter</kbd> | Run the highlighted result, or send your message |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | New line |
| <kbd>Ctrl</kbd>+<kbd>Enter</kbd> | Send to the agent, skipping suggestions |
| <kbd>Esc</kbd> | Stop the reply, or hide the window |
| <kbd>↑</kbd> | Bring back an earlier prompt |
| <kbd>Alt</kbd>+<kbd>↓</kbd> | Open the calendar |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | Expand the conversation into the full chat window |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> | Clear and reset the launcher |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> | Copy the last reply |
| <kbd>Ctrl</kbd>+<kbd>,</kbd> | Open Settings |
| <kbd>Ctrl</kbd>+<kbd>N</kbd> | New session (chat window) |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>N</kbd> | New chat window (chat window) |
| <kbd>Ctrl</kbd>+<kbd>W</kbd> | Close or hide the window |

You can also set a global hotkey for voice input and, on Windows, one for clipboard history.

### `>` commands

| Command | What it does |
|---|---|
| `>chats` | Open the chat sessions window |
| `>settings` | Open Settings |
| `>store` | Open the Extension Store |
| `>find <name>` | Search for files |
| `>cb` | Clipboard history (Windows) |
| `>prompts` | Browse your saved prompts |
| `>logs` | Open the logs |
| `>version` | Show the version and check for updates |
| `>session-title <name>` | Rename the current session |
| `>session-id` | Show the current session's ID |
| `>session-folder` | Show the current session's file on disk |
| `>clear-ux` | Clear the visible reply (your conversation history stays) |
| `>restart`, `>quit` | Restart or quit Kage |

Your agent's own slash commands (for example `/model` or `/agent`) also show up, with pickers, in both the launcher and the chat window. (`>inspect`, which opens the developer tools, only does anything in development builds.)

## Agents

Kage doesn't include a model. It starts the agent you choose and talks to it over the [Agent Client Protocol](https://agentclientprotocol.com) (ACP). The README's [agent table](../README.md#bring-your-own-agent) lists what Kage detects and sets up for you.

- **Custom agents.** Add any other ACP agent as a command line, or connect to an agent running on another machine by host and port.
- **Several connections.** Save as many as you like in Settings → Agents and switch with **Make active**. Switching takes effect after a restart.
- **Workspace folder.** Set the folder the agent works in. It can read and modify files under that path.
- **Start early.** Start the agent when Kage starts, so the first reply comes sooner.
- **Your shell's environment (macOS).** A Kage opened from the Dock or Finder doesn't get your terminal's `PATH`. Kage reads your login shell's startup files once in the background, so the agent and its MCP servers can find your tools. You can turn this off in Settings → Agents. See [Agent tools and your shell environment](AGENT_ENVIRONMENT.md).
- **Ollama** runs through the [`codex-acp`](https://github.com/zed-industries/codex-acp) adapter, so install that as well as [Ollama](https://ollama.com/download). Then use Settings → Agents → **+ Add agent** → Ollama to set the address, pick a model and test the connection.

## Chat and tool calls

Replies stream into the launcher, or into the full chat window (<kbd>Ctrl</kbd>+<kbd>E</kbd> or `>chats`) when you want more room.

- **Tool chips.** Under each reply, chips group what the agent did: read, edit, delete, move, search, fetch, think, or the tools of a particular MCP server. Each chip shows whether it's still running or failed. Expand one to see file names and line ranges, edit diffs with lines added and removed, search queries, URLs, arguments, results and errors.
- **Shell commands** appear as `$ command` blocks with a live spinner and the exit code. Expanding one shows the exact command, with hidden characters made visible, plus stdout and stderr and buttons to copy either.
- **Blocked by the agent's sandbox.** Agents run commands inside a sandbox of their own, which your approvals in Kage can't lift. When a command fails because of it (say a `brew install` that ends in `sandbox_apply: Operation not permitted`), its block says so, shows the whole command for you to read first, and offers **Copy command** and **Copy & open Terminal**, which copies the command and opens your terminal app for you to paste and run it. A command with hidden characters gets a warning and no Copy & open Terminal. Kage never runs it for you.
- **Rich replies.** Markdown with highlighted code, KaTeX math, Mermaid and Graphviz diagrams, SVG, a sandboxed HTML preview, collapsible JSON, and sortable tables (including CSV and TSV).
- **Sessions.** Search, rename (double-click the title), show on disk or delete. Dots mark sessions that are still streaming or have unread replies. Session titles are written for you in the background. The **Other Agents** tab shows read-only history from Kiro CLI, Kiro IDE and Claude Code sessions on your disk.
- **The toolbar.** A model picker, a ring showing how much of the context is used (with automatic compaction at a threshold you set), and a stop button. Attach files and images, read a reply aloud, translate it, or export the chat as Markdown. You can open several chat windows at once.
- **Plans for bigger jobs.** For multi-step computer tasks, the agent writes a plan that you review as a task list. Press **Run**, or type to change it. Each step then runs in a fresh sub-agent.
- **Personal touches.** Kage can learn your preferences as you chat; view, edit or turn them off in Settings → Personalization. Add your own standing instructions, or per-app instructions with **App Modes**, which come with starter rules for code editors, terminals, browsers and email.
- **Picking up where you left off.** The chat window remembers its size and position, and Kage brings you back to your session after an update.

## Tool permissions

Kage asks before the agent uses any tool you haven't allowed yet.

- **Allow** once, for 24 hours, or always. An "always" that goes unused for 30 days is asked again. **Deny** once or always.
- For shell commands, **Trust all commands…** asks you to confirm a warning first. For MCP tools, **Trust all tools from *server*…** trusts one server. A tool set to **Always deny** stays denied either way.
- That trust doesn't cover background features (inline assist, automations, script generation). Those still ask.
- **Settings → Agent Permissions** lists every tool Kage has seen with its policy, and every server you trust, each with a **Stop trusting** button. It also has **Reset All Permissions** and a local audit log of every grant, denial, revoke and expiry.
- **Terminator Mode** approves every tool you haven't set to **Always deny**, until you switch it off. A banner reminds you while it's on, and the cat dresses for the part.

The full rules are in [Tool Permissions](TOOL_PERMISSIONS.md).

## Inline assist and your selection

Select text in any app and press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Space</kbd>. A small panel opens at the cursor with actions that suit the text, or you can type your own request in the **Ask anything** box.

- Edits such as **Translate**, **Format**, **Add comments** and **Suggest fix**, and your own requests, are pasted back where you were typing.
- Answers such as **Summarize**, **Explain**, **Find bugs** and **Convert units** open in the chat window.
- With nothing selected, it offers **Summarize page** and **Help with this app**. **Open full chat** moves the conversation to the chat window.

The main launcher uses your selection too:

- When you open Kage, it copies the text selected in the app you came from. Tick **Include selected text** to send it with your message, or use `{selection}` in your own commands.
- Kage won't send the copy shortcut in apps on a skip list (Settings → System Integration). Terminals are on it by default, because <kbd>Ctrl</kbd>+<kbd>C</kbd> would interrupt a running command.
- Quick-action chips appear for selected text and are chosen by what the text is: Summarize, Translate, Explain, Find bugs, Explain error, Suggest fix, Format, Convert units and more. After the agent proposes a folder clean-up you get **Looks good, do it**, **More details** and **Undo changes**. You can add your own actions and pick the Translate language.

**Screen context.** By default, prompts from the launcher and from inline assist start with the name and window title of the app you came from, so the agent knows what you were looking at. Window titles can include document names, email subjects or file paths. Turn this off in Settings → System Integration → **Screen context awareness**. The same page has **Show notifications**, on by default, which notifies you when a reply finishes while Kage is hidden.

## Calendar

Turn on the clock and date in Settings → Appearance; click the date, or press <kbd>Alt</kbd>+<kbd>↓</kbd> at any time, to open a month calendar that follows your locale and first day of the week and works fully from the keyboard.

With the **Calendar** extension, which setup installs:

- A next-meeting bar with a **Join** button that picks up meeting links.
- Dots on busy days. Pick a day to see its meetings, or type `cal` or `meetings`.
- Your agent can look up your appointments, so you can ask things like "what's on tomorrow?".
- Automations can start when a meeting is about to begin.

Meetings come from the Calendar app on macOS (Kage asks for Calendars access) and from classic Outlook for desktop on Windows.

## Commands, prompts and automations

**Commands & Prompts** (Settings → Commands & Prompts) are trigger words you type in the launcher. Each one does one of these:

| Action | For example |
|---|---|
| **Run Program** | Open your editor in a project folder, with arguments |
| **Open URL** | A `yt` command, so `yt cats` searches YouTube for cats |
| **Send Prompt to Agent** | A `tldr` command that sends "Summarize this in three bullet points: {selection}" |
| **Run Script** | A JavaScript snippet whose result is shown, sent to the agent, opened as a URL or run as a command |

- Use `{*}` for everything typed after the trigger, `{0}`, `{1}` … for single words, `{selection}` for the selected text, and `{name}` to have Kage pop up a small form. Arguments placed in URLs are encoded for you.
- Icons can be an emoji, an image or the site's favicon.
- Kage can write or update a script for you, and you can test it in place.
- Commands import and export as JSON, `>prompts` lists your saved prompts, and Kage remembers the arguments you used before.

More in the [Shortcuts Guide](SHORTCUTS_GUIDE.md).

**Automations** (Settings → Automations) are chains of steps:

- **Steps:** an AI prompt (which can use the previous step's output as `{input}`), find & replace with regular expressions, text transforms (change case, trim, sort, reverse, de-duplicate or number lines, count words, base64), conditions and scripts.
- **When they run:** when you choose; on a schedule (hourly, daily on the days you pick, monthly or yearly); or on a signal from an extension, such as Calendar's "meeting about to start" or a todo falling due in Todos & Reminders.
- **On battery,** a power setting can slow scheduled runs down.

## Computer control

Turn this on during setup, or later with the built-in server's switch in Settings → MCP Servers, and your agent gets Kage's MCP server for working with your desktop through accessibility APIs rather than screenshots. It can:

- launch apps and list installed ones;
- read an app's interface, and click, focus, expand, toggle and fill in its buttons and fields;
- move the mouse, scroll, type text and press keys;
- list windows, minimized ones included;
- run a PowerShell or bash script;
- scan and tidy folders. Clean-ups move files to a `_kage_trash` folder instead of deleting them;
- ask Kage what's new in the latest version.

A few destructive key combinations (such as <kbd>Alt</kbd>+<kbd>F4</kbd>) need a separate, explicitly confirmed tool call, which goes through your permission prompt like any other tool.

**Which agents get it.** Setup and the switch register the server in Kiro CLI's MCP config (`~/.kiro/settings/mcp.json`). For any other agent, add `kage-computer-control-mcp` in that agent's own MCP settings. The program sits next to the Kage executable (on macOS, inside `Kage.app/Contents/MacOS`). Settings → MCP Servers also lets you view and edit the other servers in an `mcp.json` file: Kiro CLI's, or another file you pick. Why it's a separate program: [Computer Control MCP](COMPUTER_CONTROL_MCP.md).

## Voice and read-aloud

- **Voice input.** Turn on the microphone button in Settings → Speech (**Show Speech Button**), and optionally set a voice hotkey. Kage turns your speech into text with the system web view's speech recognition, where it's available, and can send it after you pause.
- **Read-aloud.** Replies can be read aloud with your system voices, or with **Pocket TTS**, a local AI voice engine. Pocket TTS needs Python 3.10 or later and a one-time model download of about 400 MB, and it can use custom voices.
- Kage can also read replies aloud automatically when you ask by voice.

## Appearance, hotkeys and the cat

- **Themes.** Kage Light, Kage Dark, or Kage (Auto Light / Dark), which follows your OS. Setup also installs Nord, Sunset and Kiro‑ish from the store. Changes apply to every window without a restart.
- **The launcher.** Choose where it opens (center of the active screen, next to the mouse, or where you left it), plus opacity, font size, whether it remembers its size, whether the last reply stays visible, the clock and date and their format, and the attachment toolbar.
- **Hotkeys.** Change the main hotkey and the inline-assist hotkey, and add hotkeys for voice input and, on Windows, clipboard history.
- **⌘Space on a Mac.** Tick **Use ⌘Space (replace Spotlight)** in Settings → Hotkey & Shortcuts. After you approve it, Kage turns off Spotlight's shortcut only while it's running and hands it back when it quits (or on the next launch, after a crash). It also spots clashes with other built-in macOS shortcuts and asks before it takes any over. It never turns back on a shortcut you had switched off yourself.
- **The cat.** Kage breathes, blinks and has idle moods. It reacts when you open the launcher, type, wait for a reply or hit an error, and it wears a party hat after an update. Extensions can join in: it bops along to music, wears a headset in meetings and holds an hourglass while a timer runs. Turn off **Animate Kage** or **Let extensions animate Kage** in Settings → Appearance. Your system's reduced-motion setting is always respected.
- **Backup and restore.** Settings → About Kage → Backup & restore exports your config, saved prompts and commands, custom and learned instructions, and extension data as a `.kage` file, optionally protected with a passphrase (`.kage.enc`). Import it on another machine.

## Languages

Kage speaks 32 languages. It follows your OS language, or the one you pick in Settings → Appearance → Language, and switches without a restart. Arabic, Hebrew, Persian and Urdu get a full right-to-left layout.

<p dir="ltr">English · العربية (Arabic) · বাংলা (Bengali) · 简体中文 (Chinese, Simplified) · 繁體中文 (Chinese, Traditional) · Čeština · Dansk · Nederlands · Suomi · Français · Deutsch · Ελληνικά · עברית (Hebrew) · हिन्दी (Hindi) · Magyar · Bahasa Indonesia · Italiano · 日本語 · 한국어 · Norsk · فارسی (Persian) · Polski · Português · Română · Русский · Español · Svenska · ไทย · Türkçe · Українська · اردو (Urdu) · Tiếng Việt</p>

English is written by hand. The other 31 are machine-translated, and Kage tells you so when you pick one. A few of the newest settings are still in English only. Spotted a clumsy translation? [Open an issue](https://github.com/nachmore/Kage/issues/new/choose).

## Updates

Settings → Updates has two switches:

- **Automatically Check for Updates** checks once a day.
- **Silent Updates** installs a found update quietly once the launcher has been idle for 5 minutes. With it off, an **Install now** banner appears in the launcher instead.

The setup wizard's automatic-updates box turns on both; leave it unticked and both stay off, so Kage only checks when you ask (`>version`, or Settings → Updates). Every download is checked against a signing key built into the app before it's installed. Afterwards you get a changelog, and Kage takes you back to the session you were in.

**Channels:** **Stable** and **Preview** (coming later) and **Nightly**. Only Nightly builds are published today, so stay on Nightly for now. An install from a Nightly build follows Nightly automatically.

## Everything else

- Kage sits in the system tray or menu bar (Show, New Chat Window, Settings, Quit), runs as a single instance and can start at login.
- Banners tell you when an update is ready or has just been installed. If Kage crashed last time, a banner links to the log. An offline bar appears when there's no network.
- **Settings → Internal Sites** can fetch link previews of pages behind your own sign-in, using a local script you provide. The AI can draft the rules and the script, and a Test button tries them. See [Internal Sites](INTERNAL_SITES.md).
- Settings → About Kage has a log viewer, logging options and a button that opens Kage's config folder.
- Something not working? Start Kage with `/debug` to log its conversation with the agent; see the [Debug Mode Guide](DEBUG_MODE.md).
