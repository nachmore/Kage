# Privacy Policy

**Last updated:** 2026-05-11
**Policy version:** 1

Kage is a local-first desktop assistant. This document covers the one
area where the app may transmit anything off your machine: anonymous
product analytics, used to understand feature usage and prioritise
bug fixes.

Everything described below is **opt-out**. On first run, Kage shows a
dedicated page explaining this and lets you disable analytics before
any event is ever sent. You can change your mind at any time from
**Settings → Privacy**.

## TL;DR

- Kage collects anonymous usage events through [Aptabase](https://aptabase.com), a privacy-first analytics service.
- The events are tied to a random install ID generated on your machine. It is not linked to you, your account, or your device.
- We **never** collect message contents, file paths, clipboard data, names, emails, or IP addresses.
- Data is stored in the EU.
- You can disable analytics or reset your install ID in Settings → Privacy. To have your data deleted, [open a GitHub issue](#your-rights) with the install ID shown there.

## What is collected

When analytics are enabled, each event includes:

### Identifiers
- **Install ID** — a random UUID generated on your machine the first time you consent. Not linked to your name, email, or device. You can regenerate it at any time, which orphans all prior events.

### Device attributes
- App version (e.g. `0.9.0`)
- Operating system and version (e.g. `Windows 11`, `macOS 14.5`)
- System language / locale (e.g. `en-US`)
- Country, derived from your IP address by Aptabase during ingestion. **The IP itself is not stored.**

### Events
- A record of which high-level features you use and how often (e.g. opening the chat window, running a shortcut, installing an extension). The recorded name identifies the feature — it does **not** include anything you typed, selected, or opened.
- Coarse, non-identifying properties attached to some events. For example, a "message sent" event carries the source window (floating vs chat) and a size bucket (small / medium / large) — **never the message content**.
- When an extension opens a window of its own (an extension view), the event records only where it was opened from (the launcher, chat, a card, a widget, a toolbar button, Settings or another such window) and whether the window was already open. It never says which extension, what the window shows, or what it was opened with.

The complete list of features we track is visible in [`ui/js/shared/telemetry.js`](../ui/js/shared/telemetry.js) if you'd like to review it yourself.

### Sync
- Sync data never touches Kage servers. Kage syncs through a folder you already sync (Dropbox, iCloud Drive, OneDrive, …), and everything it puts there is encrypted on your computer.
- Kage only counts that sync is used, how many computers, and which kinds of problems happen. These counts are in coarse buckets (for example "3–5 computers").
- Sync events never include the folder, which sync app you use, computer names, setting names or values, extension ids, or anything that was synced.

### Files you choose for an extension
- An extension with the "Files you choose" permission can ask you to pick a file. Kage reads the file you pick on your computer and gives the extension its text. Kage never tells the extension where the file is, and it reads nothing else.
- When an extension has read a file you picked, the usage event records only the extension's id, the file's format (for example PDF), whether the read worked or why not, and the file's size, the text's length and the time it took in coarse buckets (for example "under 1 MB"). Never the file's name, its folder, its title or any of its text.

What Kage keeps on your computer:
- Nothing from the file itself. What the extension saves is its own data: on your computer, in the full backups you make (which are encrypted), and in your sync folder (encrypted) only if you use sync and the extension syncs its data.
- The permission audit log (Settings → Tool permissions) records each read with the file's name, so you can see what your extensions read. It stays on your computer, is never sent or backed up, and you can clear it.
- A random key, used to tell an extension "this is the same file as before" without telling it where the file is. It stays on this computer and is never sent, backed up or synced.

### Background AI for extensions
- Some extensions can ask your AI agent for help in the background, with no chat. This is off for every extension until you turn it on for that extension and confirm which agent gets the text.
- The text goes to your agent, the same one your chats use. It never goes to Kage or to Aptabase. Where it goes from there depends on your agent: the dialog says whether it's an online service, this computer or another computer.
- Kage counts three things: when you turn background AI on or off for an extension (`extension_agent_access_changed`: the extension id and on or off); once a day, how many requests each extension made and how they ended, with the characters sent and received in a coarse bucket (`extension_agent_calls_daily`: the extension id, counts of ok, failed and cancelled requests and tool attempts, and a bucket such as "under 50,000"); and when Kage pauses an extension's requests (`extension_agent_calls_paused`: the extension id and whether it was after failures or a tool attempt).
- These events never include the text sent or received, the extension's purposes or labels, or the agent's name.
- On your computer, Kage keeps a log entry for each request in Settings → Tool permissions (sizes, the agent's name and the outcome, never the text), and the last five requests and replies in memory only, for **Show what was sent**.

### Crash signal
- If Kage panics (an unrecoverable internal error), we send a single `panic` event so we know a buggy build is in the wild.
- The event carries only a coarse category from a fixed list (e.g. `unwrap_none`, `index_oob`, `other`) and the source location it occurred at — e.g. `src/foo.rs:42`. These are paths *inside our own source code*, not paths to your files.
- The panic message itself is never sent: it can quote whatever data was being processed when the error hit, so it stays on your machine.
- A full crash report with the panic message, backtrace, and recent app log is written to a `crash.log` file on your machine. **This local file never leaves your machine** unless you choose to attach it to a bug report yourself.
- The panic message, backtraces, local variables, and the app log are intentionally not included in the `panic` event.

## What is never collected

- The content of any message, prompt, or AI response
- Any command you run from Kage, or its output (a run is counted with a coarse exit-code bucket only)
- File names, file paths, folder contents
- Clipboard data
- Search queries typed into Kage
- Your name, email address, username, or profile data
- Your IP address (stripped at ingest by Aptabase, never stored)
- Browsing history, screenshots, or keystrokes
- Anything from any other application on your machine

## Where your data goes

- Events are sent to Aptabase over HTTPS.
- Aptabase stores events on servers located in the **European Union**.
- No events are forwarded to any other third party. Aptabase is the only processor.
- Their own privacy policy is at <https://aptabase.com/legal/privacy>.
- Separately from analytics, Kage checks whether you're online by sending
  empty requests (no data, no identifiers) to well-known connectivity-check
  addresses: `www.msftconnecttest.com`, `www.cloudflare.com` and
  `www.google.com`. This happens at startup, when you summon Kage, and while
  it believes you're offline.

## Retention

Events are retained for up to **24 months**, after which they are automatically deleted from Aptabase.

## Your rights

At any time, you can:

- **Disable analytics** — *Settings → Privacy → Send anonymous usage data*. No further events are sent the moment you flip the toggle. One `telemetry_disabled` event fires just before to help us measure opt-out rates.
- **Reset your install ID** — *Settings → Privacy → Reset*. This generates a new random UUID and orphans all prior events from this install.
- **Request deletion** — file a GitHub issue at <https://github.com/nachmore/Kage/issues> with your install ID (visible in Settings → Privacy) and we will delete all events associated with it. Do NOT include anything else identifying about yourself; the install ID is the only thing we need.

## Legal basis (EU/UK users)

We process this data under the *legitimate interest* lawful basis of GDPR Article 6(1)(f): understanding product usage to improve the app, measured against minimal privacy impact given the anonymous, aggregated nature of the data and the explicit opt-out on first run.

## Builds without analytics

Local development builds and forks that compile without the `APTABASE_KEY` environment variable have **no analytics transport compiled in**. The toggle in Settings → Privacy still appears but has no effect — no events can be sent because the client isn't present in the binary.

## Changes to this policy

If the data we collect or the processors we use materially change, we bump the policy version. Kage will re-prompt you with the updated disclosure the next time you launch the app, and analytics stay disabled until you explicitly consent again.

## Contact

For questions or deletion requests, open an issue: <https://github.com/nachmore/Kage/issues>.
