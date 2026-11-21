# Notary

Private meeting notes for Mac. Notary listens to the sound your Mac plays, writes a live transcript, and keeps everything on your computer. No bot joins your call and no account is needed.

Free and open source (MIT). Written in Rust with [GPUI](https://crates.io/crates/gpui), the renderer behind the Zed editor, so it starts fast and uses no web view.

> **Early development.** You can record, watch a live transcript in English and German, and come back to your saved meetings later. Search and "ask across meetings" are not built yet. See the status list below.

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
| Search across titles, transcripts and notes | Works |
| Ask questions across all meetings, answered with sources, in English or German | Works |
| Speaker names | Planned |
| Windows and Linux | Not planned for v1 |

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

On first launch Notary shows a setup screen. It downloads two models from huggingface.co only when you press Download: speech recognition (574 MB, required) and summaries (2.5 GB, optional). Downloads can be cancelled and resumed, and each file is checked against its official SHA-256 before it is used.

Models are stored in `~/Library/Application Support/Notary/models`. To use another folder, or to install models by hand on an offline Mac, set `NOTARY_MODELS_DIR`, or point `NOTARY_MODEL` and `NOTARY_LLM` at the files. Summaries run in a separate helper program (`notary-llm`) that starts only while a summary is written.

If you use an outbound firewall such as LuLu or Little Snitch, allow Notary to connect to huggingface.co when it asks, or the download waits.

On first use macOS asks for Screen Recording permission, and for microphone access on macOS 15 or newer. Notary uses them only to hear the audio your Mac plays and your own voice; it never captures the screen. Use headphones on calls so your microphone does not pick up the other side.

## Search and ask

The box above the meeting list searches titles, transcripts, summaries and next steps; every word you type must appear in the meeting. The magnifier in the sidebar opens Ask: type a question in English or German, press Enter, and Notary finds the passages that matter, lets the local model answer from them only, and shows which meetings the answer came from. If nothing in your meetings matches, it says so instead of guessing.

The text fields are simple for now: typing, Backspace (Option for a word, Command for everything), Command-V to paste, Escape to clear. There is no cursor movement or selection yet.

## Package as an app

```bash
scripts/bundle.sh
```

This builds `dist/Notary.app` and `dist/Notary.dmg`, ad-hoc signed so it runs on your own Mac. To make a copy you can hand to other people, sign it with a Developer ID certificate and notarize it with Apple:

```bash
SIGN_ID="Developer ID Application: Your Name (TEAMID)" NOTARY_PROFILE=<profile> scripts/bundle.sh
```

The script signs the helper program first and the app second, uses the hardened runtime, signs the disk image, sends it to Apple, staples the ticket and checks the result with `spctl`. Create the notarization profile once with `xcrun notarytool store-credentials <profile>`. `scripts/make-icon.swift` redraws the app icon.

## Privacy

Audio, transcripts and the model all stay on your computer. Notary goes online only to download a model when you press Download.

## Licence

MIT, see [LICENSE](LICENSE). Third-party licences are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
