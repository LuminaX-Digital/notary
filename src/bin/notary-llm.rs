//! Helper program that writes meeting notes from a transcript with a local language model.
//!
//! It runs as its own process because whisper.cpp and llama.cpp each bundle a copy of ggml, which
//! must not live in one executable. Usage: `notary-llm <model.gguf>`, transcript on stdin, notes on stdout.

use anyhow::{Context, Result, bail};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use std::io::{Read, Write};
use std::num::NonZeroU32;

const MAX_NEW_TOKENS: usize = 900;
const BATCH: usize = 2048;
const MAX_TRANSCRIPT_CHARS: usize = 48_000;
const HEAD_CHARS: usize = 12_000;

const SYSTEM: &str = "You write meeting notes from a transcript. Write in the language of the transcript, so a German transcript gets German notes. Only use what the transcript says. Never invent names, numbers, dates or decisions. Keep every item short and concrete.";

const FORMAT: &str = "Write the notes in exactly this format and keep the English marker words:

# TITLE: <a title of at most 7 words>
## SUMMARY
<2 to 4 sentences on what the meeting was about and what came out of it>
## DECISIONS
- <one line per agreed choice, deadline or condition, for example 'If X is late, Y happens instead'; only if the transcript contains any>
## NEXT STEPS
- <task> | <owner's name, or Unassigned>

Leave out the DECISIONS or NEXT STEPS section completely when there is nothing to put in it.";

const SYSTEM_DE: &str = "Du schreibst Besprechungsnotizen aus einem Transkript. Schreibe auf Deutsch. Verwende nur, was im Transkript steht. Erfinde keine Namen, Zahlen, Termine oder Entscheidungen. Halte jeden Punkt kurz und konkret.";

const FORMAT_DE: &str = "Schreibe die Notizen genau in diesem Format und lasse die englischen Schl\u{fc}sselw\u{f6}rter unver\u{e4}ndert:

# TITLE: <Titel mit h\u{f6}chstens 7 W\u{f6}rtern>
## SUMMARY
<2 bis 4 S\u{e4}tze dar\u{fc}ber, worum es ging und was herauskam>
## DECISIONS
- <eine Zeile pro vereinbarter Entscheidung, Frist oder Bedingung, zum Beispiel 'Wenn X zu sp\u{e4}t ist, passiert stattdessen Y'; nur wenn das Transkript welche enth\u{e4}lt>
## NEXT STEPS
- <Aufgabe> | <Name der verantwortlichen Person oder Unassigned>

Lass DECISIONS oder NEXT STEPS komplett weg, wenn es nichts einzutragen gibt. Schreibe niemals Platzhalter wie 'Keine'. Schreibe alles auf Deutsch.";

fn shorten(transcript: &str) -> String {
    if transcript.len() <= MAX_TRANSCRIPT_CHARS {
        return transcript.to_string();
    }
    let mut head = HEAD_CHARS;
    while !transcript.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = transcript.len() - (MAX_TRANSCRIPT_CHARS - HEAD_CHARS);
    while !transcript.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}\n[... middle of the meeting left out ...]\n{}", &transcript[..head], &transcript[tail..])
}

fn main() -> Result<()> {
    let model_path = std::env::args().nth(1).context("usage: notary-llm <model.gguf> [language]")?;
    let language = std::env::args().nth(2).unwrap_or_else(|| "the language of the transcript".into());
    let mut transcript = String::new();
    std::io::stdin().read_to_string(&mut transcript)?;
    if transcript.trim().is_empty() {
        bail!("the transcript is empty");
    }

    // Qwen chat format. A German transcript gets German instructions, because English ones pull the answer into English.
    let german = language == "German";
    let (system, label, format) = if german {
        (SYSTEM_DE, "Transkript", FORMAT_DE.to_string())
    } else {
        (SYSTEM, "Transcript", format!("{FORMAT}\n\nWrite everything in {language}. Only the marker words stay in English."))
    };
    let prompt = format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{label}:\n{}\n\n{format}<|im_end|>\n<|im_start|>assistant\n",
        shorten(&transcript)
    );

    let mut backend = LlamaBackend::init()?;
    backend.void_logs();
    let model = LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default().with_n_gpu_layers(999))
        .context("could not load the language model")?;
    let vocab = model.vocab();
    let tokens = vocab.tokenize(prompt.as_bytes(), vocab.should_add_bos(), true);

    let n_ctx = (tokens.len() + MAX_NEW_TOKENS + 64) as u32;
    let mut ctx = model
        .new_context(&backend, LlamaContextParams::default().with_n_ctx(NonZeroU32::new(n_ctx)).with_n_batch(BATCH as u32))
        .context("could not create the model context")?;

    let mut batch = LlamaBatch::new(BATCH, 1);
    let last = tokens.len() as i32 - 1;
    let mut pos = 0_i32;
    for chunk in tokens.chunks(BATCH) {
        batch.clear();
        for (i, token) in chunk.iter().enumerate() {
            let at = pos + i as i32;
            batch.add(*token, at, &[0], at == last)?;
        }
        ctx.decode(&mut batch)?;
        pos += chunk.len() as i32;
    }

    let mut sampler = LlamaSampler::chain_simple([LlamaSampler::top_p(0.9, 1), LlamaSampler::temp(0.3), LlamaSampler::dist(42)]);
    let mut output: Vec<u8> = Vec::new();
    for _ in 0..MAX_NEW_TOKENS {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        if vocab.is_eog(token) {
            break;
        }
        output.extend(vocab.token_to_piece(token, false, None));
        batch.clear();
        batch.add(token, pos, &[0], true)?;
        pos += 1;
        ctx.decode(&mut batch)?;
    }

    std::io::stdout().write_all(String::from_utf8_lossy(&output).as_bytes())?;
    Ok(())
}
