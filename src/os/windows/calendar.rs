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
// Results are cached for 2 minutes on the JS side to avoid repeated calls.
//
// DEBUGGING: A standalone PowerShell version of this query lives in
// scripts/outlook_calendar.ps1 — keep it in sync when changing the query logic here.

use log::{debug, info, warn};
use std::process::{Command, Stdio};
use crate::os::calendar::CalendarEvent;

pub fn get_upcoming_events_impl(hours: u32) -> Vec<CalendarEvent> {
    let time_range = format!(
        "$start = Get-Date; $end = $start.AddHours({hours})",
        hours = hours,
    );
    run_on_sta(move || query_outlook(&time_range, "upcoming"))
}

pub fn get_events_for_date_impl(date: &str) -> Vec<CalendarEvent> {
    let time_range = format!(
        "$start = [DateTime]::Parse(\"{date}\"); $end = $start.AddDays(1)",
        date = date,
    );
    run_on_sta(move || query_outlook(&time_range, &format!("date {}", time_range)))
}

/// Spawn a closure on a dedicated thread (needed for COM/STA) and wait for the result.
fn run_on_sta<F>(f: F) -> Vec<CalendarEvent>
where
    F: FnOnce() -> Vec<CalendarEvent> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv().unwrap_or_default()
}

/// Core Outlook COM query. `time_range_setup` is a PowerShell snippet that sets
/// `$start` and `$end` variables. Everything else (COM init, filter, iteration,
/// JSON output) is shared.
fn query_outlook(time_range_setup: &str, label: &str) -> Vec<CalendarEvent> {
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

    debug!("[calendar] Running PowerShell query ({}), time_range: {}", label, time_range_setup);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            warn!("[calendar] Failed to run PowerShell ({}): {}", label, e);
            return vec![];
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        warn!("[calendar] PowerShell stderr ({}, {} bytes): {}", label, stderr.len(), stderr.trim());
    }

    let exit_code = output.status.code();
    info!("[calendar] PowerShell exit code ({}): {:?}", label, exit_code);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout_trimmed = stdout.trim();

    if stdout_trimmed.is_empty() {
        info!("[calendar] No events returned — stdout empty ({})", label);
        return vec![];
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
                warn!("[calendar] Failed to parse JSON array ({}): {} | around col {}: {:?}",
                    label, e, col, context);
                vec![]
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
                warn!("[calendar] Failed to parse single JSON object ({}): {} | around col {}: {:?}",
                    label, e, col, context);
                vec![]
            }
        }
    } else {
        warn!("[calendar] Unexpected stdout — not JSON ({}): {}",
            label, if stdout.len() > 200 { &stdout[..200] } else { &stdout });
        vec![]
    };

    info!("[calendar] Parsed {} events ({})", raw.len(), label);

    raw.into_iter().map(|r| {
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
    }).collect()
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
