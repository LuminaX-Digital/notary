//! Records system audio with ScreenCaptureKit and transcribes it with whisper on a background thread.

use anyhow::{Context, Result};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use screencapturekit::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const SAMPLE_RATE: usize = 16_000;
const MIN_CHUNK: usize = 3 * SAMPLE_RATE;
const MAX_CHUNK: usize = 10 * SAMPLE_RATE;
const PAUSE: usize = SAMPLE_RATE * 4 / 10;
const SILENCE_RMS: f32 = 0.003;

pub enum Event {
    Loading,
    Listening,
    Segment { start: f32, text: String },
    Failed(String),
    Stopped,
}

pub struct Recording {
    stop: Arc<AtomicBool>,
}

impl Recording {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn model_path() -> Option<PathBuf> {
    const FILE: &str = "ggml-large-v3-turbo-q5_0.bin";
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("NOTARY_MODEL") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("models").join(FILE));
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join("Library/Application Support/Notary/models").join(FILE));
    }
    candidates.into_iter().find(|p| p.exists())
}

pub fn start(model: PathBuf) -> (Recording, UnboundedReceiver<Event>) {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = unbounded();
    let flag = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = run(model, flag, &tx) {
            let _ = tx.unbounded_send(Event::Failed(format!("{e:#}")));
        }
        let _ = tx.unbounded_send(Event::Stopped);
    });
    (Recording { stop }, rx)
}

struct AudioTap {
    tx: mpsc::Sender<Vec<f32>>,
}

impl SCStreamOutputTrait for AudioTap {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        if !matches!(output_type, SCStreamOutputType::Audio) {
            return;
        }
        let Ok(list) = sample.audio_buffer_list() else {
            return;
        };
        for buffer in list.iter() {
            let samples: Vec<f32> = buffer
                .data()
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            if !samples.is_empty() {
                let _ = self.tx.send(samples);
            }
        }
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Cuts the stream at a pause once at least 3s are buffered, or at 10s at the latest.
fn next_chunk(buf: &mut Vec<f32>, flush: bool) -> Option<Vec<f32>> {
    if buf.is_empty() {
        return None;
    }
    let ready = buf.len() >= MAX_CHUNK
        || (buf.len() >= MIN_CHUNK && rms(&buf[buf.len() - PAUSE..]) < SILENCE_RMS)
        || flush;
    if !ready {
        return None;
    }
    let take = buf.len().min(MAX_CHUNK);
    Some(buf.drain(..take).collect())
}

fn run(model: PathBuf, stop: Arc<AtomicBool>, events: &UnboundedSender<Event>) -> Result<()> {
    let _ = events.unbounded_send(Event::Loading);
    let ctx = WhisperContext::new_with_params(
        model.to_str().context("model path is not valid UTF-8")?,
        WhisperContextParameters::default(),
    )
    .context("could not load the whisper model")?;
    let mut state = ctx.create_state()?;

    let content = SCShareableContent::get().map_err(|e| {
        anyhow::anyhow!("{e:?}. Allow Screen Recording for Notary in System Settings > Privacy & Security.")
    })?;
    let display = content.displays().into_iter().next().context("no display found")?;
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build()?;
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_captures_audio(true)
        .with_sample_rate(SAMPLE_RATE as i32)
        .with_channel_count(1);

    let (audio_tx, audio_rx) = mpsc::channel();
    let mut stream = SCStream::new(&filter, &config)?;
    stream.add_output_handler(AudioTap { tx: audio_tx }, SCStreamOutputType::Audio)?;
    stream.start_capture()?;
    let _ = events.unbounded_send(Event::Listening);

    let mut pending: Vec<f32> = Vec::new();
    let mut elapsed = 0.0_f32;
    let mut stopping = false;

    loop {
        if !stopping {
            match audio_rx.recv_timeout(Duration::from_millis(150)) {
                Ok(samples) => pending.extend(samples),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if stop.load(Ordering::Relaxed) {
                stopping = true;
                let _ = stream.stop_capture();
                pending.extend(audio_rx.try_iter().flatten());
            }
        }

        while let Some(chunk) = next_chunk(&mut pending, stopping) {
            let start = elapsed;
            elapsed += chunk.len() as f32 / SAMPLE_RATE as f32;
            if rms(&chunk) < SILENCE_RMS {
                continue;
            }
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_language(Some("auto"));
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_no_context(true);
            params.set_suppress_blank(true);
            state.full(params, &chunk)?;
            for segment in state.as_iter() {
                let text = segment.to_string();
                let text = text.trim();
                if !text.is_empty() {
                    let _ = events.unbounded_send(Event::Segment {
                        start: start + segment.start_timestamp() as f32 / 100.0,
                        text: text.to_string(),
                    });
                }
            }
        }

        if stopping {
            break;
        }
    }
    Ok(())
}
