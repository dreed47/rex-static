//! Mic capture and playback through PipeWire's `pw-cat`.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const PW_CAT: &str = "/usr/bin/pw-cat";
pub const MIC_RATE: u32 = 16_000;
/// Hard cap on one recording: 60 s of 16 kHz mono s16.
const MAX_REC_BYTES: usize = MIC_RATE as usize * 2 * 60;

pub struct Recorder {
    child: Child,
    reader: Option<JoinHandle<Vec<u8>>>,
}

impl Recorder {
    /// Start recording. `level` gets the live mic level (f32 bits, 0..1).
    pub fn start(level: Arc<AtomicU32>) -> std::io::Result<Self> {
        let mut child = Command::new(PW_CAT)
            .args(["--record", "--raw", "--rate", "16000", "--channels", "1", "--format", "s16", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut out = child.stdout.take().expect("piped stdout");
        let reader = thread::spawn(move || {
            let mut bytes = Vec::with_capacity(MIC_RATE as usize * 2 * 10);
            let mut buf = [0u8; 3200];
            loop {
                match out.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = &buf[..n & !1];
                        let sum: f64 = chunk
                            .chunks_exact(2)
                            .map(|b| {
                                let s = i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.0;
                                s * s
                            })
                            .sum();
                        let rms = (sum / (chunk.len() / 2).max(1) as f64).sqrt() as f32;
                        level.store((rms * 6.0).min(1.0).to_bits(), Ordering::Relaxed);
                        if bytes.len() + n <= MAX_REC_BYTES {
                            bytes.extend_from_slice(&buf[..n]);
                        }
                    }
                }
            }
            level.store(0f32.to_bits(), Ordering::Relaxed);
            bytes
        });
        Ok(Self { child, reader: Some(reader) })
    }

    pub fn stop(mut self) -> Vec<i16> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let bytes = self.reader.take().and_then(|r| r.join().ok()).unwrap_or_default();
        bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Play mono s16 PCM, blocking until done or `cancel` is set.
/// `on_start` runs right after the player process starts.
pub fn play(samples: &[i16], rate: u32, cancel: &AtomicBool, on_start: impl FnOnce()) -> std::io::Result<()> {
    let rate = rate.to_string();
    let mut child = Command::new(PW_CAT)
        .args(["--playback", "--raw", "--rate", &rate, "--channels", "1", "--format", "s16", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    on_start();
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    for chunk in bytes.chunks(2048) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if stdin.write_all(chunk).is_err() {
            break;
        }
    }
    drop(stdin);
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        if child.try_wait()?.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
