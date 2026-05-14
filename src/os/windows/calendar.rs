// Windows calendar integration.
//
// Uses PowerShell + Outlook COM automation to read calendar events.
// This works with Outlook desktop (Exchange, O365, personal accounts)
// which is the most common calendar source on Windows PCs.
//
// WHY POWERSHELL?
// The Outlook Object Model is a COM automation API (IDispatch-based) that requires
// late-binding. Rust's `windows` crate only supports early-bound COM (vtable-based),
// not IDispatch/late-binding. Calling Outlook COM from Rust would require a custom
// IDispatch wrapper with manual VARIANT marshaling — hundreds of lines of unsafe code
// for something PowerShell does natively in 20 lines.
//
// The Windows.ApplicationModel.Appointments WinRT API was tried first but is unreliable
// on desktop PCs — it often returns 0 events even when the Windows Calendar app shows them.
// This is a known issue (see SO #42457282).
//
// PowerShell overhead is ~300ms for startup, but the actual Outlook query is instant.
// Results are cached on the JS side (shared cache in cache.js, expires at :25/:55
// each hour) to avoid repeated calls.
//
// DEBUGGING: A standalone PowerShell version of this query lives in
// scripts/outlook_calendar.ps1 — keep it in sync when changing the query logic here.

use crate::os::calendar::CalendarEvent;
use log::{debug, info, warn};
use std::process::{Command, Stdio};

pub fn get_upcoming_events_impl(hours: u32) -> Result<Vec<CalendarEvent>, String> {
    let time_range = format!(
        "$start = Get-Date; $end = $start.AddHours({hours})",
        hours = hours,
    );
    run_on_sta(move || query_outlook(&time_range, "upcoming"))
}

pub fn get_events_for_date_impl(date: &str) -> Result<Vec<CalendarEvent>, String> {
    // Reject anything that isn't a strict YYYY-MM-DD date. The date string is
    // interpolated into a double-quoted PowerShell literal below; without this
    // gate a value like `2024-01-01"; Get-Process; "` would break out and
    // execute arbitrary PowerShell.
    if !is_strict_iso_date(date) {
        warn!("[calendar] rejected non-ISO date: {:?}", date);
        return Ok(vec![]);
    }
    let time_range = format!(
        "$start = [DateTime]::Parse(\"{date}\"); $end = $start.AddDays(1)",
        date = date,
    );
    run_on_sta(move || query_outlook(&time_range, &format!("date {}", time_range)))
}

/// Strict YYYY-MM-DD check. No surrogate pairs, only ASCII digits and dashes,
/// matches exactly the expected shape. Deliberately does NOT use chrono — we
/// want to reject *syntactically* weird inputs before PowerShell sees them,
/// even if they'd parse as a date.
fn is_strict_iso_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 10 {
        return false;
    }
    let is_digit = |b: u8| b.is_ascii_digit();
    is_digit(bytes[0])
        && is_digit(bytes[1])
        && is_digit(bytes[2])
        && is_digit(bytes[3])
        && bytes[4] == b'-'
        && is_digit(bytes[5])
        && is_digit(bytes[6])
        && bytes[7] == b'-'
        && is_digit(bytes[8])
        && is_digit(bytes[9])
}

/// Spawn a closure on a dedicated thread (needed for COM/STA) and wait for the result.
fn run_on_sta<F>(f: F) -> Result<Vec<CalendarEvent>, String>
where
    F: FnOnce() -> Result<Vec<CalendarEvent>, String> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv()
        .unwrap_or_else(|_| Err("Calendar worker thread panicked".to_string()))
}

/// Core Outlook COM query. `time_range_setup` is a PowerShell snippet that sets
/// `$start` and `$end` variables. Everything else (COM init, filter, iteration,
/// JSON output) is shared.
fn query_outlook(time_range_setup: &str, label: &str) -> Result<Vec<CalendarEvent>, String> {
    let ps_script = format!(
        r#"
try {{
    # Force UTF-8 output so Unicode chars (smart quotes etc.) don't get
    # mangled to ASCII equivalents which break JSON string values.
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $ol = New-Object -COM Outlook.Application
    $ns = $ol.GetNamespace("MAPI")
    $cal = $ns.GetDefaultFolder(9) # olFolderCalendar
    {time_range}
    $filter = "[End] >= '" + $start.ToString("g") + "' AND [Start] < '" + $end.ToString("g") + "'"
    $items = $cal.Items
    $items.Sort("[Start]")
    $items.IncludeRecurrences = $true
    $restricted = $items.Restrict($filter)
    # JSON-encode a string value: escape backslashes, quotes, and control chars.
    # We build JSON manually because ConvertTo-Json in PS 5.1 has known bugs
    # with unescaped quotes inside string properties.
    function esc($s) {{
        if ($s -eq $null) {{ return "" }}
        $s = [string]$s
        $bslash = [char]92   # backslash
        $dquote = [char]34   # double quote
        $s = $s.Replace([string]$bslash, [string]$bslash + [string]$bslash)
        $s = $s.Replace([string]$dquote, [string]$bslash + [string]$dquote)
        $s = $s -replace '[\x00-\x08\x0B\x0C\x0E-\x1F]', ''
        $s = $s.Replace("`r`n", '\r\n').Replace("`r", '\r').Replace("`n", '\n').Replace("`t", '\t')
        return $s
    }}
    $jsonItems = @()
    foreach ($item in $restricted) {{
        if ($jsonItems.Count -ge 50) {{ break }}
        $body = ""
        try {{
            $body = [string]$item.Body
            if ($body.Length -gt 4000) {{ $body = $body.Substring(0, 4000) }}
        }} catch {{}}
        $ad = if ($item.AllDayEvent) {{ "true" }} else {{ "false" }}
        $j = '{{' + '"id":"' + (esc $item.EntryID) + '",'
        $j += '"subject":"' + (esc $item.Subject) + '",'
        $j += '"location":"' + (esc $item.Location) + '",'
        $j += '"organizer":"' + (esc $item.Organizer) + '",'
        $j += '"start_time":"' + (esc $item.Start.ToUniversalTime().ToString("o")) + '",'
        $j += '"duration_minutes":' + [string][int]$item.Duration + ','
        $j += '"all_day":' + $ad + ','
        $j += '"body":"' + (esc $body) + '"'
        $j += '}}'
        $jsonItems += $j
    }}
    "[" + ($jsonItems -join ",") + "]"
}} catch {{
    Write-Error $_.Exception.Message
}}
"#,
        time_range = time_range_setup,
    );

    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    debug!(
        "[calendar] Running PowerShell query ({}), time_range: {}",
        label, time_range_setup
    );

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            warn!("[calendar] Failed to run PowerShell ({}): {}", label, e);
            return Err(format!("Failed to run PowerShell: {}", e));
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        warn!(
            "[calendar] PowerShell stderr ({}, {} bytes): {}",
            label,
            stderr.len(),
            stderr.trim()
        );
    }

    let exit_code = output.status.code();
    if exit_code != Some(0) {
        warn!(
            "[calendar] PowerShell exited with code {:?} ({})",
            exit_code, label
        );
        // Non-zero exit with stderr content means Outlook COM failed (not installed,
        // not running, or access denied). Surface this to the user.
        if !stderr.trim().is_empty() {
            return Err(format!("Outlook calendar query failed: {}", stderr.trim()));
        }
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout_trimmed = stdout.trim();

    if stdout_trimmed.is_empty() {
        info!("[calendar] No events returned — stdout empty ({})", label);
        return Ok(vec![]);
    }

    // Sanitize JSON: PowerShell's ConvertTo-Json and manual string building can
    // produce invalid JSON when calendar bodies contain unescaped quotes (including
    // Unicode smart quotes \u201C \u201D that get treated as string delimiters).
    // Fix by escaping unescaped quotes inside JSON string values on the Rust side.
    let sanitized = sanitize_ps_json(&stdout);

    let raw: Vec<RawOutlookEvent> = if stdout_trimmed.starts_with('[') {
        match serde_json::from_str(&sanitized) {
            Ok(v) => v,
            Err(e) => {
                let col = e.column().saturating_sub(1);
                let context_start = col.saturating_sub(80);
                let context_end = (col + 80).min(stdout.len());
                let context = if context_end <= stdout.len() {
                    &stdout[context_start..context_end]
                } else {
                    &stdout[context_start..]
                };
                warn!(
                    "[calendar] Failed to parse JSON array ({}): {} | around col {}: {:?}",
                    label, e, col, context
                );
                return Ok(vec![]);
            }
        }
    } else if stdout_trimmed.starts_with('{') {
        match serde_json::from_str::<RawOutlookEvent>(&sanitized) {
            Ok(e) => vec![e],
            Err(e) => {
                let col = e.column().saturating_sub(1);
                let context_start = col.saturating_sub(80);
                let context_end = (col + 80).min(stdout.len());
                let context = if context_end <= stdout.len() {
                    &stdout[context_start..context_end]
                } else {
                    &stdout[context_start..]
                };
                warn!(
                    "[calendar] Failed to parse single JSON object ({}): {} | around col {}: {:?}",
                    label, e, col, context
                );
                return Ok(vec![]);
            }
        }
    } else {
        warn!(
            "[calendar] Unexpected stdout — not JSON ({}): {}",
            label,
            if stdout.len() > 200 {
                &stdout[..200]
            } else {
                &stdout
            }
        );
        return Ok(vec![]);
    };

    info!("[calendar] Parsed {} events ({})", raw.len(), label);

    Ok(raw
        .into_iter()
        .map(|r| {
            let online_url = crate::os::calendar::extract_meeting_url(&r.location, &r.body);
            CalendarEvent {
                id: r.id,
                subject: r.subject,
                location: r.location,
                organizer: r.organizer,
                start_time: r.start_time,
                duration_minutes: r.duration_minutes,
                all_day: r.all_day,
                online_url,
            }
        })
        .collect())
}

/// Sanitize JSON output from PowerShell. Walks through the string and escapes
/// any characters inside JSON string values that would break parsing:
/// - Literal CR/LF (should be \r\n)
/// - Literal tabs (should be \t)
/// - Other control characters
///
/// This handles cases where PowerShell's string escaping is incomplete.
fn sanitize_ps_json(input: &str) -> String {
    let mut result = String::with_capacity(input.len() + 256);
    let mut in_string = false;
    let mut prev_backslash = false;

    for ch in input.chars() {
        if in_string {
            if prev_backslash {
                // Previous char was \, this is an escape sequence — pass through
                result.push(ch);
                prev_backslash = false;
                continue;
            }
            match ch {
                '\\' => {
                    result.push(ch);
                    prev_backslash = true;
                }
                '"' => {
                    // End of string
                    result.push(ch);
                    in_string = false;
                }
                '\r' => result.push_str("\\r"),
                '\n' => result.push_str("\\n"),
                '\t' => result.push_str("\\t"),
                c if c.is_control() => {
                    // Escape other control chars as \uXXXX
                    result.push_str(&format!("\\u{:04x}", c as u32));
                }
                _ => result.push(ch),
            }
        } else {
            result.push(ch);
            if ch == '"' {
                in_string = true;
            }
            prev_backslash = false;
        }
    }
    result
}

/// Raw event data from Outlook COM — minimal processing, just deserialization.
#[derive(serde::Deserialize, Default)]
struct RawOutlookEvent {
    #[serde(default)]
    id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    organizer: String,
    #[serde(default)]
    start_time: String,
    #[serde(default)]
    duration_minutes: u32,
    #[serde(default)]
    all_day: bool,
    #[serde(default)]
    body: String,
}

#[cfg(test)]
mod tests {
    //! Coverage for the pure helpers: date format validation and the
    //! PowerShell JSON sanitizer. Both are defensive layers against
    //! untrusted Outlook data / malformed PS output, worth locking in.

    use super::{is_strict_iso_date, sanitize_ps_json};

    // ---- is_strict_iso_date ------------------------------------------------

    #[test]
    fn iso_date_accepts_valid_shape() {
        assert!(is_strict_iso_date("2026-04-27"));
        assert!(is_strict_iso_date("1999-01-01"));
        assert!(is_strict_iso_date("2000-12-31"));
    }

    #[test]
    fn iso_date_rejects_wrong_length() {
        assert!(!is_strict_iso_date(""));
        assert!(!is_strict_iso_date("2026-04-2"));
        assert!(!is_strict_iso_date("2026-04-277"));
        assert!(!is_strict_iso_date("26-04-27"));
    }

    #[test]
    fn iso_date_rejects_non_digit_in_digit_positions() {
        assert!(!is_strict_iso_date("2O26-04-27")); // O not 0
        assert!(!is_strict_iso_date("abcd-04-27"));
    }

    #[test]
    fn iso_date_rejects_wrong_separator() {
        assert!(!is_strict_iso_date("2026/04/27"));
        assert!(!is_strict_iso_date("2026.04.27"));
        assert!(!is_strict_iso_date("2026 04 27"));
    }

    #[test]
    fn iso_date_rejects_non_ascii() {
        // Unicode digits look like digits but aren't ASCII.
        assert!(!is_strict_iso_date("٢٠٢٦-٠٤-٢٧"));
    }

    #[test]
    fn iso_date_rejects_injection_attempts() {
        // This is the whole point: don't let a crafted date reach PS.
        assert!(!is_strict_iso_date("2026-04-27; rm -rf /"));
        assert!(!is_strict_iso_date("' OR 1=1"));
        assert!(!is_strict_iso_date("$(whoami)"));
    }

    // ---- sanitize_ps_json --------------------------------------------------

    #[test]
    fn sanitize_passes_plain_json_unchanged() {
        let input = r#"{"id": "abc", "subject": "Hello"}"#;
        assert_eq!(sanitize_ps_json(input), input);
    }

    #[test]
    fn sanitize_escapes_literal_newline_inside_string() {
        // PowerShell sometimes emits a raw \n inside a JSON string literal
        // when the field value has a line break. That breaks standard JSON
        // parsers — sanitize should convert to \\n.
        let input = "{\"body\": \"line1\nline2\"}";
        let out = sanitize_ps_json(input);
        assert!(out.contains("line1\\nline2"), "got {}", out);
        // And the output should actually parse as JSON.
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parseable");
        assert_eq!(parsed["body"], "line1\nline2");
    }

    #[test]
    fn sanitize_escapes_carriage_return_and_tab() {
        let input = "{\"t\": \"a\tb\", \"r\": \"c\rd\"}";
        let out = sanitize_ps_json(input);
        assert!(out.contains("a\\tb"));
        assert!(out.contains("c\\rd"));
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parseable");
        assert_eq!(parsed["t"], "a\tb");
        assert_eq!(parsed["r"], "c\rd");
    }

    #[test]
    fn sanitize_escapes_other_control_chars_as_unicode() {
        // Bell character (\u0007) should be unicode-escaped.
        let input = "{\"x\": \"hi\u{0007}there\"}";
        let out = sanitize_ps_json(input);
        assert!(out.contains(r"\u0007"), "got {}", out);
    }

    #[test]
    fn sanitize_does_not_touch_already_escaped_sequences() {
        // An already-valid JSON string "line1\nline2" should be untouched.
        let input = r#"{"body": "line1\nline2"}"#;
        let out = sanitize_ps_json(input);
        assert_eq!(out, input);
    }

    #[test]
    fn sanitize_does_not_touch_control_chars_outside_strings() {
        // Control chars between tokens (whitespace) are JSON-legal.
        let input = "{\n  \"a\": 1\n}";
        let out = sanitize_ps_json(input);
        assert_eq!(out, input);
    }

    #[test]
    fn sanitize_preserves_quote_after_escape() {
        // In the string "foo\"bar", the escaped quote must not close
        // the string. The sanitizer needs to see the backslash and
        // pass the quote through.
        let input = r#"{"s": "foo\"bar"}"#;
        let out = sanitize_ps_json(input);
        assert_eq!(out, input);
        let _parsed: serde_json::Value = serde_json::from_str(&out).expect("parseable");
    }

    #[test]
    fn sanitize_handles_empty_input() {
        assert_eq!(sanitize_ps_json(""), "");
    }

    #[test]
    fn sanitize_handles_json_arrays() {
        // Outlook events come back as an array; sanitize shouldn't
        // break array syntax.
        let input = "[{\"id\": \"1\"}, {\"id\": \"2\nhm\"}]";
        let out = sanitize_ps_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parseable");
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["id"], "1");
        assert_eq!(parsed[1]["id"], "2\nhm");
    }
}
