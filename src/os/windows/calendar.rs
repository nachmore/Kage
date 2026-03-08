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

use log::{info, warn};
use std::process::{Command, Stdio};
use crate::os::calendar::CalendarEvent;

pub fn get_upcoming_events_impl(hours: u32) -> Vec<CalendarEvent> {
    // Spawn STA thread for COM
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let results = get_events_via_outlook(hours);
        let _ = tx.send(results);
    });

    rx.recv().unwrap_or_default()
}

fn get_events_via_outlook(hours: u32) -> Vec<CalendarEvent> {
    // PowerShell fetches raw appointment data from Outlook COM.
    // All post-processing (meeting URL extraction, etc.) happens in Rust
    // in the cross-platform calendar module.
    let ps_script = format!(
        r#"
try {{
    $ol = New-Object -COM Outlook.Application
    $ns = $ol.GetNamespace("MAPI")
    $cal = $ns.GetDefaultFolder(9) # olFolderCalendar
    $now = Get-Date
    $end = $now.AddHours({hours})
    $filter = "[Start] >= '" + $now.ToString("g") + "' AND [Start] <= '" + $end.ToString("g") + "'"
    $items = $cal.Items
    $items.Sort("[Start]")
    $items.IncludeRecurrences = $true
    $restricted = $items.Restrict($filter)
    $results = @()
    foreach ($item in $restricted) {{
        if ($results.Count -ge 50) {{ break }}
        $body = ""
        try {{ $body = [string]$item.Body }} catch {{}}
        $results += [PSCustomObject]@{{
            id = [string]$item.EntryID
            subject = [string]$item.Subject
            location = [string]$item.Location
            organizer = [string]$item.Organizer
            start_time = $item.Start.ToUniversalTime().ToString("o")
            duration_minutes = [int]$item.Duration
            all_day = [bool]$item.AllDayEvent
            body = $body
        }}
    }}
    $results | ConvertTo-Json -Compress
}} catch {{
    Write-Error $_.Exception.Message
}}
"#,
        hours = hours,
    );

    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            warn!("[calendar] Failed to run PowerShell: {}", e);
            return vec![];
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        warn!("[calendar] PowerShell stderr: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        info!("[calendar] No events returned from Outlook");
        return vec![];
    }

    // Parse raw events from PowerShell
    let raw: Vec<RawOutlookEvent> = if stdout.trim().starts_with('[') {
        serde_json::from_str(&stdout).unwrap_or_default()
    } else {
        serde_json::from_str::<RawOutlookEvent>(&stdout)
            .map(|e| vec![e])
            .unwrap_or_default()
    };

    info!("[calendar] Parsed {} raw events from Outlook", raw.len());

    // Convert to CalendarEvent — meeting URL extraction happens in the
    // cross-platform calendar module via extract_meeting_url()
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
