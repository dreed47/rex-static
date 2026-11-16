//! Text to speech with Piper or Kokoro, then a deliberately broken-broadcast
//! DSP pass.

use crate::lipsync::{self, Phone};
use rand::Rng;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const RATE: u32 = 22_050;
/// Envelope and glitch-flag frames per second (drives the mouth).
pub const FPS: f32 = 30.0;
const MAX_TTS_BYTES: usize = RATE as usize * 2 * 120;
/// Cap on what the timed script may write to stderr (timings and warnings).
const MAX_ERR_BYTES: u64 = 4 << 20;
/// Piper with phoneme timings, run by Piper's own Python.
const TIMED: &str = include_str!("../tools/piper_timed.py");
/// Kokoro with phoneme timings, run by the kokoro venv's Python.
const KOKORO: &str = include_str!("../tools/kokoro_timed.py");

/// Kokoro instead of Piper: the venv Python, model, voices file and voice.
#[derive(Clone)]
pub struct Kokoro {
    pub python: PathBuf,
    pub model: PathBuf,
    pub voices: PathBuf,
    pub voice: String,
}

impl Kokoro {
    /// The files it needs that are missing.
    pub fn missing(&self) -> Vec<&std::path::Path> {
        [&self.python, &self.model, &self.voices].into_iter().map(|p| p.as_path()).filter(|p| !p.is_file()).collect()
    }
}

/// A Piper binary, a voice model, and a pitch (or Kokoro).
#[derive(Clone)]
pub struct Voice {
    pub piper: PathBuf,
    pub model: PathBuf,
    /// Playback-speed factor applied to Piper's output. Below 1 lowers pitch
    /// (deeper voice) and slows speech, so Piper speaks faster to make up for it.
    pub pitch: f32,
    /// Piper length scale.
    pub pace: f32,
    /// Stutter, warble and crunch the audio.
    pub retro: bool,
    /// Piper's own Python, to run `tools/piper_timed.py` for phoneme
    /// timings; `None` to go by loudness.
    pub timed: Option<PathBuf>,
    /// Speak with Kokoro instead of Piper.
    pub kokoro: Option<Kokoro>,
}

/// The Python next to the real Piper binary (Piper's uv tool venv), if any.
/// Only the symlink to Piper is resolved: the venv's `python` must keep its
/// own path, or it loses the venv.
pub fn timed_python(piper: &std::path::Path) -> Option<PathBuf> {
    let py = std::fs::canonicalize(piper).ok()?.parent()?.join("python");
    py.is_file().then_some(py)
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

/// Raw 22.05 kHz mono s16 from Piper or Kokoro, with phoneme timings when
/// the timed script runs (empty when it can't, so the mouth goes by loudness).
pub fn synth(voice: &Voice, text: &str) -> Result<(Vec<i16>, Vec<Phone>), String> {
    if let Some(k) = &voice.kokoro {
        let mut cmd = Command::new(&k.python);
        cmd.args(["-I", "-c", KOKORO, "-m"])
            .arg(&k.model)
            .arg("--voices")
            .arg(&k.voices)
            .args(["--voice", &k.voice, "--lang", if k.voice.starts_with('b') { "en-gb" } else { "en-us" }])
            .args(["--speed", &(1.0 / voice.pace).to_string()]);
        let (pcm, err) = run(cmd, text, true)?;
        return Ok((pcm, parse_align(&err)));
    }
    if let Some(py) = &voice.timed
        && let Ok(out) = synth_timed(py, voice, text)
    {
        return Ok(out);
    }
    let mut cmd = Command::new(&voice.piper);
    cmd.arg("-m")
        .arg(&voice.model)
        .args(["--output-raw", "--length-scale", &voice.pace.to_string(), "--sentence-silence", "0.05"]);
    run(cmd, text, false).map(|(pcm, _)| (pcm, Vec::new()))
}

fn synth_timed(py: &std::path::Path, voice: &Voice, text: &str) -> Result<(Vec<i16>, Vec<Phone>), String> {
    let mut cmd = Command::new(py);
    cmd.args(["-I", "-c", TIMED, "-m"])
        .arg(&voice.model)
        .args(["--length-scale", &voice.pace.to_string(), "--sentence-silence", "0.05"]);
    let (pcm, err) = run(cmd, text, true)?;
    Ok((pcm, parse_align(&err)))
}

/// Phonemes from the script's last `ALIGN:` line; empty if there is none.
fn parse_align(err: &[u8]) -> Vec<Phone> {
    let err = String::from_utf8_lossy(err);
    let Some(line) = err.lines().rev().find_map(|l| l.strip_prefix("ALIGN:")) else {
        return Vec::new();
    };
    let mut phones: Vec<Phone> = serde_json::from_str(line).unwrap_or_default();
    phones.retain(|p| p.2 > p.1);
    phones.sort_by_key(|p| p.1);
    phones
}

/// Feed `text` to `cmd`, read its raw audio, and its stderr when `err`.
fn run(mut cmd: Command, text: &str, err: bool) -> Result<(Vec<i16>, Vec<u8>), String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(if err { Stdio::piped() } else { Stdio::null() })
        .spawn()
        .map_err(|e| format!("tts: {e}"))?;
    // Read stderr alongside stdout, so neither pipe fills up and stalls it.
    let errs = child.stderr.take().map(|e| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut e = e.take(MAX_ERR_BYTES);
            let _ = e.read_to_end(&mut buf);
            // Drain the rest so the child never blocks on a full pipe.
            let _ = std::io::copy(&mut e.into_inner(), &mut std::io::sink());
            buf
        })
    });
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin
            .write_all(tidy(text).as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|e| format!("tts stdin: {e}"))?;
    }
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("piped stdout")
        .take(MAX_TTS_BYTES as u64)
        .read_to_end(&mut bytes);
    let status = if read.is_ok() { child.wait().ok() } else { None };
    let _ = child.kill();
    let _ = child.wait();
    let errs = errs.and_then(|h| h.join().ok()).unwrap_or_default();
    read.map_err(|e| format!("tts stdout: {e}"))?;
    if bytes.len() < 200 {
        return Err("tts produced no audio".into());
    }
    if err && !status.is_some_and(|s| s.success()) {
        return Err("timed tts failed".into());
    }
    Ok((bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(), errs))
}

/// Glitched audio plus per-frame mouth envelope and glitch flags.
#[derive(Clone)]
pub struct Clip {
    pub samples: Vec<i16>,
    pub env: Vec<f32>,
    pub glitch: Vec<bool>,
    /// Mouth frame weights at `lipsync::FPS`, in `lipsync::SHAPES` order.
    pub mouth: Vec<[f32; 8]>,
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

/// A stretch of output audio and where it came from: the first sample of
/// the pitched input and how many input samples each output one moves on,
/// or `None` for silence.
type Seg = (usize, usize, Option<(f32, f32)>);

/// Stutters, pitch jumps, dropouts and a bitcrush when `retro`; otherwise
/// the audio stays clean and only the mouth is worked out. `phones` are
/// Piper's timings for `input` (empty: the mouth goes by loudness).
pub fn glitchify<R: Rng>(input: &[i16], phones: &[Phone], text: &str, pitch: f32, retro: bool, rng: &mut R) -> Clip {
    let x: Vec<f32> = input.iter().map(|&s| s as f32 / 32768.0).collect();
    let x = if x.is_empty() || pitch == 1.0 { x } else { resample(&x, pitch) };
    let src = Source { phones, pitch };
    if !retro {
        let segs = vec![(0, x.len(), Some((0.0, 1.0)))];
        return clip_of(x, Vec::new(), &segs, &src, text);
    }
    let mut segs: Vec<Seg> = Vec::new();
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
                segs.push((out.len(), part.len(), Some((i as f32, 1.0))));
                out.extend_from_slice(part);
            }
            segs.push((out.len(), c.len(), Some((i as f32, 1.0))));
            out.extend_from_slice(c);
            marks.push((start, out.len()));
        } else if voiced && r < 0.17 {
            let f = if rng.random_bool(0.5) {
                rng.random_range(0.72..0.88)
            } else {
                rng.random_range(1.15..1.40)
            };
            let r = resample(c, f);
            segs.push((out.len(), r.len(), Some((i as f32, f))));
            out.extend(r);
            marks.push((start, out.len()));
        } else if voiced && r < 0.195 {
            segs.push((out.len(), c.len(), None));
            out.extend(std::iter::repeat_n(0.0, c.len()));
            marks.push((start, out.len()));
        } else {
            segs.push((out.len(), c.len(), Some((i as f32, 1.0))));
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
    clip_of(out, marks, &segs, &src, text)
}

/// Piper's timings and the pitch factor that maps them onto the output.
struct Source<'a> {
    phones: &'a [Phone],
    pitch: f32,
}

/// Mouth weights for the output audio: each ms looks up the Piper sample
/// it came from (through `segs`) and that phoneme's shape, or, without
/// timings, how loud that bit is (`env`, at `FPS`).
fn mouth_of(len: usize, segs: &[Seg], src: &Source, env: &[f32]) -> Vec<[f32; 8]> {
    let shapes = lipsync::shapes(src.phones);
    let ms = len * 1000 / RATE as usize;
    let mut k = 0;
    let targets: Vec<Option<u8>> = (0..ms)
        .map(|m| {
            if src.phones.is_empty() {
                let f = (m as f32 / 1000.0 * FPS) as usize;
                return lipsync::shape_of_level(env.get(f).copied().unwrap_or(0.0));
            }
            let o = m * RATE as usize / 1000;
            while k + 1 < segs.len() && segs[k].0 + segs[k].1 <= o {
                k += 1;
            }
            let (start, _, from) = *segs.get(k)?;
            let (at, rate) = from?;
            lipsync::shape_at(src.phones, &shapes, (at + (o - start.min(o)) as f32 * rate) * src.pitch)
        })
        .collect();
    lipsync::weights(&targets)
}

/// A clip with its mouth envelope, mouth frames and glitch frames (`marks`
/// are sample ranges that glitched).
fn clip_of(out: Vec<f32>, marks: Vec<(usize, usize)>, segs: &[Seg], src: &Source, text: &str) -> Clip {
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
    let mouth = if out.is_empty() { Vec::new() } else { mouth_of(out.len(), segs, src, &env) };
    Clip { samples, env, glitch, mouth, text: text.to_string() }
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
        let c = glitchify(&tone, &[], "hi", 0.84, true, &mut rng);
        assert!(!c.samples.is_empty());
        assert_eq!(c.env.len(), c.glitch.len());
        let frames = (c.samples.len() as f32 / (RATE as f32 / FPS)).ceil() as usize;
        assert_eq!(c.env.len(), frames);
        assert!(c.env.iter().all(|e| (0.0..=1.0).contains(e)));
        assert!(c.glitch.iter().any(|&g| g), "2 s of tone should glitch at least once");
    }

    #[test]
    fn clean_keeps_audio() {
        let tone: Vec<i16> = (0..RATE).map(|i| ((i as f32 * 0.05).sin() * 12000.0) as i16).collect();
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let c = glitchify(&tone, &[], "hi", 1.0, false, &mut rng);
        assert_eq!(c.samples.len(), tone.len());
        assert!(c.glitch.iter().all(|&g| !g));
        assert!(c.env.iter().any(|&e| e > 0.5));
    }

    #[test]
    fn empty_input() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let c = glitchify(&[], &[], "", 0.84, true, &mut rng);
        assert!(c.samples.is_empty() && c.env.is_empty());
    }

    #[test]
    fn mouth_follows_phones_through_pitch() {
        // 1 s of tone; "a" for the first half, "m" for the second.
        let tone: Vec<i16> = (0..RATE).map(|i| ((i as f32 * 0.05).sin() * 12000.0) as i16).collect();
        let h = RATE / 2;
        let phones = vec![("a".to_string(), 0, h), ("m".to_string(), h, RATE)];
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let c = glitchify(&tone, &phones, "hi", 0.8, false, &mut rng);
        // Pitched down 0.8, the clip runs 1.25 s and the switch is at 0.625 s.
        assert!((c.duration() - 1.25).abs() < 0.01);
        let at = |s: f32| c.mouth[(s * lipsync::FPS) as usize];
        assert!(at(0.3)[0] > 0.99, "{:?}", at(0.3));
        assert!(at(0.9)[6] > 0.99, "{:?}", at(0.9));
        // The mouth leads the sound a little.
        assert!(at(0.6)[6] > 0.5, "{:?}", at(0.6));
        // Retro keeps the mouth length matched to the glitched audio.
        let r = glitchify(&tone, &phones, "hi", 0.8, true, &mut rng);
        let want = (r.samples.len() as f32 / RATE as f32 * lipsync::FPS).ceil() as usize;
        assert!(r.mouth.len().abs_diff(want) <= 1, "{} vs {want}", r.mouth.len());
    }

    #[test]
    fn mouth_by_loudness_without_phones() {
        let tone: Vec<i16> = (0..RATE).map(|i| ((i as f32 * 0.05).sin() * 12000.0) as i16).collect();
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let c = glitchify(&tone, &[], "hi", 1.0, false, &mut rng);
        assert!(c.mouth[30][0] > 0.9, "{:?}", c.mouth[30]);
    }

    #[test]
    fn align_line_parses() {
        let err = b"warning: something\n\nALIGN:[[\"h\",0,10],[\"\xc9\x9b\",10,30],[\"x\",40,40]]\n";
        let p = parse_align(err);
        assert_eq!(p, vec![("h".to_string(), 0, 10), ("ɛ".to_string(), 10, 30)]);
        assert!(parse_align(b"no timings").is_empty());
        assert!(parse_align(b"ALIGN:not json").is_empty());
    }

    #[test]
    fn tidy_strips_markdown() {
        assert_eq!(tidy("**Hi**\n- there"), "Hi there");
    }
}
