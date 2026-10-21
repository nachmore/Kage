# Internal Sites

Link previews fetch pages directly, without credentials. Pages behind the
user's own sign-in (SSO, intranet proxies, anything that authenticates with
credentials only the user's machine holds) can't be previewed that way.
**Settings → Internal Sites** lets the user define *profiles* that recognise
such sites and fetch them with a local script instead.

Kage itself knows nothing about any particular site or auth scheme: profiles
live only in the user's `config.json`, and everything below is generic.

## Profiles

Each profile has:

- **URL rules** — `domain_suffix` (host equals the pattern or is a subdomain
  of it) or `regex` (matched anywhere in the full URL, case-insensitive).
  Checked before anything leaves the machine; a match skips the direct fetch.
- **Content rules** — `contains` or `regex`, against the direct fetch's
  `body` (head window) or `final_url` (after redirects). Evaluated for every
  response, including errors and sign-in redirects, so internal hosts the URL
  rules miss are still caught.
- **A proxy script** — PowerShell or Bash, with a timeout (default 15 s,
  max 60 s).

The first enabled profile with a non-empty script whose rules match wins.

## Proxy script contract

| Input | |
|---|---|
| `KAGE_URL` | The URL to fetch. Passed in the environment, never spliced into the script text, so a crafted link can't inject code. |
| `KAGE_RESPONSE` | `text` for pages, `base64` for favicons. |

Output on stdout:

- `text`: the page HTML (Kage extracts title / description / favicon with the
  same parser as the direct path), or a JSON object
  `{"title": "...", "description": "...", "favicon": "..."}`.
- `base64`: the raw response bytes, base64-encoded. Kage sniffs the bytes and
  only accepts small raster images (PNG, ICO, GIF, JPEG, WebP, ≤ 64 KB),
  inlined as a `data:` URI — the webview can't authenticate to the host.

Exit 0 on success, non-zero on failure; diagnostics go to stderr. At most two
proxy runs execute at once. Hero images (`og:image`) are dropped for proxied
pages.

## Drafting with the agent

The editor's **Draft rules with AI** and **Draft script with AI** buttons open
a dialog that takes optional guidance for the agent, then runs a one-shot
prompt on a throwaway agent session (`ephemeral_session::run`), hidden from
the session list and deleted afterwards. The backend announces the session id
to the settings window (`internal_sites_draft_session`), which follows its
streamed text and tool calls live, answers its permission prompts (routed to
`settings` via `pending_prompt_originators`; chat windows ignore them), and
can cancel it (`dismiss_pending_permission` + `cancel_generation`). Rule drafting includes what
an unauthenticated direct fetch of the sample URL returned, so the agent can
write content rules for the sign-in page. Script drafting asks the agent to
reuse whatever mechanism its own tools use to read the site. **Test** runs
detection and the script against the sample URL and shows the resulting card.

## Privacy and safety

- The `internal_sites_*` commands are settings-only (`null` in
  `extension-permissions.js`); extensions can't run scripts or read profiles.
- Proxy logging records the profile name and exit status only — never URLs,
  stdout or stderr.
- Saving changed profiles clears the link-metadata cache so stale results
  (e.g. a sign-in page cached as a preview) don't linger.

Implementation: `src/internal_sites.rs` (rules, proxy runner, output parsing),
`src/commands/input/link_metadata.rs` (routing), `src/commands/input/internal_sites.rs`
(settings commands), `ui/js/settings/internal-sites.js` (UI).
