# Notary

Private meeting notes for Mac. Notary listens to the sound your Mac plays, writes a live transcript, and keeps everything on your computer. No bot joins your call and no account is needed.

Free and open source (MIT). Written in Rust with [GPUI](https://crates.io/crates/gpui), the renderer behind the Zed editor, so it starts fast and uses no web view.

> **Early development.** The core loop works: record, transcribe live in English and German, stop. Saving, microphone capture and summaries are in progress. See the status list below.

## Status

| | |
|---|---|
| Live transcript from system audio | Works |
| English and German, language detected per phrase | Works |
| Local model, no network use while recording | Works |
| Saving meetings to disk | In progress |
| Microphone (your own voice) | In progress |
| Summaries and "ask across meetings" | Planned |
| Speaker names | Planned |
| Windows and Linux | Not planned for v1 |

## Requirements

- Apple Silicon Mac, macOS 14.4 or newer
- 8 GB of memory (the transcription model needs about 700 MB while recording)
- Rust 1.85 or newer and Xcode command line tools to build from source

## Build and run

```bash
git clone <this repository>
cd notary
mkdir -p models
curl -L -o models/ggml-large-v3-turbo-q5_0.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin
cargo run --release
```

The model file is about 550 MB. Notary looks for it in `./models`, in `~/Library/Application Support/Notary/models`, or at the path in `NOTARY_MODEL`.

On first use macOS asks for Screen Recording permission. Notary uses it only to hear the audio your Mac plays; it never captures the screen.

## Privacy

Audio, transcripts and the model all stay on your computer. Notary goes online only to download a model you ask for.

## Licence

MIT, see [LICENSE](LICENSE). Third-party licences are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
