#!/usr/bin/env python3
"""Seed and update non-English translation catalogs via the Claude CLI.

Drives `claude -p` (non-interactive print mode) with the developer's existing
authentication — no API key plumbing, no separate billing surface. The CLI
must be on PATH; install via the standard Claude Code installer if missing.

For each target language:
  - Load `locales/<lang>/messages.json` if it exists; otherwise create with `_meta`.
  - Diff against `locales/en/messages.json`. For every key whose source text
    has changed (tracked by a per-entry `_source_hash`) or whose translation
    is missing, batch-translate via `claude -p` with `--json-schema` to
    enforce strict output shape.
  - Mark machine-translated entries with the catalog-level
    `machine_translated: true` and per-entry `_machine_translated: true`
    flags. A translator who hand-edits an entry should remove the per-entry
    flag — `scripts/translate.py` then leaves it alone on subsequent runs.

The script also walks `Kage-Extensions/extensions/<id>/_locales/` and updates
each extension's catalogs the same way. Extensions opt in by shipping
`_locales/en/messages.json`; everything else is automatic.

# Why the CLI rather than the Anthropic API directly?

So that whatever auth is already wired into the developer's environment
(OAuth, keychain, ANTHROPIC_API_KEY, etc.) is reused. The CLI handles auth;
we just hand it a prompt and a JSON schema and read the result.

# Usage

  python scripts/translate.py                       # all languages, all catalogs
  python scripts/translate.py --langs ja,ar,de      # subset of languages
  python scripts/translate.py --catalog host        # only the host catalog
  python scripts/translate.py --catalog extensions  # only extensions
  python scripts/translate.py --workers 8           # raise concurrency from default 4
  python scripts/translate.py --workers 1           # serial (debugging / rate-limit avoidance)
  python scripts/translate.py --dry-run             # show pending work, no Claude calls

# Concurrency

The script runs each (catalog, language) pair in its own worker thread. Because
every pair owns a distinct `messages.json` file and the prompts are fully
self-contained (no shared session, no cross-call context — `--bare` plus
`--no-session-persistence` make every Claude invocation stateless), there's no
risk of translations leaking between pairs. The only practical limit on
parallelism is the developer's Anthropic rate limit; the default `--workers 4`
keeps well under typical tier ceilings while still cutting wall time roughly
4×. A global print lock keeps progress output from interleaving.

# Idempotency

Safe to run repeatedly. Each entry stores a `_source_hash` of the EN message
+ description; on re-run, only entries whose hash changed (or are missing
entirely) are re-translated. Hand-edited entries (where a human translator
removed the `_machine_translated: true` flag) are never overwritten.
"""

from __future__ import annotations
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

# Force UTF-8 on stdout/stderr so non-ASCII language names print on Windows
# consoles configured for cp1252 without raising UnicodeEncodeError.
try:
    sys.stdout.reconfigure(encoding="utf-8")  # type: ignore[attr-defined]
    sys.stderr.reconfigure(encoding="utf-8")  # type: ignore[attr-defined]
except (AttributeError, OSError):
    pass

ROOT = Path(__file__).resolve().parent.parent
LOCALES = ROOT / "locales"
EN_PATH = LOCALES / "en" / "messages.json"
EXTENSIONS_DIR = (ROOT / ".." / "Kage-Extensions" / "extensions").resolve()

# CLDR top 30 by speaker count, with the four RTL languages explicit.
# Display names are the language's autonym so they look right inside their
# own dropdown entry.
DEFAULT_LANGS: dict[str, dict] = {
    "ar":     {"name": "العربية",            "rtl": True},
    "bn":     {"name": "বাংলা",                "rtl": False},
    "cs":     {"name": "Čeština",            "rtl": False},
    "da":     {"name": "Dansk",              "rtl": False},
    "de":     {"name": "Deutsch",            "rtl": False},
    "el":     {"name": "Ελληνικά",           "rtl": False},
    "es":     {"name": "Español",            "rtl": False},
    "fa":     {"name": "فارسی",              "rtl": True},
    "fi":     {"name": "Suomi",              "rtl": False},
    "fr":     {"name": "Français",           "rtl": False},
    "he":     {"name": "עברית",              "rtl": True},
    "hi":     {"name": "हिन्दी",                "rtl": False},
    "hu":     {"name": "Magyar",             "rtl": False},
    "id":     {"name": "Bahasa Indonesia",   "rtl": False},
    "it":     {"name": "Italiano",           "rtl": False},
    "ja":     {"name": "日本語",               "rtl": False},
    "ko":     {"name": "한국어",               "rtl": False},
    "nl":     {"name": "Nederlands",         "rtl": False},
    "no":     {"name": "Norsk",              "rtl": False},
    "pl":     {"name": "Polski",             "rtl": False},
    "pt":     {"name": "Português",          "rtl": False},
    "ro":     {"name": "Română",             "rtl": False},
    "ru":     {"name": "Русский",            "rtl": False},
    "sv":     {"name": "Svenska",            "rtl": False},
    "th":     {"name": "ไทย",                "rtl": False},
    "tr":     {"name": "Türkçe",             "rtl": False},
    "uk":     {"name": "Українська",         "rtl": False},
    "ur":     {"name": "اردو",               "rtl": True},
    "vi":     {"name": "Tiếng Việt",         "rtl": False},
    "zh-CN":  {"name": "简体中文",            "rtl": False},
    "zh-TW":  {"name": "繁體中文",            "rtl": False},
}

# Batch size for one Claude call. Larger batches spread the per-call overhead
# (model spin-up, system prompt) across more strings — but each call has a
# response budget, so very large batches risk truncation. 25 keys × ~80
# chars each is well within reasonable response sizes.
BATCH_SIZE = 25

# Where to find the CLI. Override via $CLAUDE_BIN if installed somewhere
# unusual. On Windows the `claude` on $PATH often resolves to a Toolbox
# shim that only dispatches in interactive mode and refuses subprocess
# invocations with "Command doesn't appear to be associated with any tool".
# So when $PATH points at a Toolbox shim we walk Toolbox's tools directory
# and pick the highest-versioned real binary instead.
def _resolve_claude_bin() -> str | None:
    override = os.environ.get("CLAUDE_BIN")
    if override:
        return override
    found = shutil.which("claude")
    if not found:
        return None
    # Detect a Toolbox-style shim wrapper. Some installer setups put a
    # shim binary on $PATH at `<root>/Toolbox/bin/claude.exe` that only
    # dispatches in interactive mode; the real per-version binaries live
    # under `<root>/Toolbox/tools/claude-code/<ver>/claude.exe`. When we
    # detect the shim, walk to the highest-versioned real binary.
    norm = found.replace("\\", "/").lower()
    if "/toolbox/bin/" not in norm:
        return found
    toolbox_root = Path(found).resolve().parent.parent
    candidates = sorted(
        (toolbox_root / "tools" / "claude-code").glob("*/claude.exe"),
        key=lambda p: p.parent.name,
        reverse=True,
    )
    return str(candidates[0]) if candidates else found


CLAUDE_BIN = _resolve_claude_bin()

# JSON schema enforced on every batch response. The CLI's --json-schema
# validates this before returning, so a malformed response is the CLI's
# problem, not ours.
RESPONSE_SCHEMA = {
    "type": "object",
    "properties": {
        "translations": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "key": {"type": "string"},
                    "message": {"type": "string"},
                },
                "required": ["key", "message"],
                "additionalProperties": False,
            },
        }
    },
    "required": ["translations"],
    "additionalProperties": False,
}


def hash_source(message: str, description: str) -> str:
    """Stable hash of the EN entry. Used to detect when a source string has
    changed and the translation needs regeneration."""
    h = hashlib.sha256()
    h.update(message.encode("utf-8"))
    h.update(b"\0")
    h.update(description.encode("utf-8"))
    return h.hexdigest()[:16]


def load_or_init(path: Path, lang: str, meta: dict) -> dict:
    if path.exists():
        return json.loads(path.read_text(encoding="utf-8"))
    return {
        "_meta": {
            "language": lang,
            "name": meta["name"],
            "rtl": meta["rtl"],
            "machine_translated": True,
        },
    }


def save(path: Path, catalog: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(catalog, ensure_ascii=False, indent=2, sort_keys=False)
    path.write_text(text + "\n", encoding="utf-8")


def build_prompt(lang_code: str, lang_name: str, batch: list[tuple[str, dict]]) -> str:
    """Build the prompt asking Claude to translate `batch`.

    The schema we hand to `--json-schema` constrains the response shape;
    here we focus on the substantive translation rules. The model receives
    the source key (for context — sometimes the key path is the only hint
    about whether a string is a button label vs. an error message vs.
    description text), the EN message template, and the translator notes.
    """
    items = []
    for key, en_entry in batch:
        items.append({
            "key": key,
            "source": en_entry["message"],
            "description": en_entry.get("description", ""),
        })
    return (
        f"You are translating UI strings for Kage, a desktop AI assistant. "
        f"Translate the items below from English into {lang_name} (BCP-47: {lang_code}).\n\n"
        "RULES (every rule is load-bearing — a violation breaks the runtime):\n"
        "  1. Preserve every {name} placeholder exactly. Do NOT translate the\n"
        "     placeholder name; only the surrounding text.\n"
        "  2. Preserve ICU plural / select syntax verbatim — `{count, plural, "
        "one {...} other {...}}` keeps that exact form, but you translate the\n"
        "     inside of each arm. Use the correct CLDR plural categories for the\n"
        "     target language: Russian needs one/few/many/other; Polish has\n"
        "     one/few/many; Arabic has zero/one/two/few/many/other; Welsh has\n"
        "     zero/one/two/few/many/other. If you change which categories are\n"
        "     present, the runtime will fall through to `other` for missing ones.\n"
        "  3. The `#` character inside a plural arm is the count placeholder.\n"
        "     Keep it as `#`.\n"
        "  4. Match the source's tone: short, neutral, modern. Match the source's\n"
        "     capitalisation style (sentence case for our UI; we don't title-case).\n"
        "  5. Technical terms with no idiomatic translation stay in English: OAuth,\n"
        "     MCP, ACP, WebView2, JSON, URL, API, CLI, RPC, ID, UUID. So do brand\n"
        "     names: Kage, Claude, Anthropic, Tauri, GitHub.\n"
        "  6. Honorifics & formality: use the standard register the platform's OS\n"
        "     uses — polite-formal Japanese (です/ます), polite Korean (합쇼체),\n"
        "     German formal Sie. NOT casual / familiar.\n"
        "  7. For RTL languages (Arabic, Hebrew, Persian, Urdu): write the\n"
        "     translation as natural RTL text. Don't insert directional control\n"
        "     characters; the runtime sets <html dir=\"rtl\"> for layout.\n"
        "  8. Punctuation: use the target language's idiomatic punctuation\n"
        "     (Chinese full-width 。，；; Japanese 。、; French &nbsp; before :)\n"
        "     where appropriate. Trailing periods on labels: keep only if the EN\n"
        "     source has one.\n\n"
        "Respond with strict JSON matching the schema you've been given. One\n"
        "translations entry per source item, in the same order. No prose, no\n"
        "markdown.\n\n"
        f"Source items ({len(items)}):\n"
        f"{json.dumps(items, ensure_ascii=False, indent=2)}"
    )


def call_claude(prompt: str, max_retries: int = 2) -> dict | None:
    """Run `claude -p ... --output-format=json --json-schema=...` and return
    the parsed `translations` payload. Returns None on hard failure.

    The CLI prints a JSON envelope with a `result` field; we also pass
    --json-schema so the result itself is validated against our shape.
    """
    if not CLAUDE_BIN:
        print("FATAL: claude CLI not found on PATH. Install Claude Code or set $CLAUDE_BIN.",
              file=sys.stderr)
        return None

    cmd = [
        CLAUDE_BIN,
        "-p",
        # `--bare` disables hooks / MCP / auto-memory / CLAUDE.md discovery so
        # the translation call doesn't accidentally pick up project context.
        # Translation is a pure transformation; hooks would just slow it down.
        "--bare",
        "--output-format=json",
        "--json-schema", json.dumps(RESPONSE_SCHEMA),
        # No tools needed — we're just transforming text.
        "--tools", "",
        # Don't persist sessions. Each batch is independent.
        "--no-session-persistence",
        prompt,
    ]
    last_err = None
    for attempt in range(max_retries + 1):
        try:
            proc = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                encoding="utf-8",
                # Long-form translations of larger batches can take 30-60s; give it room.
                timeout=300,
            )
            if proc.returncode != 0:
                last_err = f"exit code {proc.returncode}: {proc.stderr.strip()[:500]}"
                time.sleep(2 + attempt * 3)
                continue
            envelope = json.loads(proc.stdout)
            # The CLI's --output-format=json wraps the model output in:
            #   { "type": "result", "subtype": "success",
            #     "result": "free-form text",
            #     "structured_output": <object matching --json-schema>, ... }
            # When --json-schema is supplied, the schema-validated payload
            # lands in `structured_output` (the `result` field is empty).
            # When the schema is omitted, the answer is in `result` as a
            # plain string and the caller has to JSON-parse it themselves.
            result = envelope.get("structured_output")
            if result is None:
                raw = envelope.get("result")
                if isinstance(raw, str) and raw.strip():
                    result = json.loads(raw)
            if not isinstance(result, dict) or "translations" not in result:
                last_err = f"unexpected envelope shape: {list(envelope.keys())}"
                time.sleep(2 + attempt * 3)
                continue
            return result
        except subprocess.TimeoutExpired as e:
            last_err = f"timeout after {e.timeout}s"
            time.sleep(2 + attempt * 3)
        except json.JSONDecodeError as e:
            # Truncated output, schema mismatch, etc.
            last_err = f"json decode failed: {e}"
            time.sleep(2 + attempt * 3)
        except Exception as e:  # noqa: BLE001
            last_err = f"{type(e).__name__}: {e}"
            time.sleep(2 + attempt * 3)
    print(f"  ! batch failed after {max_retries + 1} attempts: {last_err}", file=sys.stderr)
    return None


# Lock around stdout/stderr so progress lines from concurrent workers don't
# interleave mid-line. Each worker prints a self-contained block (start +
# result) under the lock so the user sees the units of work cleanly.
_PRINT_LOCK = threading.Lock()


def _log(msg: str, *, err: bool = False) -> None:
    with _PRINT_LOCK:
        print(msg, file=sys.stderr if err else sys.stdout, flush=True)


def _build_task(label: str, en: dict, target_path: Path, lang: str, lang_meta: dict) -> dict | None:
    """Compute what work is needed for one (catalog, language) pair.

    Returns a task dict with the catalog, the pending list, and identifying
    metadata. Returns None if the catalog is up-to-date (already saves on
    disk after stale-key removal). Side effect: writes the catalog file
    when no translation is needed (so the no-op path still refreshes
    `_meta` and prunes stale keys atomically).
    """
    catalog = load_or_init(target_path, lang, lang_meta)
    catalog["_meta"] = {
        "language": lang,
        "name": lang_meta["name"],
        "rtl": lang_meta["rtl"],
        "machine_translated": True,
    }

    pending: list[tuple[str, dict]] = []
    for key, en_entry in en.items():
        if key.startswith("_"):
            continue
        src_hash = hash_source(en_entry["message"], en_entry.get("description", ""))
        existing = catalog.get(key)
        if existing is None:
            pending.append((key, en_entry))
            continue
        if not existing.get("_machine_translated", False):
            continue
        if existing.get("_source_hash") != src_hash:
            pending.append((key, en_entry))

    en_keys = {k for k in en if not k.startswith("_")}
    stale = [k for k in catalog if not k.startswith("_") and k not in en_keys]
    for k in stale:
        del catalog[k]

    if not pending and not stale:
        # Catalog is already current — nothing to do.
        return None

    if not pending:
        # Stale keys removed but nothing to translate. Persist and skip.
        save(target_path, catalog)
        _log(f"  - {label} :: {lang}: pruned {len(stale)} stale, no translation needed")
        return None

    return {
        "label": label,
        "target_path": target_path,
        "lang": lang,
        "lang_meta": lang_meta,
        "catalog": catalog,
        "pending": pending,
        "stale_count": len(stale),
    }


def _run_task(task: dict, dry_run: bool) -> tuple[str, str, int, int]:
    """Execute one (catalog, language) translation task.

    Each task owns its own catalog file — no two tasks ever write to the
    same path — so concurrent workers don't race on disk. We do batch the
    calls within the task sequentially because batches share state (a
    later batch may need to write to the same file).

    Returns (label, lang, ok_count, fail_count) for summary reporting.
    """
    label = task["label"]
    lang = task["lang"]
    target_path = task["target_path"]
    lang_meta = task["lang_meta"]
    catalog = task["catalog"]
    pending = task["pending"]

    _log(f"  + {label} :: {lang} ({lang_meta['name']}) — {len(pending)} keys")

    if dry_run:
        return (label, lang, 0, len(pending))

    ok = 0
    failed = 0
    for i in range(0, len(pending), BATCH_SIZE):
        batch = pending[i : i + BATCH_SIZE]
        prompt = build_prompt(lang, lang_meta["name"], batch)
        response = call_claude(prompt)
        if response is None:
            failed += len(batch)
            continue
        translations = {t["key"]: t["message"] for t in response.get("translations", [])}
        for key, en_entry in batch:
            translated = translations.get(key)
            if translated is None:
                failed += 1
                continue
            src_hash = hash_source(en_entry["message"], en_entry.get("description", ""))
            catalog[key] = {
                "message": translated,
                "description": en_entry.get("description", ""),
                "_machine_translated": True,
                "_source_hash": src_hash,
            }
            ok += 1
        # Write incrementally so a mid-run failure or kill doesn't lose
        # previous batches in this task.
        save(target_path, catalog)

    status = "✓" if failed == 0 else "✗"
    _log(f"  {status} {label} :: {lang} — {ok}/{len(pending)} keys translated")
    return (label, lang, ok, failed)


def collect_tasks(targets: dict[str, dict], catalogs: str) -> list[dict]:
    """Walk the host catalog and (optionally) every extension's catalog,
    returning the list of (catalog, language) tasks that have pending work.
    Catalogs already in sync produce no task entry.
    """
    tasks: list[dict] = []

    if catalogs in ("host", "all"):
        if not EN_PATH.exists():
            _log(f"FATAL: {EN_PATH} missing", err=True)
        else:
            en = json.loads(EN_PATH.read_text(encoding="utf-8"))
            for lang, meta in targets.items():
                target = LOCALES / lang / "messages.json"
                try:
                    t = _build_task("host", en, target, lang, meta)
                    if t is not None:
                        tasks.append(t)
                except Exception as e:  # noqa: BLE001
                    _log(f"  ! host {lang} task-build failed: {e}", err=True)

    if catalogs in ("extensions", "all"):
        # Both Kage-Extensions/<id>/ (user-installed source) and
        # ui/extensions/<id>/ (built-in, ships with the binary) follow the
        # same _locales/<lang>/messages.json convention. Walk both.
        ext_roots = []
        if EXTENSIONS_DIR.exists():
            ext_roots.append(EXTENSIONS_DIR)
        else:
            _log(f"WARN: extensions dir not found at {EXTENSIONS_DIR}, skipping", err=True)
        builtin_dir = ROOT / "ui" / "extensions"
        if builtin_dir.exists():
            ext_roots.append(builtin_dir)

        for root_dir in ext_roots:
            for ext_dir in sorted(p for p in root_dir.iterdir() if p.is_dir()):
                en_path = ext_dir / "_locales" / "en" / "messages.json"
                if not en_path.exists():
                    continue
                ext_en = json.loads(en_path.read_text(encoding="utf-8"))
                for lang, meta in targets.items():
                    target = ext_dir / "_locales" / lang / "messages.json"
                    try:
                        t = _build_task(f"ext:{ext_dir.name}", ext_en, target, lang, meta)
                        if t is not None:
                            tasks.append(t)
                    except Exception as e:  # noqa: BLE001
                        _log(
                            f"  ! ext {ext_dir.name} {lang} task-build failed: {e}",
                            err=True,
                        )

    return tasks


def run_tasks(tasks: list[dict], workers: int, dry_run: bool) -> tuple[int, int]:
    """Run all tasks across a worker pool. Returns (ok_total, failed_total)
    aggregated across every task. Each task writes its own catalog file
    so workers can't race on disk; a global print lock keeps progress
    output legible.
    """
    if not tasks:
        return (0, 0)
    if workers <= 1 or dry_run:
        # Serial path — easier to debug, also the path used by --dry-run.
        ok_total = failed_total = 0
        for t in tasks:
            _, _, ok, failed = _run_task(t, dry_run)
            ok_total += ok
            failed_total += failed
        return (ok_total, failed_total)

    ok_total = failed_total = 0
    with ThreadPoolExecutor(max_workers=workers, thread_name_prefix="i18n") as pool:
        futures = [pool.submit(_run_task, t, dry_run) for t in tasks]
        for fut in as_completed(futures):
            try:
                _, _, ok, failed = fut.result()
            except Exception as e:  # noqa: BLE001
                _log(f"  ! worker raised: {e}", err=True)
                failed_total += 1
                continue
            ok_total += ok
            failed_total += failed
    return (ok_total, failed_total)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--langs", help="Comma-separated language codes (default: all)")
    parser.add_argument(
        "--catalog",
        choices=("host", "extensions", "all"),
        default="all",
        help="Which catalogs to update",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=4,
        help=(
            "Number of concurrent Claude calls (default: 4). Each (catalog, language) "
            "pair is independent — different files, different prompts — so concurrency "
            "is safe; tasks never share in-flight state. Bump this if your "
            "Anthropic-tier rate limit allows; drop to 1 for serial execution / "
            "easier debugging."
        ),
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="List pending work without calling Claude"
    )
    args = parser.parse_args()

    targets: dict[str, dict]
    if args.langs:
        wanted = [s.strip() for s in args.langs.split(",") if s.strip()]
        targets = {k: DEFAULT_LANGS[k] for k in wanted if k in DEFAULT_LANGS}
        unknown = [k for k in wanted if k not in DEFAULT_LANGS]
        if unknown:
            print(f"WARN: unknown language(s) ignored: {unknown}", file=sys.stderr)
    else:
        targets = DEFAULT_LANGS

    if not args.dry_run and not CLAUDE_BIN:
        print(
            "FATAL: claude CLI not found on PATH. Install Claude Code or set $CLAUDE_BIN.\n"
            "       Pass --dry-run to see pending work without making API calls.",
            file=sys.stderr,
        )
        return 2

    print(f"Collecting tasks (catalog={args.catalog}, langs={len(targets)})...")
    tasks = collect_tasks(targets, args.catalog)
    if not tasks:
        print("\nAll catalogs already up to date — nothing to translate.")
        return 0
    print(f"  {len(tasks)} (catalog, language) pair(s) need translation")
    print(f"  workers={args.workers}, batch_size={BATCH_SIZE}")
    if args.dry_run:
        print("  (dry run; not calling Claude)")
    start = time.time()
    ok, failed = run_tasks(tasks, args.workers, args.dry_run)
    elapsed = time.time() - start
    print(f"\nDone in {elapsed:.0f}s. Translated {ok} keys; {failed} failed.")
    print("Run `python scripts/check_i18n.py` to verify the build is clean.")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
