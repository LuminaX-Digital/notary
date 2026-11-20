# Third-party notices

Notary is MIT licensed. It builds on the following open-source projects, each under its own licence.

| Project | Used for | Licence |
|---|---|---|
| [GPUI](https://crates.io/crates/gpui) (Zed Industries) | Window and rendering | Apache-2.0 |
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) via [whisper-rs](https://crates.io/crates/whisper-rs) | Speech to text | MIT (whisper.cpp), Unlicense (whisper-rs) |
| [llama.cpp](https://github.com/ggml-org/llama.cpp) via [llama-cpp-2](https://crates.io/crates/llama-cpp-2) | Summaries | MIT (llama.cpp), MIT OR Apache-2.0 (llama-cpp-2) |
| [screencapturekit](https://crates.io/crates/screencapturekit) | System audio capture on macOS | MIT OR Apache-2.0 |
| [anyhow](https://crates.io/crates/anyhow), [futures](https://crates.io/crates/futures), [chrono](https://crates.io/crates/chrono) | Utilities | MIT OR Apache-2.0 |

Models are downloaded separately. Summaries use Qwen3-4B-Instruct-2507 by Alibaba Cloud (Apache-2.0), in the GGUF format published by Unsloth.

Speech models are downloaded separately. The Whisper model weights are released by OpenAI under the MIT licence and converted to the ggml format by the whisper.cpp project.

Run `cargo metadata` for the complete list of transitive dependencies and their licences.
