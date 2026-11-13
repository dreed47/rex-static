//! Rex Static (or Roxy Static): a glitchy, plastic-looking digital person in
//! a small window. Settings come from `~/.config/rex-static/config`.
//! Hold Space to talk, or type and press Enter. Esc clears typing, then quits.

mod audio;
mod config;
mod face;
mod head;
mod llm;
mod mannequin;
mod memory;
mod persona;
mod stt;
mod tools;
mod tts;

use ::rand::Rng;
use ::rand::seq::IndexedRandom;
use head::{Mood, Pose};
use macroquad::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Booting,
    Idle,
    Thinking,
    Speaking,
    Error,
}

#[derive(Clone)]
struct Shared {
    phase: Phase,
    caption: String,
    heard: String,
    clip: Option<Arc<tts::Clip>>,
    clip_start: Option<Instant>,
    error: String,
}

enum Job {
    Hear(Vec<i16>),
    Say(String),
}

/// Most characters you can type in one message.
const TYPE_MAX: usize = 500;
/// Wait this long in silence before a "hmm".
const SHORT_FILLER_AFTER: Duration = Duration::from_millis(1500);
/// Wait this long before a "let me check" (straight away once a tool runs).
const LONG_FILLER_AFTER: Duration = Duration::from_millis(5000);
/// How much of the full head sway stays when not talking.
const IDLE_SWAY: f32 = 0.45;

fn models_dir() -> PathBuf {
    memory::data_dir().join("models")
}

/// Worker-side handle for speaking and updating what the window shows.
struct Rex {
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
    voice: tts::Voice,
}

impl Rex {
    fn set(&self, f: impl FnOnce(&mut Shared)) {
        if let Ok(mut s) = self.shared.lock() {
            f(&mut s);
        }
    }

    fn render(&self, text: &str) -> Option<Arc<tts::Clip>> {
        match tts::synth(&self.voice, text) {
            Ok(pcm) => Some(Arc::new(tts::glitchify(&pcm, text, self.voice.pitch, &mut ::rand::rng()))),
            Err(e) => {
                eprintln!("rex-static: {e}");
                None
            }
        }
    }

    fn speak(&self, clip: Arc<tts::Clip>) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        let text = clip.text.clone();
        let samples = clip.samples.clone();
        self.set(|s| {
            s.phase = Phase::Speaking;
            s.caption = text;
            s.clip = Some(clip);
            s.clip_start = None;
        });
        let r = audio::play(&samples, tts::RATE, &self.cancel, || {
            self.set(|s| s.clip_start = Some(Instant::now()))
        });
        if let Err(e) = r {
            eprintln!("rex-static: playback: {e}");
        }
        self.set(|s| {
            s.clip = None;
            s.clip_start = None;
        });
    }

    fn say(&self, text: &str) {
        match self.render(text) {
            Some(c) => self.speak(c),
            None => self.set(|s| s.caption = text.to_string()),
        }
    }
}

fn pick(lines: &[&str]) -> String {
    lines.choose(&mut ::rand::rng()).copied().unwrap_or("").to_string()
}

/// Rewrite the long-term notes once enough new talk has piled up.
fn maybe_update_notes(claude: &llm::Claude, busy: &Arc<AtomicBool>, who: &persona::Who) {
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let claude = claude.clone();
    let busy = busy.clone();
    let who = who.clone();
    thread::spawn(move || {
        let run = || -> Result<(), String> {
            let m = memory::Memory::open(&memory::db_path(), config::get().persona.tag()).map_err(|e| e.to_string())?;
            let (old, upto) = m.notes().map_err(|e| e.to_string())?;
            if m.count_since(upto).map_err(|e| e.to_string())? < 10 {
                return Ok(());
            }
            let new = m.since(upto).map_err(|e| e.to_string())?;
            let Some(last) = new.last().map(|x| x.0) else { return Ok(()) };
            let turns: Vec<memory::Turn> = new.into_iter().map(|x| x.1).collect();
            let req = memory::Turn { role: "user".into(), text: persona::notes_request(&who, &old, &turns) };
            if let llm::Reply::Text(t) = claude.ask(&persona::notes_system(&who), &[req], 3000)? {
                m.set_notes(t.trim(), last).map_err(|e| e.to_string())?;
            }
            Ok(())
        };
        if let Err(e) = run() {
            eprintln!("rex-static: notes: {e}");
        }
        busy.store(false, Ordering::SeqCst);
    });
}

fn worker(rex: Rex, rx: mpsc::Receiver<Job>) {
    let fail = |e: String| {
        eprintln!("rex-static: {e}");
        rex.set(|s| {
            s.phase = Phase::Error;
            s.error = e;
        });
    };
    if !rex.voice.piper.is_file() {
        return fail(format!("Piper missing: {} (run setup.sh)", rex.voice.piper.display()));
    }
    if !rex.voice.model.is_file() {
        return fail(format!("Voice missing: {} (run setup.sh)", rex.voice.model.display()));
    }
    let stt = match stt::Stt::new(&models_dir().join("ggml-base.en.bin")) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let mem = match memory::Memory::open(&memory::db_path(), config::get().persona.tag()) {
        Ok(m) => m,
        Err(e) => return fail(format!("memory: {e}")),
    };
    let claude = llm::load_key().map(|k| llm::Claude::new(k, config::get().model.clone()));
    let cfg = config::get();
    let tools = Arc::new(tools::Tools::new(rex.cancel.clone(), cfg.claude_code.clone(), cfg.ha.clone()));
    let notes_busy = Arc::new(AtomicBool::new(false));
    let key = cfg.persona.name_key();
    let mut who = persona::Who {
        name: mem.setting(key).unwrap_or_else(|| cfg.persona.default_name().to_string()),
        persona: cfg.persona,
        user: cfg.user_name.clone(),
        tools: tools.set(),
    };

    // Pre-render filler lines in the background so they play instantly.
    // Index 0 holds the short ones, 1 the long ones.
    let fillers: Arc<Mutex<[Vec<Arc<tts::Clip>>; 2]>> = Arc::default();
    {
        let fillers = fillers.clone();
        let voice = rex.voice.clone();
        thread::spawn(move || {
            for (i, lines) in [persona::SHORT_FILLERS, persona::LONG_FILLERS].into_iter().enumerate() {
                for line in lines {
                    if let Ok(pcm) = tts::synth(&voice, line) {
                        let c = Arc::new(tts::glitchify(&pcm, line, voice.pitch, &mut ::rand::rng()));
                        if let Ok(mut f) = fillers.lock() {
                            f[i].push(c);
                        }
                    }
                }
            }
        });
    }
    let mut last_filler = String::new();

    rex.cancel.store(false, Ordering::SeqCst);
    rex.say(&who.fill(&if claude.is_some() { pick(persona::GREETINGS) } else { persona::NO_KEY.to_string() }));
    rex.set(|s| s.phase = Phase::Idle);

    for job in rx {
        rex.cancel.store(false, Ordering::SeqCst);
        rex.set(|s| {
            s.phase = Phase::Thinking;
            s.caption.clear();
            s.heard = "...".into();
        });
        let text = match job {
            Job::Say(t) => t,
            Job::Hear(pcm) => match stt.transcribe(&pcm) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("rex-static: stt: {e}");
                    String::new()
                }
            },
        };
        if text.is_empty() {
            rex.set(|s| {
                s.phase = Phase::Idle;
                s.heard = "(static)".into();
            });
            continue;
        }
        rex.set(|s| s.heard = text.clone());
        let Some(claude) = &claude else {
            rex.say(&who.fill(persona::NO_KEY));
            rex.set(|s| s.phase = Phase::Idle);
            continue;
        };
        if let Err(e) = mem.add("user", &text) {
            eprintln!("rex-static: memory: {e}");
        }
        let notes = mem.notes().map(|n| n.0).unwrap_or_default();
        let turns = mem.recent(12).unwrap_or_default();
        let system = persona::system_prompt(&who, &notes);
        let (tx2, rx2) = mpsc::channel();
        let tool_used = Arc::new(AtomicBool::new(false));
        {
            let tool_used = tool_used.clone();
            let c = claude.clone();
            let tools = tools.clone();
            let shared = rex.shared.clone();
            let said = text.clone();
            thread::spawn(move || {
                let turn = tools.turn(&said);
                let on_tool = |label: &str| {
                    tool_used.store(true, Ordering::Relaxed);
                    if let Ok(mut s) = shared.lock() {
                        s.phase = Phase::Thinking;
                        s.caption = label.to_string();
                    }
                };
                let _ = tx2.send(c.converse(&system, &turns, 2000, Some(&turn), &on_tool));
            });
        }
        // Fast replies get no filler. A slowish one gets a "hmm"; a slow
        // one, or one that needs a tool, gets a "let me check".
        let started = Instant::now();
        let mut said = 0; // 0 nothing, 1 short, 2 long
        let result = loop {
            match rx2.recv_timeout(Duration::from_millis(100)) {
                Ok(r) => break Some(r),
                Err(mpsc::RecvTimeoutError::Timeout) if !rex.cancel.load(Ordering::Relaxed) => {
                    let waited = started.elapsed();
                    let want = if tool_used.load(Ordering::Relaxed) || waited >= LONG_FILLER_AFTER {
                        2
                    } else if waited >= SHORT_FILLER_AFTER {
                        1
                    } else {
                        0
                    };
                    // One "hmm" at most, then one "let me check" at most.
                    if want > said {
                        let clip = fillers.lock().ok().and_then(|f| {
                            let pool: Vec<_> = f[want - 1].iter().filter(|c| c.text != last_filler).cloned().collect();
                            pool.choose(&mut ::rand::rng()).cloned()
                        });
                        said = want;
                        if let Some(c) = clip {
                            last_filler = c.text.clone();
                            let text = c.text.clone();
                            rex.speak(c);
                            rex.set(|s| {
                                s.phase = Phase::Thinking;
                                // Keep a tool label that came in meanwhile.
                                if s.caption == text {
                                    s.caption.clear();
                                }
                            });
                        }
                    }
                }
                Err(_) => break None,
            }
        };
        match result {
            None => {}
            Some(Ok(llm::Reply::Text(t))) => {
                let (renamed, t) = persona::take_rename(&t);
                if let Some(n) = renamed {
                    match mem.set_setting(key, &n) {
                        Ok(()) => who.name = n,
                        Err(e) => eprintln!("rex-static: memory: {e}"),
                    }
                }
                if let Err(e) = mem.add("assistant", &t) {
                    eprintln!("rex-static: memory: {e}");
                }
                rex.say(&t);
            }
            Some(Ok(llm::Reply::Refused)) => rex.say(&who.fill(&pick(persona::REFUSALS))),
            Some(Err(e)) => {
                eprintln!("rex-static: claude: {e}");
                rex.set(|s| s.error = e);
                rex.say(&pick(persona::ERRORS));
            }
        }
        rex.set(|s| s.phase = Phase::Idle);
        maybe_update_notes(claude, &notes_busy, &who);
    }
}

const VERT: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying lowp vec2 uv;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1.0);
    uv = texcoord;
}
"#;

const FRAG: &str = r#"#version 100
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
varying lowp vec2 uv;
uniform sampler2D Texture;
uniform float time;
uniform float glitch;

float h(float n) { return fract(sin(n) * 43758.5453); }

void main() {
    vec2 p = uv;
    float frame = floor(time * 15.0);
    float row = floor(p.y * 200.0);
    float band = step(0.965 - glitch * 0.3, h(floor(p.y * 22.0) + frame * 7.13));
    p.x += band * (h(row + frame) * 0.1 - 0.05) * (0.5 + glitch);
    p.x += sin(p.y * 37.0 + time * 4.0) * 0.0025;
    p.y = fract(p.y + glitch * glitch * 0.04 * sin(time * 50.0));
    float ca = 0.004 + glitch * 0.014;
    vec3 c;
    c.r = texture2D(Texture, p + vec2(ca, 0.0)).r;
    c.g = texture2D(Texture, p).g;
    c.b = texture2D(Texture, p - vec2(ca, 0.0)).b;
    c *= 0.78 + 0.22 * sin(uv.y * 200.0 * 3.14159);
    c += (h(uv.x * 913.1 + uv.y * 7731.7 + frame * 3.7) - 0.5) * (0.11 + glitch * 0.2);
    float bar = fract(time * 0.13);
    c += 0.07 * exp(-pow((uv.y - bar) * 18.0, 2.0));
    vec2 q = uv - 0.5;
    c *= 1.0 - dot(q, q) * 1.3;
    gl_FragColor = vec4(c, 1.0);
}
"#;

fn wrap(text: &str, max_w: f32, size: u16) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        let try_line = if cur.is_empty() { w.to_string() } else { format!("{cur} {w}") };
        if measure_text(&try_line, None, size, 1.0).width > max_w && !cur.is_empty() {
            lines.push(std::mem::replace(&mut cur, w.to_string()));
        } else {
            cur = try_line;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Up to three lines, scrolled to follow `progress` (0..1).
fn draw_caption(text: &str, progress: f32, color: Color) {
    const SIZE: u16 = 28;
    let lines = wrap(text, screen_width() - 24.0, SIZE);
    if lines.is_empty() {
        return;
    }
    let n = lines.len();
    let first = if n <= 3 { 0 } else { ((progress * n as f32) as usize).saturating_sub(1).min(n - 3) };
    let shown = &lines[first..(first + 3).min(n)];
    let h = shown.len() as f32 * 29.0 + 10.0;
    let y0 = screen_height() - h - 6.0;
    draw_rectangle(6.0, y0, screen_width() - 12.0, h, Color::new(0.0, 0.0, 0.0, 0.7));
    for (i, l) in shown.iter().enumerate() {
        draw_text(l, 12.0, y0 + 26.0 + i as f32 * 29.0, SIZE as f32, color);
    }
}

fn conf() -> Conf {
    Conf {
        window_title: config::get().persona.default_name().into(),
        // Logical px: 375 x 1.6 monitor scale = 600 screen pixels.
        window_width: 375,
        window_height: 375,
        window_resizable: true,
        high_dpi: false,
        // Native Wayland, so a left-click drag uses xdg_toplevel.move.
        platform: miniquad::conf::Platform {
            linux_backend: miniquad::conf::LinuxBackend::WaylandWithX11Fallback,
            linux_wm_class: "rex-static",
            // Resize limits in logical px: 450 and 1400 screen pixels at 1.6 scale.
            wayland_min_size: Some((282, 282)),
            wayland_max_size: Some((875, 875)),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[macroquad::main(conf)]
async fn main() {
    let shared = Arc::new(Mutex::new(Shared {
        phase: Phase::Booting,
        caption: "Starting up...".into(),
        heard: String::new(),
        clip: None,
        clip_start: None,
        error: String::new(),
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let level = Arc::new(AtomicU32::new(0));
    let (tx, rx) = mpsc::channel();
    {
        let rex = Rex {
            shared: shared.clone(),
            cancel: cancel.clone(),
            voice: tts::Voice {
                piper: config::get().piper.clone(),
                model: models_dir().join(config::get().persona.voice_file()),
                pitch: config::get().persona.pitch(),
            },
        };
        thread::spawn(move || worker(rex, rx));
    }

    let rt = render_target(200, 200);
    rt.texture.set_filter(FilterMode::Nearest);
    let cam = Camera2D {
        zoom: vec2(2.0 / 200.0, 2.0 / 200.0),
        target: vec2(100.0, 100.0),
        render_target: Some(rt.clone()),
        ..Default::default()
    };
    let crt = load_material(
        ShaderSource::Glsl { vertex: VERT, fragment: FRAG },
        MaterialParams {
            uniforms: vec![
                UniformDesc::new("time", UniformType::Float1),
                UniformDesc::new("glitch", UniformType::Float1),
            ],
            ..Default::default()
        },
    )
    .expect("CRT shader");

    // The persona's head image (a same-named file in the data folder
    // overrides the built-in one), your own face.png, or the procedural
    // mannequin when the config says plastic or an image won't load.
    let cfg = config::get();
    let want_photo = cfg.face == config::FaceChoice::Photo;
    let mut photo = match cfg.face {
        config::FaceChoice::Photo => face::Face::load(&memory::data_dir().join("face.png"), &face::PHOTO),
        config::FaceChoice::Plastic => None,
        config::FaceChoice::Auto => {
            let p = cfg.persona;
            let own = memory::data_dir().join(p.image_file());
            if own.is_file() {
                face::Face::load(&own, p.face_spec())
            } else {
                face::Face::from_bytes(p.image(), p.face_spec())
            }
        }
    };
    let mut dummy = mannequin::Mannequin::new();
    let backdrop = face::backdrop();
    let mut rng = ::rand::rng();
    let mut rec: Option<audio::Recorder> = None;
    let mut typed = String::new();
    let mut pose = Pose::default();
    let mut tick = 0.0f32;
    let mut sway = 1.0f32;
    let mut freeze = 0.0f32;
    let mut glitch = 0.0f32;
    let start = Instant::now();

    loop {
        let t = start.elapsed().as_secs_f32();
        let dt = get_frame_time();
        if is_key_pressed(KeyCode::Escape) {
            if typed.is_empty() {
                break;
            }
            typed.clear();
        }
        let s = shared.lock().map(|s| s.clone()).unwrap_or_else(|e| e.into_inner().clone());

        // Typing. A leading space is push-to-talk, not text.
        while let Some(c) = get_char_pressed() {
            if c.is_control() || (c == ' ' && typed.is_empty()) || rec.is_some() {
                continue;
            }
            if typed.chars().count() < TYPE_MAX {
                typed.push(c);
            }
        }
        if is_key_pressed(KeyCode::Backspace) {
            typed.pop();
        }
        if (is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter)) && s.phase != Phase::Error {
            let msg = typed.trim().to_string();
            typed.clear();
            if !msg.is_empty() {
                cancel.store(true, Ordering::SeqCst);
                let _ = tx.send(Job::Say(msg));
            }
        }

        if is_key_pressed(KeyCode::Space) && typed.is_empty() && rec.is_none() && !matches!(s.phase, Phase::Booting | Phase::Error) {
            cancel.store(true, Ordering::SeqCst);
            match audio::Recorder::start(level.clone()) {
                Ok(r) => rec = Some(r),
                Err(e) => eprintln!("rex-static: mic: {e}"),
            }
        }
        if is_key_released(KeyCode::Space) {
            if let Some(r) = rec.take() {
                let pcm = r.stop();
                if pcm.len() >= audio::MIC_RATE as usize * 4 / 10 {
                    let _ = tx.send(Job::Hear(pcm));
                }
            }
        }

        let mood = if rec.is_some() {
            Mood::Listening
        } else {
            match s.phase {
                Phase::Booting => Mood::Booting,
                Phase::Idle => Mood::Idle,
                Phase::Thinking => Mood::Thinking,
                Phase::Speaking => Mood::Speaking,
                Phase::Error => Mood::Error,
            }
        };

        // Where are we in the current clip?
        let (env, hit, progress) = match (&s.clip, s.clip_start) {
            (Some(c), Some(st)) => {
                let e = st.elapsed().as_secs_f32() - 0.08;
                let i = (e * tts::FPS).max(0.0) as usize;
                let p = (e / c.duration().max(0.1)).clamp(0.0, 1.0);
                (c.env.get(i).copied().unwrap_or(0.0), c.glitch.get(i).copied().unwrap_or(false), p)
            }
            _ => (0.0, false, 1.0),
        };

        // Choppy 12 fps animation with freezes and twitches.
        tick += dt;
        if tick >= 1.0 / 12.0 {
            tick = 0.0;
            if freeze > 0.0 {
                freeze -= 1.0 / 12.0;
            } else {
                if rng.random_bool(0.03) {
                    freeze = rng.random_range(0.15..0.5);
                }
                let talk = mood == Mood::Speaking;
                // Full sway while talking, calmer when not; eased so the
                // switch doesn't jump.
                let target = if talk { 1.0 } else { IDLE_SWAY };
                sway += (target - sway) * 0.15;
                pose.yaw = ((t * 0.7).sin() * 0.32 + (t * 1.9).sin() * 0.06) * sway;
                pose.pitch = (t * 0.53).sin() * 0.1 * sway + if talk { env * 0.1 } else { 0.0 };
                pose.roll = (t * 0.9).sin() * 0.07 * sway;
                pose.dx = (t * 0.4).sin() * 4.0 * sway;
                pose.dy = (t * 0.8).sin() * 2.0 * sway;
                pose.brow = match mood {
                    Mood::Thinking => 0.8,
                    Mood::Listening => 0.5,
                    Mood::Speaking => env * 0.7,
                    _ => 0.1,
                };
                if mood == Mood::Thinking {
                    pose.roll += 0.12;
                    pose.yaw += 0.15;
                }
                if hit || rng.random_bool(if talk { 0.05 } else { 0.02 }) {
                    pose.yaw += rng.random_range(-0.3..0.3) * sway;
                    pose.roll += rng.random_range(-0.15..0.15) * sway;
                    pose.dx += rng.random_range(-8.0..8.0) * sway;
                }
            }
            pose.mouth = env;
        }
        let base = match mood {
            Mood::Thinking => 0.2,
            Mood::Error => 0.5,
            Mood::Booting => 0.35,
            _ => 0.0,
        };
        if hit || rng.random_bool(0.004) {
            glitch = 1.0;
        }
        glitch = (glitch - dt * 3.0).max(base);

        // Draw the low-res broadcast.
        set_camera(&cam);
        face::draw_backdrop(&backdrop);
        match photo.as_mut() {
            Some(f) => {
                face::draw_body(&pose, f.neck, f.collar, f.shirt);
                f.draw(&pose, t);
            }
            None if want_photo => {
                face::draw_body(&pose, Color::new(0.8, 0.62, 0.5, 1.0), Color::new(0.92, 0.9, 0.84, 1.0), face::SHIRT);
                head::draw_head(&pose, mood);
            }
            None => {
                face::draw_body(&pose, mannequin::Mannequin::NECK, mannequin::Mannequin::COLLAR, face::SHIRT);
                dummy.draw(&pose, t);
            }
        }
        let blink = (t * 2.0).fract() < 0.6;
        if rec.is_some() {
            if blink {
                draw_circle(9.0, 9.0, 3.0, RED);
            }
            draw_text("REC", 15.0, 13.0, 12.0, WHITE);
            let lvl = f32::from_bits(level.load(Ordering::Relaxed));
            draw_rectangle(44.0, 6.0, 50.0, 6.0, Color::new(0.0, 0.0, 0.0, 0.6));
            draw_rectangle(44.0, 6.0, 50.0 * lvl, 6.0, GREEN);
        } else {
            let label = match s.phase {
                Phase::Booting => "LOADING",
                Phase::Thinking => "...",
                Phase::Error => "OFFLINE",
                _ => "",
            };
            if s.phase != Phase::Booting || blink {
                draw_text(label, 5.0, 13.0, 12.0, Color::new(1.0, 1.0, 1.0, 0.85));
            }
        }

        set_default_camera();
        clear_background(BLACK);
        crt.set_uniform("time", t);
        crt.set_uniform("glitch", glitch);
        gl_use_material(&crt);
        // Keep the head square if the window isn't.
        let side = screen_width().min(screen_height());
        draw_texture_ex(
            &rt.texture,
            (screen_width() - side) / 2.0,
            (screen_height() - side) / 2.0,
            WHITE,
            DrawTextureParams { dest_size: Some(vec2(side, side)), ..Default::default() },
        );
        gl_use_default_material();

        // Crisp captions on top.
        let cursor = if blink { "_" } else { " " };
        match s.phase {
            _ if !typed.is_empty() => {
                draw_caption(&format!("> {typed}{cursor}"), 1.0, Color::new(1.0, 0.9, 0.4, 1.0))
            }
            Phase::Error => draw_caption(&s.error, 0.0, Color::new(1.0, 0.4, 0.4, 1.0)),
            Phase::Thinking if s.caption.is_empty() && !s.heard.is_empty() => {
                draw_caption(&format!("YOU: {}", s.heard), 1.0, Color::new(0.75, 0.75, 0.8, 1.0))
            }
            _ if rec.is_some() => draw_caption("Listening... let go of Space when done.", 0.0, GREEN),
            _ if !s.caption.is_empty() => draw_caption(&s.caption, progress, WHITE),
            _ => {}
        }

        next_frame().await;
    }
    cancel.store(true, Ordering::SeqCst);
    drop(rec);
    thread::sleep(Duration::from_millis(100));
}
