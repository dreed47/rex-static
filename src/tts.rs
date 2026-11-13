//! Text to speech with Piper, then a deliberately broken-broadcast DSP pass.

use rand::Rng;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const RATE: u32 = 22_050;
/// Envelope and glitch-flag frames per second (drives the mouth).
pub const FPS: f32 = 30.0;
const MAX_TTS_BYTES: usize = RATE as usize * 2 * 120;
/// Piper length scale before the pitch shift: the overall speaking pace.
const PACE: f32 = 0.774;

/// A Piper binary, a voice model, and a pitch.
#[derive(Clone)]
pub struct Voice {
    pub piper: PathBuf,
    pub model: PathBuf,
    /// Playback-speed factor applied to Piper's output. Below 1 lowers pitch
    /// (deeper voice) and slows speech, so Piper speaks faster to make up for it.
    pub pitch: f32,
}

/// Speak-ready text: one line, no markdown symbols.
pub fn tidy(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '*' | '#' | '_' | '`' | '~' | '>' | '|' => ' ',
            '\n' | '\r' | '\t' => ' ',
            _ => c,
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| !matches!(*w, "-" | "+"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Raw 22.05 kHz mono s16 from Piper.
pub fn synth(voice: &Voice, text: &str) -> Result<Vec<i16>, String> {
    let mut child = Command::new(&voice.piper)
        .arg("-m")
        .arg(&voice.model)
        .args(["--output-raw", "--length-scale", &(PACE * voice.pitch).to_string(), "--sentence-silence", "0.05"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("piper: {e}"))?;
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin
            .write_all(tidy(text).as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|e| format!("piper stdin: {e}"))?;
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .take(MAX_TTS_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("piper stdout: {e}"))?;
    let _ = child.kill();
    let _ = child.wait();
    if bytes.len() < 200 {
        return Err("piper produced no audio".into());
    }
    Ok(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
}

/// Glitched audio plus per-frame mouth envelope and glitch flags.
#[derive(Clone)]
pub struct Clip {
    pub samples: Vec<i16>,
    pub env: Vec<f32>,
    pub glitch: Vec<bool>,
    pub text: String,
}

impl Clip {
    pub fn duration(&self) -> f32 {
        self.samples.len() as f32 / RATE as f32
    }
}

fn resample(x: &[f32], factor: f32) -> Vec<f32> {
    let n = (x.len() as f32 / factor) as usize;
    (0..n)
        .map(|k| {
            let p = k as f32 * factor;
            let i = p as usize;
            let fr = p - i as f32;
            let a = x[i.min(x.len() - 1)];
            let b = *x.get(i + 1).unwrap_or(&a);
            a + (b - a) * fr
        })
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

/// Stutters, pitch jumps, dropouts and a bitcrush.
pub fn glitchify<R: Rng>(input: &[i16], text: &str, pitch: f32, rng: &mut R) -> Clip {
    let x: Vec<f32> = input.iter().map(|&s| s as f32 / 32768.0).collect();
    let x = if x.is_empty() || pitch == 1.0 { x } else { resample(&x, pitch) };
    let chunk = (RATE as f32 * 0.07) as usize;
    let mut out: Vec<f32> = Vec::with_capacity(x.len() * 13 / 10);
    let mut marks: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < x.len() {
        let end = (i + chunk).min(x.len());
        let c = &x[i..end];
        let start = out.len();
        let voiced = rms(c) > 0.03;
        let r: f32 = rng.random();
        if voiced && i > 0 && r < 0.10 {
            let part = &c[..c.len() * 2 / 3];
            for _ in 0..rng.random_range(1..=2) {
                out.extend_from_slice(part);
            }
            out.extend_from_slice(c);
            marks.push((start, out.len()));
        } else if voiced && r < 0.17 {
            let f = if rng.random_bool(0.5) {
                rng.random_range(0.72..0.88)
            } else {
                rng.random_range(1.15..1.40)
            };
            out.extend(resample(c, f));
            marks.push((start, out.len()));
        } else if voiced && r < 0.195 {
            out.extend(std::iter::repeat_n(0.0, c.len()));
            marks.push((start, out.len()));
        } else {
            out.extend_from_slice(c);
        }
        i = end;
    }

    // Bitcrush and sample-and-hold.
    let levels = 40.0;
    let mut held = 0.0;
    for (k, s) in out.iter_mut().enumerate() {
        if k % 2 == 0 {
            held = (*s * 1.15 * levels).round() / levels;
        }
        *s = held;
    }
    let samples: Vec<i16> = out.iter().map(|s| (s.clamp(-1.0, 1.0) * 32000.0) as i16).collect();

    let spf = RATE as f32 / FPS;
    let frames = (out.len() as f32 / spf).ceil() as usize;
    let mut env: Vec<f32> = (0..frames)
        .map(|f| {
            let a = (f as f32 * spf) as usize;
            let b = (((f + 1) as f32 * spf) as usize).min(out.len());
            rms(&out[a..b])
        })
        .collect();
    let peak = env.iter().cloned().fold(0.0001f32, f32::max);
    for e in env.iter_mut() {
        let v = (*e / peak).powf(0.6);
        *e = if v < 0.15 { 0.0 } else { v };
    }
    let glitch: Vec<bool> = (0..frames)
        .map(|f| {
            let a = (f as f32 * spf) as usize;
            let b = ((f + 1) as f32 * spf) as usize;
            marks.iter().any(|&(s, e)| s < b && e > a)
        })
        .collect();
    Clip { samples, env, glitch, text: text.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn glitch_shapes_match() {
        let tone: Vec<i16> = (0..RATE * 2)
            .map(|i| ((i as f32 * 0.05).sin() * 12000.0) as i16)
            .collect();
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let c = glitchify(&tone, "hi", 0.84, &mut rng);
        assert!(!c.samples.is_empty());
        assert_eq!(c.env.len(), c.glitch.len());
        let frames = (c.samples.len() as f32 / (RATE as f32 / FPS)).ceil() as usize;
        assert_eq!(c.env.len(), frames);
        assert!(c.env.iter().all(|e| (0.0..=1.0).contains(e)));
        assert!(c.glitch.iter().any(|&g| g), "2 s of tone should glitch at least once");
    }

    #[test]
    fn empty_input() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let c = glitchify(&[], "", 0.84, &mut rng);
        assert!(c.samples.is_empty() && c.env.is_empty());
    }

    #[test]
    fn tidy_strips_markdown() {
        assert_eq!(tidy("**Hi**\n- there"), "Hi there");
    }
}
