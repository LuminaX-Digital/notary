// notary-mac: native helper for Notary. Permission status, calendar events, call detection
// and the accessibility tree of call windows. Output is JSON on stdout only.
// Usage: see README.md in this folder.

import AppKit
import ApplicationServices
import AVFoundation
import CoreAudio
import CoreGraphics
import EventKit
import Foundation

// MARK: - Output

func jsonString(_ obj: Any, pretty: Bool = false) -> String {
    let options: JSONSerialization.WritingOptions = pretty ? [.prettyPrinted, .sortedKeys] : [.sortedKeys]
    guard JSONSerialization.isValidJSONObject(obj),
          let data = try? JSONSerialization.data(withJSONObject: obj, options: options),
          let text = String(data: data, encoding: .utf8) else { return "{\"error\":\"encoding-failed\"}" }
    return text
}

/// Prints one JSON value. Returns false when stdout is gone (the reader quit).
@discardableResult
func emit(_ obj: Any, pretty: Bool = false) -> Bool {
    let ok = fputs(jsonString(obj, pretty: pretty) + "\n", stdout) >= 0
    fflush(stdout)
    return ok
}

func fail(_ code: String, exitCode: Int32 = 1) -> Never {
    emit(["error": code])
    exit(exitCode)
}

// MARK: - Permission status

func microphoneStatus() -> String {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: return "authorized"
    case .denied: return "denied"
    case .restricted: return "restricted"
    case .notDetermined: return "notDetermined"
    @unknown default: return "notDetermined"
    }
}

func calendarStatus() -> String {
    switch EKEventStore.authorizationStatus(for: .event) {
    case .fullAccess: return "fullAccess"
    case .writeOnly: return "writeOnly"
    case .denied: return "denied"
    case .restricted: return "restricted"
    case .notDetermined: return "notDetermined"
    @unknown default: return "notDetermined"
    }
}

func statusObject() -> [String: Any] {
    [
        "microphone": microphoneStatus(),
        "screen": CGPreflightScreenCaptureAccess(),
        "accessibility": AXIsProcessTrusted(),
        "calendar": calendarStatus(),
    ]
}

func requestPermission(_ kind: String) {
    switch kind {
    case "microphone":
        let sem = DispatchSemaphore(value: 0)
        AVCaptureDevice.requestAccess(for: .audio) { _ in sem.signal() }
        sem.wait()
    case "calendar":
        let sem = DispatchSemaphore(value: 0)
        EKEventStore().requestFullAccessToEvents { _, _ in sem.signal() }
        sem.wait()
    case "accessibility":
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        _ = AXIsProcessTrustedWithOptions([key: true] as CFDictionary)
    case "screen":
        _ = CGRequestScreenCaptureAccess()
    default:
        fail("unknown-permission", exitCode: 2)
    }
    emit(statusObject())
}

// MARK: - Calendar

func isoFormatter() -> ISO8601DateFormatter {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime]
    f.timeZone = TimeZone.current
    return f
}

func parseISO(_ text: String) -> Date? {
    let plain = ISO8601DateFormatter()
    plain.formatOptions = [.withInternetDateTime]
    if let d = plain.date(from: text) { return d }
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return fractional.date(from: text)
}

func requireCalendarAccess() {
    if EKEventStore.authorizationStatus(for: .event) != .fullAccess { fail("calendar-not-authorized") }
}

func listCalendars() {
    requireCalendarAccess()
    let store = EKEventStore()
    let out: [[String: Any]] = store.calendars(for: .event).map {
        ["id": $0.calendarIdentifier, "title": $0.title, "source": $0.source?.title ?? ""]
    }
    emit(out)
}

func mailAddress(_ participant: EKParticipant) -> String {
    let raw = participant.url.absoluteString
    if raw.lowercased().hasPrefix("mailto:") { return String(raw.dropFirst("mailto:".count)).removingPercentEncoding ?? "" }
    return ""
}

func listEvents(_ args: [String]) {
    guard args.count >= 2, let from = parseISO(args[0]), let to = parseISO(args[1]), from <= to else { fail("bad-date", exitCode: 2) }
    var calendarIds: Set<String>? = nil
    var fullRange = false
    var i = 2
    while i < args.count {
        if args[i] == "--full-range" { fullRange = true }
        else if args[i] == "--calendars", i + 1 < args.count {
            calendarIds = Set(args[i + 1].split(separator: ",").map(String.init))
            i += 1
        }
        i += 1
    }
    requireCalendarAccess()
    let store = EKEventStore()
    store.refreshSourcesIfNecessary()
    var calendars: [EKCalendar]? = nil
    if let ids = calendarIds { calendars = store.calendars(for: .event).filter { ids.contains($0.calendarIdentifier) } }

    // EventKit answers at most about four years per query, so long ranges are split.
    var found: [EKEvent] = []
    var cursor = from
    let step: TimeInterval = 3 * 365 * 24 * 3600
    while cursor < to {
        let end = min(cursor.addingTimeInterval(step), to)
        found += store.events(matching: store.predicateForEvents(withStart: cursor, end: end, calendars: calendars))
        cursor = end
    }

    let fmt = isoFormatter()
    var seen = Set<String>()
    var rows: [(Date, [String: Any])] = []
    for e in found {
        if e.isAllDay || e.status == .canceled { continue }
        let start = fmt.string(from: e.startDate)
        let id = (e.eventIdentifier ?? "") + ":" + start
        if !seen.insert(id).inserted { continue }
        var me: String? = nil
        var declined = false
        var attendees: [[String: String]] = []
        for p in e.attendees ?? [] {
            if p.isCurrentUser {
                let mail = mailAddress(p)
                if !mail.isEmpty { me = mail }
                if p.participantStatus == .declined { declined = true }
                continue
            }
            if attendees.count < 50 { attendees.append(["name": p.name ?? "", "email": mailAddress(p)]) }
        }
        let row: [String: Any] = [
            "id": id,
            "title": e.title ?? "",
            "start": start,
            "end": fmt.string(from: e.endDate),
            "calendar": e.calendar.title,
            "calendarId": e.calendar.calendarIdentifier,
            "calendarSource": e.calendar.source?.title ?? "",
            "externalId": e.calendarItemExternalIdentifier ?? "",
            "location": e.location ?? "",
            "notes": String((e.notes ?? "").prefix(2000)),
            "url": e.url?.absoluteString ?? "",
            "me": me as Any? ?? NSNull(),
            "declined": declined,
            "attendees": attendees,
        ]
        rows.append((e.startDate, row))
    }
    rows.sort { $0.0 < $1.0 }
    let limited = fullRange ? rows : Array(rows.prefix(200))
    emit(limited.map { $0.1 })
}

// MARK: - Apps and microphone

func appRow(_ app: NSRunningApplication) -> [String: Any] {
    ["bundleId": app.bundleIdentifier ?? "", "name": app.localizedName ?? app.bundleIdentifier ?? "", "pid": Int(app.processIdentifier)]
}

func listRunningApps() {
    let apps = NSWorkspace.shared.runningApplications.filter { $0.activationPolicy == .regular && $0.bundleIdentifier != nil }
    emit(apps.map(appRow))
}

func audioInt32(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> UInt32? {
    var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var data: UInt32 = 0
    var size = UInt32(MemoryLayout<UInt32>.size)
    return AudioObjectGetPropertyData(object, &address, 0, nil, &size, &data) == noErr ? data : nil
}

func audioBundleID(_ object: AudioObjectID) -> String {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioProcessPropertyBundleID, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var ref: Unmanaged<CFString>? = nil
    var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(object, &address, 0, nil, &size, &ref) == noErr, let value = ref else { return "" }
    return value.takeRetainedValue() as String
}

func listMicApps() {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyProcessObjectList,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr else { emit([[String: Any]]()); return }
    var ids = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
    guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &ids) == noErr else { emit([[String: Any]]()); return }

    let myPid = ProcessInfo.processInfo.processIdentifier
    var out: [[String: Any]] = []
    var seen = Set<pid_t>()
    for object in ids {
        guard let running = audioInt32(object, kAudioProcessPropertyIsRunningInput), running != 0,
              let raw = audioInt32(object, kAudioProcessPropertyPID), case let pid = pid_t(bitPattern: raw), pid != myPid,
              seen.insert(pid).inserted else { continue }
        let bundle = audioBundleID(object)
        let name = NSRunningApplication(processIdentifier: pid)?.localizedName ?? bundle
        out.append(["bundleId": bundle, "name": name, "pid": Int(pid)])
    }
    emit(out)
}

// MARK: - Accessibility tree

func axAttribute(_ el: AXUIElement, _ name: String) -> CFTypeRef? {
    var out: CFTypeRef?
    return AXUIElementCopyAttributeValue(el, name as CFString, &out) == .success ? out : nil
}

func axString(_ el: AXUIElement, _ name: String) -> String {
    guard let v = axAttribute(el, name) else { return "" }
    if let s = v as? String { return s }
    if CFGetTypeID(v) == CFBooleanGetTypeID() || CFGetTypeID(v) == CFNumberGetTypeID() { return (v as? NSNumber)?.stringValue ?? "" }
    return ""
}

func axChildren(_ el: AXUIElement) -> [AXUIElement] {
    guard let v = axAttribute(el, kAXChildrenAttribute as String), let list = v as? [AnyObject] else { return [] }
    return list.compactMap { CFGetTypeID($0) == AXUIElementGetTypeID() ? ($0 as! AXUIElement) : nil }
}

func axSize(_ el: AXUIElement) -> [Double]? {
    guard let v = axAttribute(el, kAXSizeAttribute as String), CFGetTypeID(v) == AXValueGetTypeID() else { return nil }
    var size = CGSize.zero
    return AXValueGetValue(v as! AXValue, .cgSize, &size) ? [Double(size.width), Double(size.height)] : nil
}

let textRoles: Set<String> = ["AXTextField", "AXTextArea", "AXSecureTextField", "AXComboBox", "AXSearchField"]

/// Copies an element and its descendants, bounded by depth and by a shared element budget.
func tree(_ el: AXUIElement, depth: Int, maxDepth: Int, budget: inout Int) -> [String: Any] {
    budget -= 1
    let role = axString(el, kAXRoleAttribute as String)
    var node: [String: Any] = [
        "role": role,
        "subrole": axString(el, kAXSubroleAttribute as String),
        "description": axString(el, kAXDescriptionAttribute as String),
        "title": axString(el, kAXTitleAttribute as String),
        "value": textRoles.contains(role) ? "" : axString(el, kAXValueAttribute as String),
        "identifier": axString(el, kAXIdentifierAttribute as String),
        "domClasses": (axAttribute(el, "AXDOMClassList") as? [String]) ?? [],
    ]
    if role == "AXTabGroup", let size = axSize(el) { node["size"] = size }
    var kids: [[String: Any]] = []
    if depth < maxDepth && budget > 0 {
        for child in axChildren(el) {
            if budget <= 0 { break }
            kids.append(tree(child, depth: depth + 1, maxDepth: maxDepth, budget: &budget))
        }
    }
    node["children"] = kids
    return node
}

enum Family { case zoom, teams, webex, browser, other }

func family(of bundle: String) -> Family {
    switch bundle {
    case "us.zoom.xos": return .zoom
    case "com.microsoft.teams", "com.microsoft.teams2": return .teams
    case "Cisco-Systems.Spark", "com.cisco.webexmeetingsapp": return .webex
    case "com.google.Chrome", "company.thebrowser.Browser", "com.apple.Safari", "com.microsoft.edgemac", "org.mozilla.firefox", "com.brave.Browser":
        return .browser
    default: return .other
    }
}

let chromiumBundles: Set<String> = ["com.google.Chrome", "company.thebrowser.Browser", "com.microsoft.edgemac", "com.brave.Browser"]

func limits(for family: Family) -> (depth: Int, budget: Int) { family == .zoom ? (14, 1500) : (40, 4000) }

func hasCallControls(_ window: AXUIElement) -> Bool {
    var queue = [window]
    var index = 0
    var visited = 0
    while index < queue.count && visited < 1500 {
        let el = queue[index]
        index += 1
        visited += 1
        let words = [axString(el, kAXTitleAttribute as String), axString(el, kAXDescriptionAttribute as String)].joined(separator: " ").lowercased()
        if words.contains("leave") || words.contains("hang up") || words.contains("mute") { return true }
        if axString(el, "AXDOMIdentifier") == "hangup-button" { return true }
        queue += axChildren(el)
    }
    return false
}

func matches(_ pattern: String, _ text: String) -> Bool {
    text.range(of: pattern, options: .regularExpression) != nil
}

func isCallWindow(_ window: AXUIElement, family: Family) -> Bool {
    let title = axString(window, kAXTitleAttribute as String)
    let id = axString(window, kAXIdentifierAttribute as String)
    switch family {
    case .zoom:
        return id.hasPrefix("zm.meeting") || title.hasPrefix("Zoom Meeting") || title.hasPrefix("Zoom Webinar") || title == "zoom floating video window"
    case .teams:
        return hasCallControls(window)
    case .webex:
        return id.contains("FloatingMediaWindow") || id.contains("PopOutWindow")
    case .browser:
        return matches("\\bMeet\\b", title) || matches("\\b(Teams|Zoom)\\b", title)
    case .other:
        return true
    }
}

func appElement(_ bundle: String) -> (AXUIElement, pid_t)? {
    guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first else { return nil }
    let element = AXUIElementCreateApplication(app.processIdentifier)
    AXUIElementSetMessagingTimeout(element, 1.0)
    return (element, app.processIdentifier)
}

func windows(of app: AXUIElement) -> [AXUIElement] {
    guard let v = axAttribute(app, kAXWindowsAttribute as String), let list = v as? [AnyObject] else { return [] }
    return list.compactMap { CFGetTypeID($0) == AXUIElementGetTypeID() ? ($0 as! AXUIElement) : nil }
}

func windowObject(_ window: AXUIElement, family: Family) -> [String: Any] {
    let (maxDepth, limit) = limits(for: family)
    var budget = limit
    return [
        "title": axString(window, kAXTitleAttribute as String),
        "id": axString(window, kAXIdentifierAttribute as String),
        "tree": tree(window, depth: 0, maxDepth: maxDepth, budget: &budget),
    ]
}

// MARK: - call-watch

var stopRequested = false

func setAccessibilityHelpers(_ app: AXUIElement, bundle: String, on: Bool) {
    let value: CFBoolean = on ? kCFBooleanTrue : kCFBooleanFalse
    let fam = family(of: bundle)
    if fam == .zoom || fam == .teams || chromiumBundles.contains(bundle) {
        AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, value)
    }
    if fam == .browser {
        AXUIElementSetAttributeValue(app, "AXEnhancedUserInterface" as CFString, value)
    }
}

func nowMillis() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }

func callWatch(_ args: [String]) {
    guard let first = args.first, let ms = Int(first), args.count >= 2 else { fail("usage", exitCode: 2) }
    let interval = max(ms, 100)
    let bundles = Array(args.dropFirst())

    // Quit when the reader closes stdin.
    Thread.detachNewThread {
        while true {
            if FileHandle.standardInput.availableData.isEmpty { break }
        }
        stopRequested = true
    }
    signal(SIGPIPE, SIG_IGN)
    for sig in [SIGINT, SIGTERM] {
        signal(sig) { _ in stopRequested = true }
    }

    var prepared: [String: (AXUIElement, pid_t)] = [:]
    func release() {
        for (bundle, entry) in prepared { setAccessibilityHelpers(entry.0, bundle: bundle, on: false) }
        prepared.removeAll()
    }

    var wait = Double(interval) / 1000
    while !stopRequested {
        let started = Date()
        if !AXIsProcessTrusted() {
            if !emit(["t": nowMillis(), "error": "not-trusted"]) { break }
        } else {
            for bundle in bundles {
                guard let (element, pid) = appElement(bundle) else {
                    prepared.removeValue(forKey: bundle)
                    continue
                }
                if prepared[bundle]?.1 != pid {
                    setAccessibilityHelpers(element, bundle: bundle, on: true)
                    prepared[bundle] = (element, pid)
                }
                let fam = family(of: bundle)
                let chosen = windows(of: element).filter { isCallWindow($0, family: fam) }
                let capped = fam == .other ? Array(chosen.prefix(3)) : chosen
                let objects = capped.map { windowObject($0, family: fam) }
                if !emit(["t": nowMillis(), "app": bundle, "windows": objects]) { stopRequested = true; break }
            }
        }
        let took = Date().timeIntervalSince(started)
        wait = max(Double(interval) / 1000, took * 2)
        var slept = 0.0
        while slept < wait && !stopRequested {
            Thread.sleep(forTimeInterval: 0.05)
            slept += 0.05
        }
    }
    release()
    exit(0)
}

// MARK: - call-titles

/// Titles of the windows that look like a call, per app, for the bundles given. Browsers only count with a call tab.
func callTitles(_ bundles: [String]) {
    var out: [[String: Any]] = []
    if AXIsProcessTrusted() {
        for bundle in bundles {
            guard let (element, _) = appElement(bundle) else { continue }
            let fam = family(of: bundle)
            let titles = windows(of: element).filter { isCallWindow($0, family: fam) }.map { axString($0, kAXTitleAttribute as String) }
            if !titles.isEmpty { out.append(["bundleId": bundle, "titles": titles]) }
        }
    }
    emit(out)
}

// MARK: - menubar

final class MenuBar: NSObject {
    let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    let toggleItem = NSMenuItem(title: "Start recording", action: #selector(toggle), keyEquivalent: "")

    override init() {
        super.init()
        let menu = NSMenu()
        let open = NSMenuItem(title: "Open Notary", action: #selector(openApp), keyEquivalent: "")
        open.target = self
        toggleItem.target = self
        let quit = NSMenuItem(title: "Quit Notary", action: #selector(quitApp), keyEquivalent: "")
        quit.target = self
        menu.addItem(open)
        menu.addItem(toggleItem)
        menu.addItem(.separator())
        menu.addItem(quit)
        item.menu = menu
        show(state: "idle")
    }

    func send(_ word: String) {
        FileHandle.standardOutput.write((word + "\n").data(using: .utf8)!)
    }

    @objc func openApp() { send("open") }
    @objc func toggle() { send("toggle") }
    @objc func quitApp() { send("quit") }

    func show(state: String) {
        let recording = state == "recording" || state == "paused"
        let image = NSImage(systemSymbolName: recording ? "record.circle.fill" : "waveform", accessibilityDescription: "Notary")
        image?.isTemplate = !recording
        item.button?.image = image
        item.button?.contentTintColor = recording ? (state == "paused" ? .systemOrange : .systemRed) : nil
        toggleItem.title = recording ? "Stop recording" : "Start recording"
    }
}

/// Shows a menu bar item. Prints "open", "toggle" or "quit" when chosen; reads "idle", "recording" or "paused" on stdin.
func menuBar() {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let bar = MenuBar()
    signal(SIGPIPE, SIG_IGN)
    Thread.detachNewThread {
        var pending = Data()
        while true {
            let chunk = FileHandle.standardInput.availableData
            if chunk.isEmpty { exit(0) }
            pending.append(chunk)
            while let nl = pending.firstIndex(of: 10) {
                let line = String(data: pending[pending.startIndex..<nl], encoding: .utf8) ?? ""
                pending.removeSubrange(pending.startIndex...nl)
                DispatchQueue.main.async { bar.show(state: line.trimmingCharacters(in: .whitespaces)) }
            }
        }
    }
    app.run()
}

// MARK: - play

/// Plays audio files together from `start` seconds; ends when they finish or when stdin closes.
func play(_ args: [String]) {
    guard args.count >= 2, let start = Double(args[0]) else { fail("usage", exitCode: 2) }
    var players: [AVAudioPlayer] = []
    for file in args.dropFirst() {
        if let p = try? AVAudioPlayer(contentsOf: URL(fileURLWithPath: file)), start < p.duration {
            p.currentTime = start
            p.prepareToPlay()
            players.append(p)
        }
    }
    if players.isEmpty { exit(0) }
    Thread.detachNewThread {
        while true { if FileHandle.standardInput.availableData.isEmpty { exit(0) } }
    }
    let at = players[0].deviceCurrentTime + 0.15
    for p in players { p.play(atTime: at) }
    let began = Date()
    while Date().timeIntervalSince(began) < 0.6 || players.contains(where: { $0.isPlaying }) {
        Thread.sleep(forTimeInterval: 0.1)
    }
    exit(0)
}

// MARK: - ax-dump

func axDump(_ args: [String]) {
    guard let bundle = args.first else { fail("usage", exitCode: 2) }
    guard let (element, _) = appElement(bundle) else { fail("app-not-running") }
    if !AXIsProcessTrusted() { fail("not-trusted") }
    setAccessibilityHelpers(element, bundle: bundle, on: true)
    defer { setAccessibilityHelpers(element, bundle: bundle, on: false) }
    let fam = family(of: bundle)
    var window: AXUIElement? = nil
    if let focused = axAttribute(element, kAXFocusedWindowAttribute as String), CFGetTypeID(focused) == AXUIElementGetTypeID() {
        window = (focused as! AXUIElement)
    } else {
        window = windows(of: element).first
    }
    guard let target = window else { fail("no-window") }
    emit(windowObject(target, family: fam), pretty: true)
}

// MARK: - Entry point

let arguments = Array(CommandLine.arguments.dropFirst())
guard let command = arguments.first else { fail("usage", exitCode: 2) }
let rest = Array(arguments.dropFirst())

switch command {
case "status": emit(statusObject())
case "request":
    guard let kind = rest.first else { fail("usage", exitCode: 2) }
    requestPermission(kind)
case "calendars": listCalendars()
case "events": listEvents(rest)
case "mic-apps": listMicApps()
case "running-apps": listRunningApps()
case "call-watch": callWatch(rest)
case "call-titles": callTitles(rest)
case "menubar": menuBar()
case "play": play(rest)
case "ax-dump": axDump(rest)
default: fail("unknown-command", exitCode: 2)
}
