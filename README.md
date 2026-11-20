# Notary

Private meeting notes for Mac. Notary listens to the sound your Mac plays, writes a live transcript, and keeps everything on your computer. No bot joins your call and no account is needed.

Free and open source (MIT). Written in Rust with [GPUI](https://crates.io/crates/gpui), the renderer behind the Zed editor, so it starts fast and uses no web view.

> **Early development.** You can record, watch a live transcript in English and German, and come back to your saved meetings later. Search and "ask across meetings" are not built yet. See the status list below.

## Status

| | |
|---|---|
| Live transcript from system audio | Works |
| English and German, language detected per phrase | Works |
| Local model, no network use while recording | Works |
| Meetings saved on this Mac, with delete | Works |
| Microphone (your own voice, macOS 15 or newer) | Built, needs more testing |
| Summaries, titles and next steps you can tick off, in English or German | Works |
| "Ask across meetings" and search | Planned |
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
mkdir -p models
curl -L -o models/ggml-large-v3-turbo-q5_0.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin
curl -L -o models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf \
  https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf
cargo build --release
cargo run --release
```

The speech model is about 550 MB and the summary model about 2.5 GB. Notary looks for them in `./models`, in `~/Library/Application Support/Notary/models`, or at the paths in `NOTARY_MODEL` and `NOTARY_LLM`. Summaries run in a separate helper program (`notary-llm`) that starts only while a summary is written.

On first use macOS asks for Screen Recording permission, and for microphone access on macOS 15 or newer. Notary uses them only to hear the audio your Mac plays and your own voice; it never captures the screen. Use headphones on calls so your microphone does not pick up the other side.

## Package as an app

```bash
scripts/bundle.sh
```

This builds `dist/Notary.app` and `dist/Notary.dmg`, ad-hoc signed so it runs on your own Mac. Pass `SIGN_ID="Developer ID Application: ..."` to sign for distribution; the script's header lists the notarization commands.

## Privacy

Audio, transcripts and the model all stay on your computer. Notary goes online only to download a model you ask for.

## Licence

MIT, see [LICENSE](LICENSE). Third-party licences are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
