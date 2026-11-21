//! Helper program that writes meeting notes from a transcript with a local language model.
//!
//! It runs as its own process because whisper.cpp and llama.cpp each bundle a copy of ggml, which
//! must not live in one executable. Usage: `notary-llm <model.gguf>`, transcript on stdin, notes on stdout.

use anyhow::{Context, Result, bail};
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::params::kv_overrides::ParamOverrideValue;
use llama_cpp_2::sampling::LlamaSampler;
use std::io::{Read, Write};
use std::num::NonZeroU32;

const MAX_NEW_TOKENS: usize = 900;
const MAX_NEW_TOKENS_ASK: usize = 400;
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

const ASK_SYSTEM_EN: &str = "You answer questions about the user's own meetings. Use only the numbered sources. After every claim, cite the source it comes from, like [1] or [2]. If the sources do not contain the answer, say so in one sentence and do not guess. Answer in at most 4 short sentences, in English.";

const ASK_SYSTEM_DE: &str = "Du beantwortest Fragen zu den eigenen Besprechungen des Nutzers. Nutze nur die nummerierten Quellen. Nenne nach jeder Aussage die Quelle, zum Beispiel [1] oder [2]. Wenn die Quellen die Antwort nicht enthalten, sage das in einem Satz und rate nicht. Antworte in h\u{f6}chstens 4 kurzen S\u{e4}tzen auf Deutsch.";

/// Turns each line of stdin into a vector that captures its meaning, one line of numbers per input line.
fn embed(model_path: &str) -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let texts: Vec<&str> = input.lines().collect();
    if texts.is_empty() {
        bail!("nothing to embed");
    }
    let mut backend = LlamaBackend::init()?;
    if std::env::var("NOTARY_LLM_LOGS").is_err() {
        backend.void_logs();
    }
    // The published file lacks this value, which newer llama.cpp requires for BERT models.
    let mut params = std::pin::pin!(LlamaModelParams::default().with_n_gpu_layers(999));
    params.as_mut().append_kv_override(c"tokenizer.ggml.token_type_count", ParamOverrideValue::Int(2));
    let model = LlamaModel::load_from_file(&backend, model_path, &params).context("could not load the embedding model")?;
    let vocab = model.vocab();
    let mut ctx = model
        .new_context(
            &backend,
            LlamaContextParams::default()
                .with_embeddings(true)
                .with_pooling_type(LlamaPoolingType::Mean)
                .with_n_ctx(NonZeroU32::new(512))
                .with_n_batch(512)
                .with_n_ubatch(512),
        )
        .context("could not create the embedding context")?;
    let mut batch = LlamaBatch::new(512, 1);
    let mut out = String::new();
    for text in texts {
        let mut tokens = vocab.tokenize(text.as_bytes(), true, false);
        tokens.truncate(512);
        batch.clear();
        batch.add_sequence(&tokens, 0, false)?;
        ctx.clear_kv_cache();
        ctx.decode(&mut batch)?;
        let v = ctx.embeddings_seq_ith(0)?;
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        let line: Vec<String> = v.iter().map(|x| format!("{:.5}", x / norm)).collect();
        out.push_str(&line.join(" "));
        out.push('\n');
    }
    std::io::stdout().write_all(out.as_bytes())?;
    Ok(())
}

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
    let mode = std::env::args().nth(3).unwrap_or_default();
    if mode == "embed" {
        return embed(&model_path);
    }
    let ask = mode == "ask";
    let max_new = if ask { MAX_NEW_TOKENS_ASK } else { MAX_NEW_TOKENS };
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
    let prompt = if ask {
        let system = if german { ASK_SYSTEM_DE } else { ASK_SYSTEM_EN };
        format!("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{transcript}<|im_end|>\n<|im_start|>assistant\n")
    } else {
        format!(
            "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{label}:\n{}\n\n{format}<|im_end|>\n<|im_start|>assistant\n",
            shorten(&transcript)
        )
    };

    let mut backend = LlamaBackend::init()?;
    if std::env::var("NOTARY_LLM_LOGS").is_err() {
        backend.void_logs();
    }
    let model = LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default().with_n_gpu_layers(999))
        .context("could not load the language model")?;
    let vocab = model.vocab();
    let tokens = vocab.tokenize(prompt.as_bytes(), vocab.should_add_bos(), true);

    let n_ctx = (tokens.len() + max_new + 64) as u32;
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
    for _ in 0..max_new {
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
