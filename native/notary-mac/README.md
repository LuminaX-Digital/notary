# notary-mac

Native helper for Notary. One executable, public Apple APIs only. Build it with `scripts/build-native.sh`
(output: `target/release/notary-mac`, arm64, macOS 14.4 or newer).

Everything on stdout is JSON. Errors are `{"error":"<code>"}` with a non-zero exit (2 for bad usage, 1 otherwise).
Nothing else is ever printed, and window titles are never logged.

## Subcommands

### `status`
`{"microphone":"authorized|denied|notDetermined|restricted","screen":bool,"accessibility":bool,"calendar":"fullAccess|writeOnly|denied|notDetermined|restricted"}`

### `request microphone|calendar|accessibility|screen`
Shows the system permission prompt, waits (microphone, calendar), then prints the new `status` object.
Accessibility and screen return immediately; the user grants them in System Settings. Unknown kind:
`{"error":"unknown-permission"}`.

### `calendars`
`[{"id","title","source"}]`. Error `calendar-not-authorized` unless status is `fullAccess`.

### `events FROM_ISO TO_ISO [--calendars id,id] [--full-range]`
Array sorted by start, capped at 200 unless `--full-range`. All-day and cancelled events are skipped.

```
{"id":"<eventIdentifier>:<startISO>","title","start","end" (ISO 8601 with offset),"calendar","calendarId",
 "calendarSource","externalId","location","notes" (first 2000 chars),"url","me": "<email>"|null,
 "declined": bool,"attendees":[{"name","email"}] (max 50, the user excluded)}
```
Errors: `calendar-not-authorized`, `bad-date` (exit 2).

### `mic-apps`
`[{"bundleId","name","pid"}]`: processes that have microphone input running right now (CoreAudio process list), without
the helper itself. Helper processes appear under their own bundle id (for example `us.zoom.CptHost`); map them to the
owning app on the Rust side. Empty array when none.

### `running-apps`
`[{"bundleId","name","pid"}]` for regular (Dock) applications.

### `call-watch MS BUNDLEID...`
Long-running. One JSON line per running app per tick (nothing for apps that are not running):

```
{"t":<ms since epoch>,"app":"<bundleId>","windows":[{"title","id","tree":<node>}]}
```
`windows` holds only call windows (may be empty). When Accessibility is not granted the tick is
`{"t":<ms>,"error":"not-trusted"}` and the helper keeps running. A tick is at least `MS` ms (floor 100) and at least
twice the time the read took. The helper exits 0 when stdin reaches EOF (or on SIGINT/SIGTERM) and first resets the
accessibility switches it set. Window choice: Zoom `us.zoom.xos` (identifier `zm.meeting…`, title "Zoom Meeting/Webinar",
"zoom floating video window"), Teams `com.microsoft.teams|teams2` (window with call controls), Webex
`Cisco-Systems.Spark|com.cisco.webexmeetingsapp` (floating or pop-out window), browsers (title contains Meet, Teams or
Zoom). Any other bundle id: up to 3 windows.

Node (every key is always present):
```
{"role","subrole","description","title","value","identifier","domClasses":[...],
 "size":[w,h]  (only on AXTabGroup),"children":[node...]}
```
Strings are empty when absent. `value` is empty for text fields. Limits: depth 14 and 1500 elements for Zoom, depth 40
and 4000 elements otherwise.

### `ax-dump BUNDLEID`
Pretty-printed `{"id","title","tree"}` of the app's focused (else first) window, for bug reports. Errors:
`app-not-running`, `not-trusted`, `no-window`.

## Wiring into the app bundle

- Sign the helper inside-out with the hardened runtime, like `notary-llm`.
- TCC attributes prompts to the responsible process, which is Notary.app, so the usage strings belong in Notary's
  `Info.plist`: `NSCalendarsFullAccessUsageDescription` (calendar) and `NSMicrophoneUsageDescription` (already there).
  Accessibility and Screen Recording have no usage string.
- Entitlement for the hardened runtime: `com.apple.security.personal-information.calendars` = true (EventKit). The
  existing `com.apple.security.device.audio-input` covers the microphone.
- `call-watch` sets `AXManualAccessibility` on Zoom, Teams and Chromium browsers and `AXEnhancedUserInterface` on
  browsers while it runs, and resets both on exit. If the process is killed with SIGKILL they stay set until the target
  app restarts.
