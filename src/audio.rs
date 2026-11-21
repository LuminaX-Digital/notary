//! Saving the audio of a meeting: two small files, one for your microphone and one for the call.
//!
//! While recording, samples go to a 16 kHz mono WAV on disk (its header is refreshed every few seconds,
//! so a crash still leaves playable audio). After the recording the WAV is compressed with macOS's own
//! `afconvert` to AAC, about 7 MB an hour, and the WAV is removed.

use std::fs::{self, File};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const SAMPLE_RATE: u32 = 16_000;
const SYNC_EVERY_SAMPLES: u64 = SAMPLE_RATE as u64 * 5;

pub struct Wav {
    file: BufWriter<File>,
    samples: u64,
    last_sync: u64,
}

fn header(data_bytes: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

impl Wav {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = BufWriter::new(File::create(path)?);
        file.write_all(&header(0))?;
        Ok(Self { file, samples: 0, last_sync: 0 })
    }

    pub fn write(&mut self, samples: &[f32]) -> std::io::Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        self.file.write_all(&bytes)?;
        self.samples += samples.len() as u64;
        if self.samples - self.last_sync >= SYNC_EVERY_SAMPLES {
            self.sync_header()?;
        }
        Ok(())
    }

    fn sync_header(&mut self) -> std::io::Result<()> {
        let data = (self.samples * 2).min(u32::MAX as u64 - 36) as u32;
        self.file.flush()?;
        let inner = self.file.get_mut();
        inner.seek(SeekFrom::Start(0))?;
        inner.write_all(&header(data))?;
        inner.seek(SeekFrom::End(0))?;
        self.last_sync = self.samples;
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        self.sync_header()
    }
}

/// Where the audio of a meeting lives.
pub fn meeting_dir(id: i64) -> PathBuf {
    crate::settings::data_dir().join("meetings").join(id.to_string())
}

pub fn has_audio(id: i64) -> bool {
    let dir = meeting_dir(id);
    dir.join("me.m4a").exists() || dir.join("them.m4a").exists()
}

/// Compresses `name.wav` to `name.m4a` and removes the WAV. Silent or empty files are dropped.
pub fn compress(dir: &Path, name: &str) {
    let wav = dir.join(format!("{name}.wav"));
    let Ok(meta) = fs::metadata(&wav) else { return };
    if meta.len() <= 44 {
        let _ = fs::remove_file(&wav);
        return;
    }
    let m4a = dir.join(format!("{name}.m4a"));
    let ok = Command::new("/usr/bin/afconvert")
        .args(["-f", "m4af", "-d", "aac", "-b", "24000"])
        .arg(&wav)
        .arg(&m4a)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        let _ = fs::remove_file(&wav);
    }
}

/// Converts any audio or video file macOS can read to a 16 kHz mono 16-bit WAV at `out`.
pub fn convert_to_wav(input: &Path, out: &Path) -> Result<(), String> {
    let result = Command::new("/usr/bin/afconvert")
        .args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"])
        .arg(input)
        .arg(out)
        .output()
        .map_err(|e| e.to_string())?;
    if result.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&result.stderr).trim().to_string())
    }
}

/// Reads the samples of a 16-bit mono WAV file as floats, finding the data chunk wherever it is.
pub fn read_wav(path: &Path) -> std::io::Result<Vec<f32>> {
    let bytes = fs::read(path)?;
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not a 16-bit WAV file");
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" {
        return Err(bad());
    }
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            let end = (pos + 8 + size).min(bytes.len());
            return Ok(bytes[pos + 8..end].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect());
        }
        pos += 8 + size + (size & 1);
    }
    Err(bad())
}

pub fn delete_audio(id: i64) {
    let _ = fs::remove_dir_all(meeting_dir(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("notary-audio-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn wav_header_matches_the_samples_written() {
        let dir = temp_dir("wav");
        let path = dir.join("a.wav");
        let mut w = Wav::create(&path).unwrap();
        w.write(&vec![0.5; 1000]).unwrap();
        w.finish().unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 2000);
        assert_eq!(bytes.len(), 44 + 2000);
        assert_eq!(i16::from_le_bytes(bytes[44..46].try_into().unwrap()), 16383);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn long_recordings_refresh_the_header_while_writing() {
        let dir = temp_dir("sync");
        let path = dir.join("a.wav");
        let mut w = Wav::create(&path).unwrap();
        w.write(&vec![0.0; SAMPLE_RATE as usize * 6]).unwrap();
        w.file.flush().unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()) >= SAMPLE_RATE * 2 * 5);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn compresses_to_a_much_smaller_aac_file() {
        let dir = temp_dir("aac");
        let mut w = Wav::create(&dir.join("me.wav")).unwrap();
        let tone: Vec<f32> = (0..SAMPLE_RATE * 5).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        w.write(&tone).unwrap();
        w.finish().unwrap();
        compress(&dir, "me");
        assert!(dir.join("me.m4a").exists() && !dir.join("me.wav").exists());
        assert!(fs::metadata(dir.join("me.m4a")).unwrap().len() < 40_000);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_recordings_leave_no_files() {
        let dir = temp_dir("empty");
        Wav::create(&dir.join("them.wav")).unwrap().finish().unwrap();
        compress(&dir, "them");
        assert!(!dir.join("them.wav").exists() && !dir.join("them.m4a").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
