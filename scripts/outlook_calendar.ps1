# Outlook Calendar Query Script
#
# Standalone version of the PowerShell scripts embedded in:
#   src/os/windows/calendar.rs
#
# KEEP IN SYNC: If you change the Outlook COM query logic in calendar.rs,
# update this script to match (and vice versa). The Rust code uses these
# same queries via powershell -Command, so this file is the debuggable
# equivalent for testing outside the app.
#
# Usage:
#   .\outlook_calendar.ps1                  # Upcoming events (next 4 hours)
#   .\outlook_calendar.ps1 -Hours 8         # Upcoming events (next 8 hours)
#   .\outlook_calendar.ps1 -Date 2026-03-10 # All events on a specific date
#   .\outlook_calendar.ps1 -Raw             # Output raw JSON (like the Rust code sees)
#   .\outlook_calendar.ps1 -Raw -DumpFile out.json  # Save raw JSON to file for inspection
#   .\outlook_calendar.ps1 -RawNoSanitize   # Raw JSON WITHOUT body sanitization (reproduces Rust bug)

param(
    [int]$Hours = 4,
    [string]$Date = "",
    [switch]$Raw,
    [switch]$RawNoSanitize,
    [string]$DumpFile = ""
)

try {
    # Force UTF-8 output so Unicode chars (smart quotes etc.) don't get
    # mangled to ASCII equivalents which break JSON string values.
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $ol = New-Object -COM Outlook.Application
    $ns = $ol.GetNamespace("MAPI")
    $cal = $ns.GetDefaultFolder(9) # olFolderCalendar

    if ($Date -ne "") {
        # Date mode: all events on a specific date (midnight to midnight)
        $start = [DateTime]::Parse($Date)
        $end = $start.AddDays(1)
        Write-Host "Querying events for date: $Date" -ForegroundColor Cyan
    } else {
        # Upcoming mode: events in the next N hours
        $start = Get-Date
        $end = $start.AddHours($Hours)
        Write-Host "Querying upcoming events: next $Hours hours" -ForegroundColor Cyan
        Write-Host "  From: $start" -ForegroundColor DarkGray
        Write-Host "  To:   $end" -ForegroundColor DarkGray
    }

    $filter = "[End] >= '" + $start.ToString("g") + "' AND [Start] < '" + $end.ToString("g") + "'"
    Write-Host "  Filter: $filter" -ForegroundColor DarkGray
    Write-Host ""

    $items = $cal.Items
    $items.Sort("[Start]")
    $items.IncludeRecurrences = $true
    $restricted = $items.Restrict($filter)

    # JSON-encode a string value: escape backslashes, quotes, and control chars.
    # We build JSON manually because ConvertTo-Json in PS 5.1 has known bugs
    # with unescaped quotes inside string properties.
    function esc($s) {
        if ($s -eq $null) { return "" }
        $s = [string]$s
        $bslash = [char]92   # backslash
        $dquote = [char]34   # double quote
        $s = $s.Replace([string]$bslash, [string]$bslash + [string]$bslash)
        $s = $s.Replace([string]$dquote, [string]$bslash + [string]$dquote)
        $s = $s -replace '[\x00-\x08\x0B\x0C\x0E-\x1F]', ''
        $s = $s.Replace("`r`n", '\r\n').Replace("`r", '\r').Replace("`n", '\n').Replace("`t", '\t')
        return $s
    }

    $jsonItems = @()
    foreach ($item in $restricted) {
        if ($jsonItems.Count -ge 50) { break }
        $body = ""
        try {
            $body = [string]$item.Body
            if ($body.Length -gt 4000) { $body = $body.Substring(0, 4000) }
        } catch {}
        $ad = if ($item.AllDayEvent) { "true" } else { "false" }
        $jsonItems += '{{"id":"{0}","subject":"{1}","location":"{2}","organizer":"{3}","start_time":"{4}","duration_minutes":{5},"all_day":{6},"body":"{7}"}}' -f `
            (esc $item.EntryID), (esc $item.Subject), (esc $item.Location), (esc $item.Organizer), `
            (esc $item.Start.ToUniversalTime().ToString("o")), [int]$item.Duration, $ad, (esc $body)
    }
    $jsonOutput = "[" + ($jsonItems -join ",") + "]"

    if ($Raw -or $RawNoSanitize) {
        if ($DumpFile -ne "") {
            $jsonOutput | Out-File -FilePath $DumpFile -Encoding utf8
            Write-Host "Wrote $($jsonOutput.Length) chars to $DumpFile" -ForegroundColor Green
        } else {
            $jsonOutput
        }
    } else {
        # Pretty-print: parse our own JSON back for display
        $results = $jsonOutput | ConvertFrom-Json
        Write-Host "Found $($results.Count) event(s):" -ForegroundColor Green
        Write-Host ""
        foreach ($evt in $results) {
            $localStart = [DateTime]::Parse($evt.start_time).ToLocalTime()
            $endTime = $localStart.AddMinutes($evt.duration_minutes)
            $timeStr = if ($evt.all_day) { "All Day" } else { "$($localStart.ToString('HH:mm')) - $($endTime.ToString('HH:mm'))" }

            Write-Host "  $timeStr  $($evt.subject)" -ForegroundColor Yellow
            if ($evt.location) { Write-Host "           Location:  $($evt.location)" -ForegroundColor DarkGray }
            if ($evt.organizer) { Write-Host "           Organizer: $($evt.organizer)" -ForegroundColor DarkGray }
            Write-Host ""
        }
        if ($results.Count -eq 0) {
            Write-Host "  (no events)" -ForegroundColor DarkGray
        }
    }
} catch {
    Write-Error $_.Exception.Message
    Write-Host ""
    Write-Host "Common issues:" -ForegroundColor Yellow
    Write-Host "  - Outlook must be installed (desktop version, not just web)" -ForegroundColor DarkGray
    Write-Host "  - Outlook must have been opened at least once to set up the profile" -ForegroundColor DarkGray
    Write-Host "  - If using New Outlook, COM automation may not be available" -ForegroundColor DarkGray
} finally {
    # Release COM objects in reverse order of creation. Without this, the
    # Outlook process can stay pinned in memory after the script exits, and
    # repeated runs leak marshalling resources.
    foreach ($comObj in @($restricted, $items, $cal, $ns, $ol)) {
        if ($null -ne $comObj) {
            try { [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($comObj) } catch {}
        }
    }
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
