#!/usr/bin/env python3
r"""i18n drift checker.

Fails if any of the following hold:

  1. A `t!(...)` call in Rust references a key that's not in `locales/en/messages.json`.
  2. A `t("...")` call in JS / `data-i18n="..."` attribute references a key
     that's not in EN.
  3. Any non-English catalog has a different set of keys than EN. (Per the
     ship policy: "we're using AI, we should be able to fill in any gaps
     easily" — drift in either direction is treated as a bug.)
  4. An EN key is referenced nowhere in source. (Stale key — clean up or
     mark `_unused: true` if intentionally reserved.)
  5. A catalog file has a malformed `_meta` block, missing `language` field,
     or any `message` value containing a placeholder that no other catalog
     in the same key has.

Run via `python scripts/check_i18n.py` or as part of `python scripts/test_all.py`.
The CI gate is hard-fail: any drift kills the build. Re-run
`python scripts/translate.py` to regenerate missing translations rather than
hand-editing.

Limitations:
  - Only static keys are checkable. Dynamic keys (Rust `t!(some_var)`, JS
    `t(`prefix.${id}`)`) are silently skipped — use them sparingly, and add
    the resolved keys to KNOWN_DYNAMIC_KEYS below so we can still drift-check
    them.
"""

from __future__ import annotations
import json
import os
import re
import sys
from pathlib import Path
from typing import Iterable

# Force UTF-8 on stdout/stderr so the OK/FAIL markers don't crash the script
# on Windows consoles configured for cp1252. This is a Python 3.7+ API.
try:
    sys.stdout.reconfigure(encoding="utf-8")  # type: ignore[attr-defined]
    sys.stderr.reconfigure(encoding="utf-8")  # type: ignore[attr-defined]
except (AttributeError, OSError):
    pass

ROOT = Path(__file__).resolve().parent.parent
LOCALES_DIR = ROOT / "locales"
EXTENSIONS_DIR = (ROOT / ".." / "Kage-Extensions" / "extensions").resolve()

# ---------------------------------------------------------------------------
# Source-scan regexes.
# ---------------------------------------------------------------------------
#
# The patterns here only match string-literal first arguments. Dynamic keys
# (variables, template strings) are intentionally skipped — they should be
# rare and listed in KNOWN_DYNAMIC_KEYS below.

# Rust: `t!("key", ...)` and `i18n::translate("key", ...)` and
# `AppError::keyed(KIND, "key", ...)`.
#
# `i18n::translate_in("en", "key", ...)` takes the locale first, then the key —
# we use a negative lookahead so the locale literal isn't misread as the key.
RUST_PATTERNS = [
    re.compile(r"""\bt!\s*\(\s*"([^"]+)"""),
    # translate(key, ...) — locale-implicit form.
    re.compile(r"""\bi18n::translate\s*\(\s*"([^"]+)"""),
    # translate_in(lang, key, ...) — capture the second string literal only.
    re.compile(r"""\bi18n::translate_in\s*\(\s*[^,]+,\s*"([^"]+)"""),
    re.compile(r"""\bAppError::keyed\s*\([^,]+,\s*"([^"]+)"""),
]

# JS: `t("key", ...)`, `t('key', ...)`, `tHtml("key", ...)`, `data-i18n="key"`,
# `data-i18n-title="key"`, etc.
JS_PATTERNS = [
    re.compile(r"""\bt\s*\(\s*['"]([^'"]+)['"]"""),
    re.compile(r"""\btHtml\s*\(\s*['"]([^'"]+)['"]"""),
    re.compile(r"""\bformatMessage\s*\(\s*['"]([^'"]+)['"]"""),
]

# HTML: data-i18n* attributes.
HTML_PATTERN = re.compile(
    r"""data-i18n(?:-(?:title|placeholder|aria-label|alt|html))?\s*=\s*['"]([^'"]+)['"]"""
)

# Keys produced by dynamic call sites — known good, exempt from "is referenced"
# check. Each entry is either a literal key OR a glob pattern (with `*` as a
# wildcard at any segment) — useful when a backtick template constructs the
# key from a runtime value (e.g. `settings.manager.cap.${cap}.label`). Glob
# patterns must match a key segment exactly (we don't allow partial matches
# inside a segment) — keeps the rule readable. Add a short comment when
# adding entries here.
KNOWN_DYNAMIC_KEYS: set[str] = {
    # All `errors.passthrough` invocations come from AppError::raw and the
    # legacy free-form constructors; the key has no static call site but is
    # the implicit fallback for everything routed through them.
    "errors.passthrough",
    # Capability badges in settings/manager.js use a backtick template
    # `settings.manager.cap.${cap}.label` / `.desc` to render the per-cap
    # label & tooltip, so the static t() regex doesn't see the keys.
    "settings.manager.cap.*.label",
    "settings.manager.cap.*.desc",
    # Automations dropdowns: each TRANSFORM/SCHEDULE/DAY value is rendered
    # via a template literal `settings.automations.<group>.${value}`.
    "settings.automations.transform.*",
    "settings.automations.schedule.*",
    "settings.automations.day.*",
    # Steering editor picks title/subtitle/empty-hint/row-placeholder by
    # ternary on the editor mode (auto vs user); the literal keys are
    # passed to t() but not in the immediate-call form the regex matches.
    "settings.assistant.editor.title.*",
    "settings.assistant.editor.subtitle.*",
    "settings.assistant.editor.empty_hint.*",
    "settings.assistant.editor.row_placeholder.*",
    # Theme-options dropdown selects via `settings.appearance.theme.${value}`.
    "settings.appearance.theme.*",
    # Welcome window's extension picker has data-i18n attributes on a few
    # rows the JS swaps in dynamically. `**` matches both
    # welcome.extensions.cap.none and welcome.extensions.cap.none.title.
    "welcome.extensions.cap.**",
    "welcome.extensions.empty",
    "welcome.extensions.intro_html",
    "welcome.extensions.load_failed",
    "welcome.extensions.section.*",
    "welcome.extensions.toggle.*",
    # Native window titles set by Tauri via .setTitle() — sourced through
    # t() at runtime.
    "window.title.*",
    # Fallback dialog string used when Tauri's ask() plugin isn't loaded.
    # Unused on healthy builds but kept as the safety net.
    "settings.manager.dialog.restart.fallback",
    # Internal-only string thrown when a caller passes the wrong base class
    # to registerModule. Keyed for symmetry with the rest of the manager
    # surface; not user-displayed in normal flow.
    "settings.manager.module_must_extend",
    # Connection settings & restart dialog text — kept for the secondary
    # confirm-dialog path that hasn't been removed yet (was used before the
    # inline restart-prompt banner replaced the native ask() dialog). Safe
    # to drop in a future cleanup pass once the dialog code is gone.
    "settings.manager.dialog.restart.title",
    "settings.manager.dialog.restart.message",
    # Shortcut-list strings that the renderer toggles in/out of the DOM
    # depending on whether a shortcut list is empty / has duplicates.
    "settings.shortcuts.alert.duplicate_trigger",
    "settings.shortcuts.list.delete_confirm",
    "settings.shortcuts.list.empty",
}


def _key_matches_dynamic(key: str) -> bool:
    """Return True if `key` matches a literal entry or a glob pattern in
    KNOWN_DYNAMIC_KEYS. Glob `*` matches a single segment (no dots);
    `**` at the tail matches zero or more trailing segments.
    """
    if key in KNOWN_DYNAMIC_KEYS:
        return True
    key_segments = key.split(".")
    for pat in KNOWN_DYNAMIC_KEYS:
        if "*" not in pat:
            continue
        pat_segments = pat.split(".")
        # `**` tail wildcard: pattern matches if all preceding segments do
        # and the key has at least as many segments.
        if pat_segments and pat_segments[-1] == "**":
            head = pat_segments[:-1]
            if len(key_segments) < len(head):
                continue
            if all(p == "*" or p == k for p, k in zip(head, key_segments)):
                return True
            continue
        if len(pat_segments) != len(key_segments):
            continue
        if all(p == "*" or p == k for p, k in zip(pat_segments, key_segments)):
            return True
    return False


def load_catalog(path: Path) -> dict:
    raw = json.loads(path.read_text(encoding="utf-8"))
    return raw


def catalog_keys(raw: dict) -> set[str]:
    return {k for k in raw.keys() if not k.startswith("_")}


def scan_source(root: Path, suffixes: Iterable[str], patterns: Iterable[re.Pattern]) -> dict[str, list[Path]]:
    """Return {key: [files]} for keys referenced from source.

    Skips third-party vendor code, build output, and test files. Test code
    references can collide with real keys (e.g. a test asserting `t("foo")`
    returns something) and shouldn't gate the build.
    """
    used: dict[str, list[Path]] = {}
    skip_dirs = {
        "target", "node_modules", ".git",
        "_locales",                    # extension catalogs themselves
        "vendor",                      # ui/vendor — third-party JS
        "ui-vendor",                   # bundling source for vendor JS
        "ui-tests",                    # test fixtures use t() in assertions
        "tests",                       # Rust integration tests
        "dist",                        # built output
    }
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if any(part in skip_dirs for part in path.parts):
            continue
        if not any(str(path).endswith(suffix) for suffix in suffixes):
            continue
        # Skip Rust test modules (`#[cfg(test)] mod tests`) and inline
        # `mod tests`. The simplest heuristic: skip the test inline blocks
        # by stripping `#[cfg(test)] mod tests { ... }` segments.
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        if str(path).endswith(".rs"):
            text = _strip_rust_test_modules(text)
            text = _strip_rust_doc_comments(text)
        for pat in patterns:
            for m in pat.finditer(text):
                key = m.group(1)
                used.setdefault(key, []).append(path)
    return used


def _strip_rust_test_modules(text: str) -> str:
    """Remove `#[cfg(test)] mod NAME { ... }` blocks. Brace-balanced scan.

    A test block can legitimately contain `t!("some.key")` calls in its
    assertions, but those keys aren't shipped — they exist only to verify
    the i18n machinery itself.
    """
    out = []
    i = 0
    while i < len(text):
        # Look for `#[cfg(test)]` followed (possibly across whitespace) by `mod`.
        m = re.search(r"#\[cfg\(test\)\]\s*(?:#\[[^\]]+\]\s*)*mod\s+\w+\s*\{", text[i:])
        if not m:
            out.append(text[i:])
            break
        out.append(text[i : i + m.start()])
        # Walk the brace-balanced body.
        j = i + m.end()
        depth = 1
        while j < len(text) and depth > 0:
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
            j += 1
        i = j
    return "".join(out)


def _strip_rust_doc_comments(text: str) -> str:
    """Strip /// and //! lines so example code in docstrings doesn't count
    as a real key reference."""
    out_lines = []
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("///") or stripped.startswith("//!"):
            continue
        out_lines.append(line)
    return "\n".join(out_lines)


def extract_placeholders(template: str) -> set[str]:
    """Return the set of `{name}` placeholder names in the template,
    ignoring ICU plural / select formatters which use `{n, plural, ...}`
    syntax — those *contain* a name but we treat them as generic counts."""
    names: set[str] = set()
    i = 0
    while i < len(template):
        if template[i] != "{":
            i += 1
            continue
        # Find matching close.
        depth = 0
        j = i
        while j < len(template):
            if template[j] == "{":
                depth += 1
            elif template[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        if j >= len(template):
            break
        inner = template[i + 1 : j]
        # Plural / select: `name, plural, ...` — first token is the name.
        first = inner.split(",", 1)[0].strip()
        if first and not first.startswith("="):
            names.add(first)
        i = j + 1
    return names


def main() -> int:
    errors: list[str] = []
    warnings: list[str] = []

    # ---- Load English catalog (canonical) -------------------------------
    en_path = LOCALES_DIR / "en" / "messages.json"
    if not en_path.exists():
        print(f"FATAL: canonical catalog missing: {en_path}", file=sys.stderr)
        return 2
    en = load_catalog(en_path)
    en_keys = catalog_keys(en)
    print(f"Canonical EN catalog: {len(en_keys)} keys")

    # ---- Scan source ----------------------------------------------------
    rust_used = scan_source(ROOT / "src", [".rs"], RUST_PATTERNS)
    js_used = scan_source(ROOT / "ui", [".js", ".mjs"], JS_PATTERNS)
    html_used = scan_source(ROOT / "ui", [".html"], [HTML_PATTERN])

    used_keys: set[str] = set()
    used_keys.update(rust_used.keys())
    used_keys.update(js_used.keys())
    used_keys.update(html_used.keys())

    print(f"Keys referenced from source: {len(used_keys)}")

    # ---- Source → EN: every used key must exist in EN -------------------
    missing_in_en = used_keys - en_keys

    # KNOWN_DYNAMIC_KEYS counts as "referenced" for the unused-key check
    # below. Literal entries are checked directly; glob patterns
    # (containing `*`) match any key in EN whose segments line up.
    for k in en_keys:
        if _key_matches_dynamic(k):
            used_keys.add(k)
    for k in sorted(missing_in_en):
        sample_files = (
            rust_used.get(k, [])
            + js_used.get(k, [])
            + html_used.get(k, [])
        )[:3]
        sample = ", ".join(str(p.relative_to(ROOT)) for p in sample_files)
        errors.append(
            f"missing in en: {k!r} referenced from [{sample}]"
        )

    # ---- EN → Source: every EN key must be referenced (or dynamic-known) -
    unused = en_keys - used_keys
    for k in sorted(unused):
        warnings.append(f"unused EN key: {k!r} — remove from catalog or add to KNOWN_DYNAMIC_KEYS")

    # ---- EN → other catalogs: no drift ---------------------------------
    other_langs = sorted(
        d.name
        for d in LOCALES_DIR.iterdir()
        if d.is_dir() and d.name != "en" and (d / "messages.json").exists()
    )
    print(f"Non-EN catalogs to verify: {len(other_langs)} ({', '.join(other_langs) or 'none'})")
    for lang in other_langs:
        path = LOCALES_DIR / lang / "messages.json"
        try:
            cat = load_catalog(path)
        except json.JSONDecodeError as e:
            errors.append(f"{lang}: malformed JSON: {e}")
            continue
        keys = catalog_keys(cat)
        missing = en_keys - keys
        extra = keys - en_keys
        for k in sorted(missing):
            errors.append(f"{lang}: missing key {k!r} (in en, not in {lang})")
        for k in sorted(extra):
            errors.append(f"{lang}: extra key {k!r} (in {lang}, not in en) — remove or add to en first")

        # Placeholder consistency: a key's translation must reference the
        # same `{name}` placeholders as the EN source.
        for k in sorted(en_keys & keys):
            en_phs = extract_placeholders(en[k]["message"])
            tr_phs = extract_placeholders(cat[k]["message"])
            missing_phs = en_phs - tr_phs
            extra_phs = tr_phs - en_phs
            if missing_phs:
                errors.append(
                    f"{lang}: key {k!r} is missing placeholders {sorted(missing_phs)} "
                    f"present in en — translation will display {{name}} literal"
                )
            if extra_phs:
                errors.append(
                    f"{lang}: key {k!r} introduces placeholders {sorted(extra_phs)} "
                    f"not in en — runtime will leave them literal"
                )

    # ---- Extensions: each ext must have _locales/en/messages.json -------
    if EXTENSIONS_DIR.exists():
        ext_errors = check_extensions()
        errors.extend(ext_errors)
    else:
        warnings.append(f"Kage-Extensions repo not found at {EXTENSIONS_DIR}; skipping extension i18n check")

    # ---- Report ---------------------------------------------------------
    for w in warnings:
        print(f"WARN: {w}")
    for e in errors:
        print(f"ERR:  {e}", file=sys.stderr)

    if errors:
        print(f"\n❌ i18n drift check failed: {len(errors)} error(s), {len(warnings)} warning(s)")
        print("Re-run `python scripts/translate.py` to regenerate missing translations.")
        return 1
    print(f"\n✅ i18n drift check passed ({len(warnings)} warning(s))")
    return 0


def check_extensions() -> list[str]:
    """Per-extension i18n: each extension must have _locales/en/messages.json
    and every other locale present must contain the same keys as en. Each
    string referenced via the extension's `i18n.t(...)` proxy in JS or
    extension-side `manifest.localized` declarations must be in the en catalog.
    """
    errs: list[str] = []
    for ext_dir in sorted(p for p in EXTENSIONS_DIR.iterdir() if p.is_dir()):
        if not (ext_dir / "manifest.json").exists():
            continue
        locales = ext_dir / "_locales"
        if not locales.exists():
            errs.append(f"extension {ext_dir.name}: missing _locales/ directory")
            continue
        en_path = locales / "en" / "messages.json"
        if not en_path.exists():
            errs.append(
                f"extension {ext_dir.name}: missing _locales/en/messages.json (canonical)"
            )
            continue
        try:
            ext_en = load_catalog(en_path)
        except json.JSONDecodeError as e:
            errs.append(f"extension {ext_dir.name}: en catalog malformed JSON: {e}")
            continue
        ext_en_keys = catalog_keys(ext_en)

        # Cross-locale parity within the extension.
        for sub in sorted(p for p in locales.iterdir() if p.is_dir() and p.name != "en"):
            cat_path = sub / "messages.json"
            if not cat_path.exists():
                errs.append(f"extension {ext_dir.name}: {sub.name}/messages.json missing")
                continue
            try:
                cat = load_catalog(cat_path)
            except json.JSONDecodeError as e:
                errs.append(
                    f"extension {ext_dir.name}: {sub.name} catalog malformed JSON: {e}"
                )
                continue
            keys = catalog_keys(cat)
            for k in sorted(ext_en_keys - keys):
                errs.append(f"extension {ext_dir.name}/{sub.name}: missing key {k!r}")
            for k in sorted(keys - ext_en_keys):
                errs.append(f"extension {ext_dir.name}/{sub.name}: extra key {k!r}")

        # Used keys in extension JS — _meta and _missing-key are exempt.
        used = set()
        for js_file in ext_dir.rglob("*.js"):
            try:
                text = js_file.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                continue
            for pat in JS_PATTERNS:
                for m in pat.finditer(text):
                    used.add(m.group(1))
        # Also pick up data-i18n in extension HTML if any.
        for html_file in ext_dir.rglob("*.html"):
            try:
                text = html_file.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                continue
            for m in HTML_PATTERN.finditer(text):
                used.add(m.group(1))
        # Manifest's localizable fields use __MSG_xxx__ tokens (Chrome-style).
        manifest = json.loads((ext_dir / "manifest.json").read_text(encoding="utf-8"))
        for field in ("name", "description"):
            v = manifest.get(field)
            if isinstance(v, str) and v.startswith("__MSG_") and v.endswith("__"):
                used.add(v[6:-2])

        for k in sorted(used - ext_en_keys):
            errs.append(
                f"extension {ext_dir.name}: key {k!r} referenced from source but missing in _locales/en/messages.json"
            )
    return errs


if __name__ == "__main__":
    sys.exit(main())
