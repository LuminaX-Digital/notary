//! Records system audio and the microphone with ScreenCaptureKit and transcribes both with whisper
//! on a background thread.

use crate::audio;
use anyhow::{Context, Result};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use screencapturekit::prelude::*;
use std::collections::HashSet;
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
const NO_SPEECH_MAX: f32 = 0.6;
const ECHO_WINDOW_SECS: f32 = 8.0;
const ECHO_SIMILARITY: f32 = 0.5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    System,
    Mic,
}

pub enum Event {
    Loading,
    Listening { mic: bool },
    Segment { source: Source, start: f32, text: String },
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
    crate::models::find(crate::models::Kind::Speech)
}

/// What the user chose about speech: the whisper language code and words that help it spell.
#[derive(Clone)]
pub struct Config {
    pub language: String,
    pub prompt: String,
    /// Where to save the audio of this recording; None keeps no audio.
    pub audio_dir: Option<PathBuf>,
}

pub fn start(model: PathBuf, config: Config) -> (Recording, UnboundedReceiver<Event>) {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = unbounded();
    let flag = stop.clone();
    std::thread::spawn(move || {
        if let Err(e) = run(model, config, flag, &tx) {
            let _ = tx.unbounded_send(Event::Failed(format!("{e:#}")));
        }
        let _ = tx.unbounded_send(Event::Stopped);
    });
    (Recording { stop }, rx)
}

struct AudioTap {
    source: Source,
    tx: mpsc::Sender<(Source, Vec<f32>)>,
}

impl SCStreamOutputTrait for AudioTap {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        let expected = match self.source {
            Source::System => matches!(output_type, SCStreamOutputType::Audio),
            Source::Mic => matches!(output_type, SCStreamOutputType::Microphone),
        };
        if !expected {
            return;
        }
        let Ok(list) = sample.audio_buffer_list() else {
            return;
        };
        let rate = sample
            .format_description()
            .and_then(|f| f.audio_sample_rate())
            .unwrap_or(SAMPLE_RATE as f64);
        for buffer in list.iter() {
            let samples: Vec<f32> = buffer
                .data()
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            if !samples.is_empty() {
                let _ = self.tx.send((self.source, resample(&samples, rate)));
            }
        }
    }
}

/// Converts audio at `rate` Hz to the 16 kHz whisper expects, by linear interpolation.
fn resample(samples: &[f32], rate: f64) -> Vec<f32> {
    if (rate - SAMPLE_RATE as f64).abs() < 1.0 {
        return samples.to_vec();
    }
    let step = rate / SAMPLE_RATE as f64;
    let out_len = (samples.len() as f64 / step) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * step;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = samples[idx];
            let b = samples.get(idx + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// One stream of audio waiting to be cut into chunks at pauses.
struct Lane {
    source: Source,
    buf: Vec<f32>,
    elapsed: f32,
}

impl Lane {
    fn new(source: Source) -> Self {
        Self { source, buf: Vec::new(), elapsed: 0.0 }
    }

    /// Cuts at a pause once at least 3s are buffered, or at 10s at the latest.
    fn next_chunk(&mut self, flush: bool) -> Option<(f32, Vec<f32>)> {
        if self.buf.is_empty() {
            return None;
        }
        let ready = self.buf.len() >= MAX_CHUNK
            || (self.buf.len() >= MIN_CHUNK && rms(&self.buf[self.buf.len() - PAUSE..]) < SILENCE_RMS)
            || flush;
        if !ready {
            return None;
        }
        let take = self.buf.len().min(MAX_CHUNK);
        let chunk: Vec<f32> = self.buf.drain(..take).collect();
        let start = self.elapsed;
        self.elapsed += chunk.len() as f32 / SAMPLE_RATE as f32;
        Some((start, chunk))
    }
}

fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// True when the microphone line is most likely the speakers' sound coming back in.
fn is_echo(mic_text: &str, mic_time: f32, system: &[(f32, HashSet<String>)]) -> bool {
    let mic = words(mic_text);
    if mic.is_empty() {
        return false;
    }
    system.iter().any(|(t, sys)| {
        if (t - mic_time).abs() > ECHO_WINDOW_SECS {
            return false;
        }
        let shared = mic.intersection(sys).count() as f32;
        let all = mic.union(sys).count() as f32;
        all > 0.0 && shared / all >= ECHO_SIMILARITY
    })
}

fn base_config() -> SCStreamConfiguration {
    SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_captures_audio(true)
        .with_sample_rate(SAMPLE_RATE as i32)
        .with_channel_count(1)
}

fn run(model: PathBuf, speech: Config, stop: Arc<AtomicBool>, events: &UnboundedSender<Event>) -> Result<()> {
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

    let (config, mic_on) = match base_config().with_captures_microphone(true) {
        Ok(config) => (config, true),
        Err(_) => (base_config(), false),
    };

    let (audio_tx, audio_rx) = mpsc::channel();
    let mut stream = SCStream::new(&filter, &config)?;
    stream.add_output_handler(AudioTap { source: Source::System, tx: audio_tx.clone() }, SCStreamOutputType::Audio)?;
    if mic_on {
        stream.add_output_handler(AudioTap { source: Source::Mic, tx: audio_tx }, SCStreamOutputType::Microphone)?;
    }
    stream.start_capture()?;
    let _ = events.unbounded_send(Event::Listening { mic: mic_on });

    let mut lanes = [Lane::new(Source::System), Lane::new(Source::Mic)];
    let mut wavs: [Option<audio::Wav>; 2] = [None, None];
    if let Some(dir) = &speech.audio_dir {
        wavs[Source::System as usize] = audio::Wav::create(&dir.join("them.wav")).ok();
        wavs[Source::Mic as usize] = audio::Wav::create(&dir.join("me.wav")).ok();
    }
    let mut system_lines: Vec<(f32, HashSet<String>)> = Vec::new();
    let mut stopping = false;

    loop {
        if !stopping {
            match audio_rx.recv_timeout(Duration::from_millis(150)) {
                Ok((source, samples)) => {
                    if let Some(w) = wavs[source as usize].as_mut() {
                        let _ = w.write(&samples);
                    }
                    lanes[source as usize].buf.extend(samples)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("Audio capture stopped unexpectedly. If you changed your sound output, press Record again.")
                }
            }
            if stop.load(Ordering::Relaxed) {
                stopping = true;
                let _ = stream.stop_capture();
                for (source, samples) in audio_rx.try_iter() {
                    if let Some(w) = wavs[source as usize].as_mut() {
                        let _ = w.write(&samples);
                    }
                    lanes[source as usize].buf.extend(samples);
                }
            }
        }

        for lane in lanes.iter_mut() {
            while let Some((start, chunk)) = lane.next_chunk(stopping) {
                if rms(&chunk) < SILENCE_RMS {
                    continue;
                }
                let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
                params.set_language(Some(&speech.language));
                if !speech.prompt.is_empty() {
                    params.set_initial_prompt(&speech.prompt);
                }
                params.set_print_special(false);
                params.set_print_progress(false);
                params.set_print_realtime(false);
                params.set_print_timestamps(false);
                params.set_no_context(true);
                params.set_suppress_blank(true);
                state.full(params, &chunk)?;
                for segment in state.as_iter() {
                    if segment.no_speech_probability() > NO_SPEECH_MAX {
                        continue;
                    }
                    let text = segment.to_string();
                    let text = text.trim();
                    if !text.chars().any(char::is_alphanumeric) {
                        continue;
                    }
                    let at = start + segment.start_timestamp() as f32 / 100.0;
                    match lane.source {
                        Source::System => system_lines.push((at, words(text))),
                        Source::Mic if is_echo(text, at, &system_lines) => continue,
                        Source::Mic => {}
                    }
                    let _ = events.unbounded_send(Event::Segment {
                        source: lane.source,
                        start: at,
                        text: text.to_string(),
                    });
                }
            }
        }

        if stopping {
            break;
        }
    }
    for w in wavs.into_iter().flatten() {
        let _ = w.finish();
    }
    if let Some(dir) = &speech.audio_dir {
        audio::compress(dir, "me");
        audio::compress(dir, "them");
    }
    Ok(())
}
