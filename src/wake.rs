//! Wake phrase: "Hey Miles", "Hey Zosia" and so on, with no key press.
//!
//! Vosk (a small offline recognizer) listens all the time and only looks
//! for the word "hey". When an utterance has one, Whisper transcribes that
//! clip and [`parse`] checks a persona's current name follows it. Vosk's
//! small model doesn't know every name (it has no "Zosia"), so the name
//! check is Whisper's job; that also means a rename works straight away.
//!
//! After a wake with nothing else said, it records until about 1 s of
//! silence. A wake with a request in the same breath ("Hey Miles, what time
//! is it?") goes straight through.
//!
//! libvosk loads at run time from a fixed path, so the app builds and runs
//! without it; only the wake phrase needs it.

use crate::audio;
use crate::persona::Persona;
use std::ffi::{CStr, CString, c_char, c_float, c_int, c_short, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const LIBVOSK: &str = "/usr/lib/libvosk.so";
pub const MODEL_DIR: &str = "vosk-model-small-en-us-0.15";
/// 100 ms of 16 kHz mono.
const CHUNK: usize = audio::MIC_RATE as usize / 10;
/// Longest clip Whisper checks for a wake phrase.
const WAKE_MAX: usize = audio::MIC_RATE as usize * 10;
/// Longest request after a wake.
const ASK_MAX: f32 = 30.0;
/// Silence that ends a request.
const ASK_END: f32 = 1.0;
/// Give up when nothing is said this long after a wake.
const ASK_START: f32 = 6.0;

/// Shared between the listener, the window and the worker.
#[derive(Default)]
pub struct Ctl {
    /// Set by the window: don't listen (speaking, thinking, Space held).
    pub mute: AtomicBool,
    /// Set by the listener while it records a request.
    pub asking: AtomicBool,
}

/// What a transcript says, if it starts with a wake phrase.
#[derive(Debug, PartialEq, Eq)]
pub struct Wake {
    pub persona: Persona,
    /// Words said after the name.
    pub rest: usize,
}

/// Lowercase letters only, folded so spellings Whisper tends to mix up
/// compare equal ("Zosia", "Sosia", "Zosha").
fn fold(word: &str) -> String {
    let w: String = word.chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect();
    let w = w.replace("ph", "f").replace("ck", "k").replace("sh", "s").replace('x', "ks");
    let mut out = String::new();
    for c in w.chars() {
        let c = match c {
            'z' => 's',
            'y' => 'i',
            'c' => 'k',
            _ => c,
        };
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != b[j])).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// How close `heard` (folded words) is to `name`, or None when too far.
fn closeness(heard: &[String], name: &str) -> Option<(usize, usize)> {
    let want: Vec<String> = name.split_whitespace().map(fold).filter(|w| !w.is_empty()).collect();
    if want.is_empty() {
        return None;
    }
    // The full name, or just its first word ("Hey Rex" for Rex Static).
    let mut best = None;
    for n in [want.len(), 1] {
        if heard.len() < n {
            continue;
        }
        let (a, b) = (heard[..n].concat(), want[..n].concat());
        let len = b.chars().count();
        let max = match len {
            0..=3 => 0,
            4..=5 => 1,
            _ => 2,
        };
        // Whisper sometimes glues a sound to the front ("Hey Dazoya" for
        // "Hey Zosia"), so also try with one or two leading letters dropped.
        // Each dropped letter counts against the match when ranking. Only
        // for names of 5 letters or more: short names ("Rex") would also
        // match other names ("Alex").
        let chars: Vec<char> = a.chars().collect();
        let skips = if len >= 5 { 2usize.min(chars.len().saturating_sub(3)) } else { 0 };
        for skip in 0..=skips {
            let tail: String = chars[skip..].iter().collect();
            let d = distance(&tail, &b);
            let score = d + skip;
            if d <= max && best.is_none_or(|(bd, _)| score < bd) {
                best = Some((score, n));
            }
        }
    }
    best
}

/// Find "hey <name>" in a transcript. `names` holds each persona's current
/// name. The closest name wins.
pub fn parse(text: &str, names: &[(Persona, String)]) -> Option<Wake> {
    let words: Vec<String> = text.split_whitespace().map(fold).filter(|w| !w.is_empty()).collect();
    let at = words.iter().position(|w| matches!(w.as_str(), "hei" | "hai" | "hi"))?;
    let after = &words[at + 1..];
    let (persona, _, used) = names
        .iter()
        .filter_map(|(p, n)| closeness(after, n).map(|(d, used)| (*p, d, used)))
        .min_by_key(|x| x.1)?;
    Some(Wake { persona, rest: after.len() - used })
}

/// Does a Vosk transcript have the word "hey"? A name starting with an s or
/// z sound ("Hey Zosia") blends into it, and Vosk hears "he's osha" or
/// "hayes osha", so those count too. Whisper still checks the name, so an
/// ordinary "he's" only costs one Whisper run.
pub fn has_hey(text: &str) -> bool {
    text.split_whitespace().any(|w| matches!(w, "hey" | "hay" | "he's" | "hey's" | "hayes" | "hays" | "haze"))
}

type ModelNew = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type Free = unsafe extern "C" fn(*mut c_void);
type RecNew = unsafe extern "C" fn(*mut c_void, c_float) -> *mut c_void;
type Accept = unsafe extern "C" fn(*mut c_void, *const c_short, c_int) -> c_int;
type Text = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type Delays = unsafe extern "C" fn(*mut c_void, c_float, c_float, c_float);
type LogLevel = unsafe extern "C" fn(c_int);

/// libvosk, loaded from [`LIBVOSK`], with one model and one recognizer.
struct Vosk {
    model: *mut c_void,
    rec: *mut c_void,
    model_free: Free,
    rec_free: Free,
    accept: Accept,
    result: Text,
    reset: Free,
    delays: Option<Delays>,
    _lib: libloading::Library,
}

impl Vosk {
    fn open(model_dir: &std::path::Path) -> Result<Self, String> {
        // SAFETY: a fixed system path, and the symbol types match vosk_api.h.
        unsafe {
            let lib = libloading::Library::new(LIBVOSK).map_err(|e| format!("{LIBVOSK}: {e} (install vosk-api)"))?;
            macro_rules! sym {
                ($name:literal, $t:ty) => {
                    *lib.get::<$t>($name).map_err(|e| format!("libvosk: {e}"))?
                };
            }
            let log: LogLevel = sym!(b"vosk_set_log_level", LogLevel);
            log(-1);
            let model_new: ModelNew = sym!(b"vosk_model_new", ModelNew);
            let rec_new: RecNew = sym!(b"vosk_recognizer_new", RecNew);
            let model_free: Free = sym!(b"vosk_model_free", Free);
            let path = CString::new(model_dir.to_str().ok_or("model path is not UTF-8")?).map_err(|e| e.to_string())?;
            let model = model_new(path.as_ptr());
            if model.is_null() {
                return Err(format!("Vosk model missing: {} (run setup.sh)", model_dir.display()));
            }
            let rec = rec_new(model, audio::MIC_RATE as c_float);
            if rec.is_null() {
                model_free(model);
                return Err("Vosk recognizer failed".into());
            }
            Ok(Self {
                model,
                rec,
                model_free,
                rec_free: sym!(b"vosk_recognizer_free", Free),
                accept: sym!(b"vosk_recognizer_accept_waveform_s", Accept),
                result: sym!(b"vosk_recognizer_final_result", Text),
                reset: sym!(b"vosk_recognizer_reset", Free),
                delays: lib.get::<Delays>(b"vosk_recognizer_set_endpointer_delays").ok().map(|s| *s),
                _lib: lib,
            })
        }
    }

    /// Feed audio. Returns the utterance text once Vosk hears it end.
    fn feed(&mut self, pcm: &[i16]) -> Option<String> {
        // SAFETY: rec is live until drop; pcm outlives the call.
        let done = unsafe { (self.accept)(self.rec, pcm.as_ptr(), pcm.len() as c_int) };
        (done == 1).then(|| self.text())
    }

    fn text(&mut self) -> String {
        // SAFETY: the string stays valid until the next recognizer call.
        let raw = unsafe { CStr::from_ptr((self.result)(self.rec)) }.to_string_lossy().into_owned();
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v["text"].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// Start a fresh utterance. `start`: most silence before speech;
    /// `end`: silence that ends it; `max`: longest utterance (seconds).
    fn restart(&mut self, start: f32, end: f32, max: f32) {
        // SAFETY: rec is live until drop.
        unsafe {
            (self.reset)(self.rec);
            if let Some(d) = self.delays {
                d(self.rec, start, end, max);
            }
        }
    }
}

impl Drop for Vosk {
    fn drop(&mut self) {
        // SAFETY: freed once, recognizer before its model.
        unsafe {
            (self.rec_free)(self.rec);
            (self.model_free)(self.model);
        }
    }
}

/// Listen for wake phrases until the app quits. `check` sends a clip with
/// "hey" in it to Whisper and returns the verdict channel: true means
/// record a request now.
pub fn run(
    ctl: Arc<Ctl>,
    level: Arc<AtomicU32>,
    model_dir: std::path::PathBuf,
    check: impl Fn(Vec<i16>) -> Option<mpsc::Receiver<bool>>,
    hear: impl Fn(Vec<i16>),
) {
    let mut vosk = match Vosk::open(&model_dir) {
        Ok(v) => v,
        Err(e) => return eprintln!("rex-static: wake: {e}"),
    };
    let idle = |v: &mut Vosk| v.restart(5.0, 0.5, 10.0);
    let ask = |v: &mut Vosk| v.restart(ASK_START, ASK_END, ASK_MAX);
    loop {
        let mut mic = match audio::Stream::start() {
            Ok(m) => m,
            Err(e) => return eprintln!("rex-static: wake: mic: {e}"),
        };
        idle(&mut vosk);
        let mut clip: Vec<i16> = Vec::new();
        let mut verdict: Option<mpsc::Receiver<bool>> = None;
        let mut asking = false;
        let mut muted = false;
        while let Some(chunk) = mic.read(CHUNK) {
            if ctl.mute.load(Ordering::Relaxed) {
                if !muted || asking {
                    muted = true;
                    asking = false;
                    verdict = None;
                    clip.clear();
                    ctl.asking.store(false, Ordering::Relaxed);
                    level.store(0f32.to_bits(), Ordering::Relaxed);
                    idle(&mut vosk);
                }
                continue;
            }
            muted = false;
            clip.extend_from_slice(&chunk);
            if let Some(r) = &verdict {
                // Whisper is checking; keep the audio for the request.
                match r.try_recv() {
                    Err(mpsc::TryRecvError::Empty) => continue,
                    Ok(true) => {
                        verdict = None;
                        asking = true;
                        ctl.asking.store(true, Ordering::Relaxed);
                        beep();
                        ask(&mut vosk);
                        let held = std::mem::take(&mut clip);
                        if let Some(t) = vosk.feed(&held) {
                            clip = held;
                            finish(&mut asking, &ctl, &level, t, std::mem::take(&mut clip), &hear);
                            idle(&mut vosk);
                        } else {
                            clip = held;
                        }
                        continue;
                    }
                    _ => {
                        verdict = None;
                        clip.clear();
                        idle(&mut vosk);
                        continue;
                    }
                }
            }
            if asking {
                level.store(rms_level(&chunk).to_bits(), Ordering::Relaxed);
            }
            if let Some(text) = vosk.feed(&chunk) {
                let pcm = std::mem::take(&mut clip);
                if asking {
                    finish(&mut asking, &ctl, &level, text, pcm, &hear);
                } else if has_hey(&text) {
                    verdict = check(pcm[pcm.len().saturating_sub(WAKE_MAX)..].to_vec());
                }
                idle(&mut vosk);
            } else if clip.len() > WAKE_MAX + CHUNK * 10 && !asking {
                // Vosk caps an utterance, so this only trims the leading silence.
                clip.drain(..clip.len() - WAKE_MAX);
            }
        }
        // The mic stream died (PipeWire restarted?). Try again shortly.
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn finish(asking: &mut bool, ctl: &Ctl, level: &AtomicU32, text: String, pcm: Vec<i16>, hear: &impl Fn(Vec<i16>)) {
    *asking = false;
    ctl.asking.store(false, Ordering::Relaxed);
    level.store(0f32.to_bits(), Ordering::Relaxed);
    // Vosk heard nothing at all: a false start, so don't bother Whisper.
    if !text.trim().is_empty() {
        hear(pcm);
    }
}

fn rms_level(chunk: &[i16]) -> f32 {
    let sum: f64 = chunk.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
    let rms = (sum / chunk.len().max(1) as f64).sqrt() as f32;
    (rms * 6.0).min(1.0)
}

/// A short soft tone: "I'm listening".
fn beep() {
    let rate = 22_050u32;
    let n = rate as usize * 12 / 100;
    let tone: Vec<i16> = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let fade = (i.min(n - i) as f32 / (rate as f32 * 0.01)).min(1.0);
            ((t * 880.0 * std::f32::consts::TAU).sin() * fade * 5000.0) as i16
        })
        .collect();
    std::thread::spawn(move || {
        let never = AtomicBool::new(false);
        let _ = audio::play(&tone, rate, &never, || {});
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<(Persona, String)> {
        vec![
            (Persona::Male, "Rex Static".into()),
            (Persona::Female, "Roxy Static".into()),
            (Persona::ModernMale, "Miles".into()),
            (Persona::ModernFemale, "Zosia".into()),
        ]
    }

    fn wake(text: &str) -> Option<(Persona, usize)> {
        parse(text, &names()).map(|w| (w.persona, w.rest))
    }

    #[test]
    fn finds_each_name() {
        assert_eq!(wake("Hey Miles."), Some((Persona::ModernMale, 0)));
        assert_eq!(wake("Hey, Zosia!"), Some((Persona::ModernFemale, 0)));
        assert_eq!(wake("Hey Rex."), Some((Persona::Male, 0)));
        assert_eq!(wake("hey rex static"), Some((Persona::Male, 0)));
        assert_eq!(wake("Hey Roxy"), Some((Persona::Female, 0)));
    }

    #[test]
    fn close_spellings() {
        assert_eq!(wake("Hey Sosia"), Some((Persona::ModernFemale, 0)));
        assert_eq!(wake("Hey Zosha."), Some((Persona::ModernFemale, 0)));
        assert_eq!(wake("Hey Myles"), Some((Persona::ModernMale, 0)));
        assert_eq!(wake("Hi Miles"), Some((Persona::ModernMale, 0)));
        assert_eq!(wake("Hey Roxie"), Some((Persona::Female, 0)));
        assert_eq!(wake("Hey Dazoya."), Some((Persona::ModernFemale, 0)));
        assert_eq!(wake("Hey Zoya"), Some((Persona::ModernFemale, 0)));
    }

    #[test]
    fn counts_the_request() {
        assert_eq!(wake("Hey Miles, what time is it?"), Some((Persona::ModernMale, 4)));
        assert_eq!(wake("Hey Rex Static, lights off"), Some((Persona::Male, 2)));
    }

    #[test]
    fn ignores_other_talk() {
        assert_eq!(wake("Hey, what time is it?"), None);
        assert_eq!(wake("They said it was fine."), None);
        assert_eq!(wake("Okay Zosia"), None);
        assert_eq!(wake("Hey Max"), None);
        assert_eq!(wake("Hey Rick"), None);
        assert_eq!(wake("Hey Sophie, come here"), None);
        assert_eq!(wake("Hey Mike"), None);
        assert_eq!(wake("Hey Alex"), None);
        assert_eq!(wake("Hey"), None);
        assert_eq!(wake(""), None);
    }

    #[test]
    fn renamed_persona() {
        let mut n = names();
        n[2].1 = "Barnaby".into();
        assert_eq!(parse("Hey Barnaby", &n).map(|w| w.persona), Some(Persona::ModernMale));
        assert_eq!(parse("Hey Miles", &n), None);
    }

    /// Manual: REX_WAKE_CLIPS=<dir of 22050 Hz s16 .raw files> cargo test
    /// --release vosk_on_clips -- --ignored --nocapture
    #[test]
    #[ignore]
    fn vosk_on_clips() {
        let dir = std::env::var("REX_WAKE_CLIPS").expect("REX_WAKE_CLIPS");
        let model = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .join(".local/share/rex-static/models")
            .join(MODEL_DIR);
        let mut v = Vosk::open(&model).unwrap();
        let (mut audio_s, mut spent) = (0.0, std::time::Duration::ZERO);
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let b = std::fs::read(&f).unwrap();
            let src: Vec<f32> = b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32).collect();
            let n = src.len() * 16_000 / 22_050;
            let mut pcm: Vec<i16> = (0..n)
                .map(|i| {
                    let x = i as f32 * 22_050.0 / 16_000.0;
                    let (j, f) = (x as usize, x.fract());
                    let a = src[j.min(src.len() - 1)];
                    let b = src[(j + 1).min(src.len() - 1)];
                    (a + (b - a) * f) as i16
                })
                .collect();
            let mut r = ::rand::rng();
            pcm.extend((0..16_000 * 3).map(|_| ::rand::Rng::random_range(&mut r, -60i16..60)));
            let t0 = pcm.len() - 16_000 * 3;
            v.restart(5.0, 0.5, 10.0);
            let t = std::time::Instant::now();
            let mut text = String::new();
            for (k, c) in pcm.chunks(CHUNK).enumerate() {
                if let Some(x) = v.feed(c) {
                    text.push_str(&format!("@{:.1}s ", (k * CHUNK) as f32 / 16_000.0 - t0 as f32 / 16_000.0));
                    text.push_str(&x);
                    text.push(' ');
                }
            }
            let tail = v.text();
            if !tail.is_empty() {
                text.push_str(&format!("[tail: {tail}]"));
            }
            spent += t.elapsed();
            audio_s += pcm.len() as f64 / 16_000.0;
            println!("{:30} {:?} hey={}", f.file_name().unwrap().to_string_lossy(), text.trim(), has_hey(&text));
        }
        println!("CPU: {:.1}% of one core", spent.as_secs_f64() / audio_s * 100.0);
    }

    /// Manual: like `vosk_on_clips`, through Whisper and [`parse`].
    #[test]
    #[ignore]
    fn whisper_on_clips() {
        let dir = std::env::var("REX_WAKE_CLIPS").expect("REX_WAKE_CLIPS");
        let models = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share/rex-static/models");
        let stt = crate::stt::Stt::new(&models.join("ggml-base.en.bin")).unwrap();
        let n = names();
        let prompt = format!("Hey {}.", n.iter().map(|x| x.1.as_str()).collect::<Vec<_>>().join(", hey "));
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let b = std::fs::read(&f).unwrap();
            let src: Vec<f32> = b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32).collect();
            let len = src.len() * 16_000 / 22_050;
            let pcm: Vec<i16> = (0..len)
                .map(|i| {
                    let x = i as f32 * 22_050.0 / 16_000.0;
                    let (j, fr) = (x as usize, x.fract());
                    let a = src[j.min(src.len() - 1)];
                    (a + (src[(j + 1).min(src.len() - 1)] - a) * fr) as i16
                })
                .collect();
            let text = stt.transcribe_with(&pcm, Some(&prompt)).unwrap();
            println!("{:36} {:?} -> {:?}", f.file_name().unwrap().to_string_lossy(), text, parse(&text, &n));
        }
    }

    #[test]
    fn hey_in_vosk_text() {
        assert!(has_hey("hey miles"));
        assert!(has_hey("okay hey social"));
        assert!(has_hey("he's osha what time is it"));
        assert!(has_hey("hayes osha"));
        assert!(!has_hey("they said it was fine"));
    }
}
