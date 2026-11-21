//! The two models Notary needs: where they live, and a resumable, verified download.

use anyhow::{Context, Result, bail};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Speech,
    Summary,
}

pub const ALL: [Kind; 2] = [Kind::Speech, Kind::Summary];

pub struct Spec {
    pub name: &'static str,
    pub blurb: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    env: &'static str,
}

const SPEECH: Spec = Spec {
    name: "Speech recognition",
    blurb: "Whisper large-v3-turbo. Turns what you hear and say into text, in English, German and many other languages. Needed to record.",
    file: "ggml-large-v3-turbo-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
    size: 574_041_195,
    sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    env: "NOTARY_MODEL",
};

const SUMMARY: Spec = Spec {
    name: "Summaries",
    blurb: "Qwen3 4B Instruct. Writes a title, summary, decisions and next steps after a recording. Optional.",
    file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    size: 2_497_281_120,
    sha256: "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597",
    env: "NOTARY_LLM",
};

pub fn spec(kind: Kind) -> &'static Spec {
    match kind {
        Kind::Speech => &SPEECH,
        Kind::Summary => &SUMMARY,
    }
}

/// Where downloaded models are stored.
pub fn dir() -> PathBuf {
    if let Ok(p) = std::env::var("NOTARY_MODELS_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/Notary/models")
}

/// The installed model file, if there is one: an explicit path, the models folder, or ./models.
pub fn find(kind: Kind) -> Option<PathBuf> {
    let s = spec(kind);
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var(s.env) {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(dir().join(s.file));
    candidates.push(PathBuf::from("models").join(s.file));
    candidates.into_iter().find(|p| p.is_file())
}

pub enum Progress {
    Bytes { done: u64, total: u64 },
    Verifying,
    Done,
    Failed(String),
    Cancelled,
}

pub struct Download {
    cancel: Arc<AtomicBool>,
}

impl Download {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn start(kind: Kind) -> (Download, UnboundedReceiver<Progress>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = unbounded();
    let flag = cancel.clone();
    std::thread::spawn(move || {
        match run(spec(kind), &flag, &tx) {
            Ok(()) => {}
            Err(e) => {
                let _ = tx.unbounded_send(Progress::Failed(format!("{e:#}")));
            }
        }
    });
    (Download { cancel }, rx)
}

fn disk_error(e: std::io::Error) -> anyhow::Error {
    if e.kind() == ErrorKind::StorageFull {
        anyhow::anyhow!("There is not enough free disk space for this download")
    } else {
        anyhow::Error::new(e).context("could not write the model file")
    }
}

fn sha256_of(path: &PathBuf) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// True when the file has exactly the official size and SHA-256.
fn matches_official(path: &PathBuf, size: u64, sha256: &str) -> Result<bool> {
    Ok(path.metadata()?.len() == size && sha256_of(path)? == sha256)
}

fn run(spec: &Spec, cancel: &AtomicBool, tx: &UnboundedSender<Progress>) -> Result<()> {
    let dir = dir();
    fs::create_dir_all(&dir).context("could not create the models folder")?;
    let target = dir.join(spec.file);
    let part = dir.join(format!("{}.part", spec.file));
    if target.is_file() {
        let _ = tx.unbounded_send(Progress::Done);
        return Ok(());
    }

    let mut have = part.metadata().map(|m| m.len()).unwrap_or(0);
    if have > spec.size {
        fs::remove_file(&part)?;
        have = 0;
    }

    if have < spec.size {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(None)
                .build(),
        );
        let mut request = agent.get(spec.url);
        if have > 0 {
            request = request.header("Range", format!("bytes={have}-"));
        }
        let response = request
            .call()
            .context("could not reach huggingface.co. Check your internet connection and try again")?;
        let (mut file, mut done) = match response.status().as_u16() {
            206 => (OpenOptions::new().append(true).open(&part)?, have),
            200 => (File::create(&part).map_err(disk_error)?, 0),
            code => bail!("huggingface.co answered with status {code}"),
        };

        let mut reader = response.into_body().into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut last_report = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                file.flush()?;
                let _ = tx.unbounded_send(Progress::Cancelled);
                return Ok(());
            }
            let n = reader.read(&mut buf).context("the connection was interrupted")?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(disk_error)?;
            done += n as u64;
            if last_report.elapsed() >= Duration::from_millis(150) {
                let _ = tx.unbounded_send(Progress::Bytes { done, total: spec.size });
                last_report = Instant::now();
            }
        }
        file.flush()?;
        file.sync_all()?;
    }

    let _ = tx.unbounded_send(Progress::Verifying);
    if !matches_official(&part, spec.size, spec.sha256)? {
        let _ = fs::remove_file(&part);
        bail!("The download did not match the official file and was removed. Press Retry");
    }
    fs::rename(&part, &target)?;
    let _ = tx.unbounded_send(Progress::Done);
    Ok(())
}

pub fn format_bytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1e6;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{mb:.0} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn temp_file(name: &str, content: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("notary-test-{}-{name}", std::process::id()));
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn accepts_a_file_with_the_official_size_and_hash() {
        let path = temp_file("ok", b"abc");
        assert!(matches_official(&path, 3, ABC_SHA256).unwrap());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_corrupted_or_truncated_files() {
        let corrupted = temp_file("corrupt", b"abd");
        assert!(!matches_official(&corrupted, 3, ABC_SHA256).unwrap());
        let truncated = temp_file("short", b"ab");
        assert!(!matches_official(&truncated, 3, ABC_SHA256).unwrap());
        fs::remove_file(corrupted).unwrap();
        fs::remove_file(truncated).unwrap();
    }

    #[test]
    fn official_hashes_look_like_sha256() {
        for kind in ALL {
            let s = spec(kind);
            assert_eq!(s.sha256.len(), 64);
            assert!(s.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(s.size > 100_000_000);
        }
    }

    #[test]
    fn formats_sizes_for_people() {
        assert_eq!(format_bytes(574_041_195), "574 MB");
        assert_eq!(format_bytes(2_497_281_120), "2.5 GB");
    }
}
