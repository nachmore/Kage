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

**Settings → Agents → "Use my shell's environment for tools Kage and the
agent run"**. The setting covers everything Kage starts that comes from
your own installs: the agent and its MCP servers, shortcut commands,
speech (the Python behind pocket-tts), the icalBuddy calendar fallback,
and internal-site proxy scripts. When it's off, none of your startup files
run; those programs get the environment Kage was started with plus the
usual user tool folders (`~/.toolbox/bin`, `~/.local/bin`, Homebrew's
`bin`, …) — so anything your shell setup adds beyond those (an `nvm` or
`pyenv` shim, a custom `PATH` entry, `JAVA_HOME`) won't be found when
Kage is opened from the Dock or Finder. The change applies to tools right
away and to the agent the next time it starts (switching connections, a
reconnect, or the next launch) — no restart required. It's a per-machine
setting: restoring a backup doesn't change it.

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
- Kage re-reads your environment in the background now and then: when the
  agent restarts and the last read is more than 30 minutes old, and after
  a failed read (on the next tool or agent start at least a minute later,
  backing off up to every 30 minutes while it keeps failing). A re-read
  takes effect from the *following* start, so a newly installed tool shows
  up after the next agent restart once a refresh has happened — or right
  away if you restart Kage.
