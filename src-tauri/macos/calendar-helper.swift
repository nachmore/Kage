// Kage calendar helper — reads EventKit events and outputs JSON.
//
// Usage:
//   kage-calendar-helper upcoming <hours>
//   kage-calendar-helper date <yyyy-MM-dd>
//
// Output (stdout): JSON array of { id, subject, location, organizer,
// start_time (ISO8601), duration_minutes, all_day, online_url }.
//
// Error handling: writes a JSON object { "error": "<message>" } to stdout
// and exits with a non-zero code. The Rust caller treats any non-JSON
// stdout as fatal and falls back to icalBuddy (or empty).
//
// Permission: uses EKEventStore.requestFullAccessToEvents on macOS 14+,
// falls back to requestAccess(to:) on older versions. If the user denies
// access, we exit with an "error: access denied" payload — the Rust
// caller surfaces this verbatim.

import EventKit
import Foundation

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

enum Mode {
    case upcoming(hours: Int)
    case date(iso: String)
}

func parseMode(_ argv: [String]) -> Mode? {
    guard argv.count >= 3 else { return nil }
    switch argv[1] {
    case "upcoming":
        if let h = Int(argv[2]), h > 0 {
            return .upcoming(hours: h)
        }
        return nil
    case "date":
        return .date(iso: argv[2])
    default:
        return nil
    }
}

// ---------------------------------------------------------------------------
// JSON emit helpers
// ---------------------------------------------------------------------------

func emitError(_ message: String) -> Never {
    let payload: [String: String] = ["error": message]
    if let data = try? JSONSerialization.data(withJSONObject: payload),
       let str = String(data: data, encoding: .utf8) {
        print(str)
    } else {
        print("{\"error\": \"unknown\"}")
    }
    exit(1)
}

func emit(_ events: [[String: Any]]) {
    let data = (try? JSONSerialization.data(withJSONObject: events, options: [])) ?? Data("[]".utf8)
    let str = String(data: data, encoding: .utf8) ?? "[]"
    print(str)
}

// ---------------------------------------------------------------------------
// Permission
// ---------------------------------------------------------------------------

func requestAccess(store: EKEventStore) -> Bool {
    let semaphore = DispatchSemaphore(value: 0)
    var granted = false

    // macOS 14+ uses the granular "full access" API; older macOS uses the
    // deprecated single-tier requestAccess. Try new API via selector so
    // this file compiles against older SDKs without #available compile-time
    // guards that force a minimum SDK.
    //
    // ObjC selector: `requestFullAccessToEventsWithCompletion:` — Apple's
    // ObjC interface adds the `WithCompletion:` suffix when Swift import
    // lifts the completion-handler label into the name.
    let sel = NSSelectorFromString("requestFullAccessToEventsWithCompletion:")
    if store.responds(to: sel) {
        typealias Block = @convention(block) (Bool, Error?) -> Void
        let cb: Block = { ok, _ in
            granted = ok
            semaphore.signal()
        }
        let method = store.method(for: sel)
        typealias CMethod = @convention(c) (AnyObject, Selector, Block) -> Void
        let fn = unsafeBitCast(method, to: CMethod.self)
        fn(store, sel, cb)
    } else {
        store.requestAccess(to: .event) { ok, _ in
            granted = ok
            semaphore.signal()
        }
    }

    _ = semaphore.wait(timeout: .now() + .seconds(15))
    return granted
}

// ---------------------------------------------------------------------------
// Date helpers
// ---------------------------------------------------------------------------

let isoFormatter: ISO8601DateFormatter = {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return f
}()

let dayFormatter: DateFormatter = {
    let f = DateFormatter()
    f.dateFormat = "yyyy-MM-dd"
    f.timeZone = TimeZone.current
    return f
}()

// ---------------------------------------------------------------------------
// Meeting-URL sniffing — kept simple and parallel to the Rust `extract_meeting_url`
// ---------------------------------------------------------------------------

let meetingDomainPatterns: [String] = [
    "teams.microsoft.com",
    ".zoom.us",
    "zoom.us",
    "meet.google.com",
    "chime.aws",
    ".webex.com",
]

func extractMeetingURL(location: String?, notes: String?) -> String? {
    let haystacks = [location ?? "", notes ?? ""]
    for text in haystacks {
        for line in text.components(separatedBy: .newlines) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard let start = trimmed.range(of: "https://") else { continue }
            let rest = String(trimmed[start.lowerBound...])
            // cut at first whitespace or quote
            let stopSet = CharacterSet(charactersIn: " \"'<>)")
            var candidate = rest
            if let stop = rest.rangeOfCharacter(from: stopSet) {
                candidate = String(rest[..<stop.lowerBound])
            }
            if candidate.count < 16 { continue }
            let afterScheme = String(candidate.dropFirst("https://".count))
            for pattern in meetingDomainPatterns {
                if afterScheme.hasPrefix(pattern)
                    || afterScheme.contains("\(pattern)/")
                    || (pattern.hasPrefix(".") && afterScheme
                        .split(separator: "/")
                        .first
                        .map { String($0).hasSuffix(pattern) } == true)
                {
                    return candidate
                }
            }
        }
    }
    return nil
}

// ---------------------------------------------------------------------------
// EventKit → JSON
// ---------------------------------------------------------------------------

func serialize(_ event: EKEvent) -> [String: Any] {
    let startIso = isoFormatter.string(from: event.startDate)
    let duration = max(0, Int(event.endDate.timeIntervalSince(event.startDate) / 60))
    let allDay = event.isAllDay

    var dict: [String: Any] = [
        "id": event.eventIdentifier ?? "eventkit:\(startIso):\(event.title ?? "")",
        "subject": event.title ?? "",
        "location": event.location ?? "",
        "organizer": event.organizer?.name ?? "",
        "start_time": startIso,
        "duration_minutes": duration,
        "all_day": allDay,
    ]

    if let url = event.url?.absoluteString {
        dict["online_url"] = url
    } else if let inferred = extractMeetingURL(location: event.location, notes: event.notes) {
        dict["online_url"] = inferred
    }
    return dict
}

func fetchEvents(store: EKEventStore, from start: Date, to end: Date) -> [[String: Any]] {
    let calendars = store.calendars(for: .event)
    let predicate = store.predicateForEvents(withStart: start, end: end, calendars: calendars)
    let events = store.events(matching: predicate)
    // Sort by start time ascending.
    return events
        .sorted { $0.startDate < $1.startDate }
        .map { serialize($0) }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

let argv = CommandLine.arguments
guard let mode = parseMode(argv) else {
    emitError("usage: kage-calendar-helper {upcoming <hours> | date <yyyy-MM-dd>}")
}

let store = EKEventStore()
guard requestAccess(store: store) else {
    emitError("calendar access denied or timed out — grant Calendar permission in System Settings")
}

let (start, end): (Date, Date)
switch mode {
case .upcoming(let hours):
    let now = Date()
    start = now
    end = now.addingTimeInterval(TimeInterval(hours) * 3600)
case .date(let iso):
    guard let d = dayFormatter.date(from: iso) else {
        emitError("invalid date '\(iso)' — expected yyyy-MM-dd")
    }
    start = d
    end = d.addingTimeInterval(24 * 3600)
}

let events = fetchEvents(store: store, from: start, to: end)
emit(events)
