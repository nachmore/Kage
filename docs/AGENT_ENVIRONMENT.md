# Agent tools and your shell environment (macOS / Linux)

When you open Kage from Finder, the Dock, Spotlight, or a Linux desktop
launcher — and every time it relaunches after an update — it does not get
the environment your terminal has. On macOS it starts with only
`PATH=/usr/bin:/bin:/usr/sbin:/sbin`, and nothing from `.zshrc`,
`.bash_profile`, or similar files. The agent Kage starts, and the MCP
servers the agent starts from your `mcp.json`, would inherit that, so a
server like `"command": "builder-mcp"` (installed in `~/.toolbox/bin`) or
anything run through `#!/usr/bin/env node` would fail with "command not
found" even though it works in a terminal.

To avoid that, Kage runs your login shell's startup files **once, in the
background, shortly after launch**, and gives the agent (and the tools it
starts) the resulting `PATH` and the variables your startup files export —
`JAVA_HOME`, version-manager settings, proxy and certificate settings,
`AWS_PROFILE`, `LANG`, and so on. Your terminal settings are only *added*:
nothing Kage already has is replaced, and session state such as `HOME`,
terminal settings, or a freshly started `ssh-agent` is never copied.
Temporary AWS credentials (anything with a session token) are not copied
either, so AWS tools fall back to your profile or SSO login, which can
refresh itself.

Windows doesn't need any of this; the setting isn't shown there.

## Turning it off

**Settings → Agents → "Use my shell's environment for agent tools"**. When
it's off, the agent and its tools get exactly the environment Kage was
started with. The change applies the next time the agent starts (switching
connections, a reconnect, or the next launch) — no restart required.

You can also force it from the environment Kage is started with:
`KAGE_LOGIN_SHELL_ENV=0` turns it off, `KAGE_LOGIN_SHELL_ENV=1` turns it on
regardless of the setting. If you start Kage from a terminal it already
has your environment, and nothing is run.

## Keeping your startup files friendly to it

Because Kage runs your startup files in the background, anything in them
that does more than set variables runs too: refreshing credentials,
opening a browser for a sign-in, network calls, starting `tmux`, printing
prompts. Kage sets `KAGE_RESOLVING_ENVIRONMENT=1` while it does this, so
you can skip those parts:

```sh
# ~/.zshrc or ~/.bash_profile
if [ -z "$KAGE_RESOLVING_ENVIRONMENT" ]; then
    refresh-my-credentials        # anything that signs in, opens a browser, …
    [ -z "$TMUX" ] && exec tmux
fi
```

```fish
# ~/.config/fish/config.fish
if not set -q KAGE_RESOLVING_ENVIRONMENT
    # interactive-only work here
end
```

Things to know:

- Kage gives up after 10 seconds and stops everything the startup files
  started. If it takes more than 3 seconds, or has to be stopped, Kage's
  log (Settings → About) says so, naming this setting and the variable
  above.
- The startup files run without a terminal and with no input, so anything
  that asks a question gets an immediate "end of input".
- Kage re-reads your environment in the background now and then (when the
  agent restarts and the last read is more than 30 minutes old, or a
  minute after a failed read), so newly installed tools are picked up
  without restarting Kage.
