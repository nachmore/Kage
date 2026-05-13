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
- You can disable analytics, reset your install ID, or request deletion from Settings → Privacy.

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

The complete list of features we track is visible in [`ui/js/shared/telemetry.js`](../ui/js/shared/telemetry.js) if you'd like to review it yourself.

### Crash signal
- If Kage panics (an unrecoverable internal error), we send a single `panic` event so we know a buggy build is in the wild.
- The event carries the panic message (truncated to ~250 characters) and the source location it occurred at — e.g. `src/foo.rs:42`. These are paths *inside our own source code*, not paths to your files.
- A full crash report with the backtrace and recent app log is written to a `crash.log` file on your machine. **This local file never leaves your machine** unless you choose to attach it to a bug report yourself.
- Backtraces, local variables, and the app log are intentionally not included in the `panic` event.

## What is never collected

- The content of any message, prompt, or AI response
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
