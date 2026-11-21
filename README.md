# Notary

Private meeting notes for Mac. Notary listens to the sound your Mac plays, writes a live transcript, and keeps everything on your computer. No bot joins your call and no account is needed.

Free and open source (MIT). Written in Rust with [GPUI](https://crates.io/crates/gpui), the renderer behind the Zed editor, so it starts fast and uses no web view.

**[Download Notary for Mac](https://github.com/LuminaX-Digital/notary/releases/latest)** (signed and notarized, Apple Silicon, macOS 14.4 or newer)

> **Version 1.0.** Record calls and meetings, get a live transcript in English and German with speaker names, summaries with next steps, search and questions across your meetings, all on your Mac. The status list says what was tested against live calls and what only on test data.

## Status

| | |
|---|---|
| Live transcript from system audio | Works |
| English and German, language detected per phrase | Works |
| Local models, no network use while recording | Works |
| Models download inside the app, with resume and a checksum check | Works |
| Meetings saved on this Mac, with delete | Works |
| Microphone (your own voice, macOS 15 or newer) | Built, needs more testing |
| Summaries, titles and next steps you can tick off, in English or German | Works |
| Search across titles, transcripts and notes, with a real text field (cursor, selection, copy and paste) | Works |
| Ask questions across all meetings, answered with sources, in English or German | Works |
| Ask finds passages by meaning as well as by words (optional 131 MB model) | Works |
| Manual update check (contacts github.com only when you press it) | Works |
| Speaker names from Zoom, Teams and Google Meet windows | Built, tested on fixtures only; needs live calls and the Accessibility permission |
| Offer to record when a call app uses the microphone, stop when the call ends | Built, tested with a fake clock; needs live calls |
| Audio kept per meeting (small AAC files), copy, export, folders, rename, notes | Works |
| Trash with 30 days to restore, ZIP backup | Works |
| Calendar: coming up, titles and attendees from the matching event | Built; needs the Calendar permission to try |
| Your name, spoken language, summary language, custom words, permissions | Works |
| Playback from any line, pause and resume, find in a transcript | Works |
| Import an audio or video file and transcribe it | Works |
| Menu bar item to open Notary and start or stop recording | Works |
| MCP server for AI apps (read only, off by default): `Notary mcp` | Works |
| Regenerate a summary with your own instruction | Works |
| First-run setup with permissions | Works |
| Windows and Linux | Not planned |

## Requirements

- Apple Silicon Mac, macOS 14.4 or newer
- 8 GB of memory (about 700 MB while recording, about 3 GB while a summary is written)
- Rust 1.85 or newer and Xcode command line tools to build from source

## Build and run

```bash
git clone <this repository>
cd notary
cargo run --release
```

On first launch Notary shows a setup screen. It downloads up to three models from huggingface.co, each only when you press Download: speech recognition (574 MB, required), summaries (2.5 GB, optional) and search by meaning (131 MB, optional). Downloads can be cancelled and resumed, and each file is checked against its official SHA-256 before it is used.

Models are stored in `~/Library/Application Support/Notary/models`. To use another folder, or to install models by hand on an offline Mac, set `NOTARY_MODELS_DIR`, or point `NOTARY_MODEL` and `NOTARY_LLM` at the files. Summaries run in a separate helper program (`notary-llm`) that starts only while a summary is written.

If you use an outbound firewall such as LuLu or Little Snitch, allow Notary to connect to huggingface.co when it asks, or the download waits.

On first use macOS asks for Screen Recording permission, and for microphone access on macOS 15 or newer. Notary uses them only to hear the audio your Mac plays and your own voice; it never captures the screen. Use headphones on calls so your microphone does not pick up the other side.

## Search and ask

The box above the meeting list searches titles, transcripts, summaries and next steps; every word you type must appear in the meeting. The magnifier in the sidebar opens Ask: type a question in English or German, press Enter, and Notary finds the passages that matter, lets the local model answer from them only, and shows which meetings the answer came from. If nothing in your meetings matches, it says so instead of guessing.

Text fields support the cursor keys, Option for word jumps, Command for line start and end, Shift to select, and Command-A, C, X and V. Escape clears the field.

With the optional "Search by meaning" model installed, Ask also finds passages that share no word with your question, so a German question about "Berechtigungen" finds an English talk about "permissions". Without it, Ask matches words only.

## Package as an app

```bash
scripts/bundle.sh
```

This builds `dist/Notary.app` and `dist/Notary.dmg`, ad-hoc signed so it runs on your own Mac. To make a copy you can hand to other people, sign it with a Developer ID certificate and notarize it with Apple:

```bash
SIGN_ID="Developer ID Application: Your Name (TEAMID)" NOTARY_PROFILE=<profile> scripts/bundle.sh
```

The script builds the native helper `notary-mac` (Swift; accessibility, calendar and microphone use), signs the helpers first and the app second, uses the hardened runtime, signs the disk image, sends it to Apple, staples the ticket and checks the result with `spctl`. Create the notarization profile once with `xcrun notarytool store-credentials <profile>`. `scripts/make-icon.swift` redraws the app icon.

## Privacy

Audio, transcripts and the model all stay on your computer. Notary goes online only when you ask it to: to download a model when you press Download, and to ask github.com for a newer version when you press Check for updates.

## Licence

MIT, see [LICENSE](LICENSE). Third-party licences are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
