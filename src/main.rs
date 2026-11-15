//! Rex Static (or Roxy Static): a glitchy, plastic-looking digital person in
//! a small window. Settings come from `~/.config/rex-static/config`.
//! Hold Space to talk, or type and press Enter. Esc clears typing, then quits.

mod audio;
mod config;
mod face;
mod head;
mod lipsync;
mod llm;
mod mannequin;
mod memory;
mod mqtt;
mod patches;
mod persona;
mod prefs;
mod stt;
mod tools;
mod tts;
mod wake;

use ::rand::Rng;
use ::rand::seq::IndexedRandom;
use head::{Mood, Pose};
use macroquad::miniquad::{BlendFactor, BlendState, BlendValue, Equation, PipelineParams};
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
    /// Read out as is (from Home Assistant). Waits for current speech and
    /// stays out of the chat memory.
    Announce(String),
    /// A clip the wake listener heard "hey" in. Answer true to record a
    /// request now.
    Wake(Vec<i16>, mpsc::Sender<bool>),
}

/// Command-line flag: record a request right after the greeting (set when
/// "Hey <name>" switches persona).
const LISTEN_FLAG: &str = "--listen";

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
            Ok((pcm, phones)) => {
                Some(Arc::new(tts::glitchify(&pcm, &phones, text, self.voice.pitch, self.voice.retro, &mut ::rand::rng())))
            }
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
        self.set(|s| {
            s.phase = Phase::Speaking;
            s.caption.clone_from(&clip.text);
            s.clip = Some(clip.clone());
            s.clip_start = None;
        });
        let r = audio::play(&clip.samples, tts::RATE, &self.cancel, || {
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
    let tools = Arc::new(tools::Tools::new(rex.cancel.clone(), cfg.claude_code.clone(), cfg.ha.clone(), cfg.persona, cfg.brief));
    let notes_busy = Arc::new(AtomicBool::new(false));
    let key = cfg.persona.name_key();
    let mut who = persona::Who {
        name: mem.setting(key).unwrap_or_else(|| cfg.persona.default_name().to_string()),
        persona: cfg.persona,
        user: cfg.user_name.clone(),
        tools: tools.set(),
        brief: tools.brief(),
        always: tools.always_labels(),
    };

    // Pre-render filler lines in the background so they play instantly.
    // Index 0 holds the short ones, 1 the long ones.
    let fillers: Arc<Mutex<[Vec<Arc<tts::Clip>>; 2]>> = Arc::default();
    {
        let fillers = fillers.clone();
        let voice = rex.voice.clone();
        thread::spawn(move || {
            for (i, lines) in [persona::SHORT_FILLERS, who.persona.long_fillers()].into_iter().enumerate() {
                for line in lines {
                    if let Ok((pcm, phones)) = tts::synth(&voice, line) {
                        let c = Arc::new(tts::glitchify(&pcm, &phones, line, voice.pitch, voice.retro, &mut ::rand::rng()));
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
    rex.say(&who.fill(&if claude.is_some() { pick(who.persona.greetings()) } else { persona::NO_KEY.to_string() }));
    rex.set(|s| s.phase = Phase::Idle);

    for job in rx {
        rex.cancel.store(false, Ordering::SeqCst);
        if let Job::Announce(text) = job {
            rex.set(|s| s.heard.clear());
            rex.say(&text);
            rex.set(|s| s.phase = Phase::Idle);
            continue;
        }
        // A wake check stays quiet: no "thinking" unless it's a real request.
        let job = match job {
            Job::Wake(pcm, reply) => {
                let names: Vec<_> = persona::Persona::ALL
                    .into_iter()
                    .map(|p| (p, mem.setting(p.name_key()).unwrap_or_else(|| p.default_name().to_string())))
                    .collect();
                let prompt = format!("Hey {}.", names.iter().map(|n| n.1.as_str()).collect::<Vec<_>>().join(", hey "));
                let text = stt.transcribe_with(&pcm, Some(&prompt)).unwrap_or_default();
                if std::env::var_os("REX_WAKE_DEBUG").is_some() {
                    eprintln!("rex-static: wake: heard {text:?}: {:?}", wake::parse(&text, &names));
                }
                match wake::parse(&text, &names) {
                    None => {
                        let _ = reply.send(false);
                        continue;
                    }
                    Some(w) if w.persona != who.persona => {
                        let _ = reply.send(false);
                        match config::save_persona(w.persona) {
                            Ok(()) => restart(true),
                            Err(e) => eprintln!("rex-static: wake: {e}"),
                        }
                        continue;
                    }
                    Some(w) if w.rest < 2 => {
                        let _ = reply.send(true);
                        continue;
                    }
                    // "Hey Miles, what time is it?": the request is in there.
                    Some(_) => {
                        let _ = reply.send(false);
                        Job::Say(text)
                    }
                }
            }
            j => j,
        };
        rex.set(|s| {
            s.phase = Phase::Thinking;
            s.caption.clear();
            s.heard = "...".into();
        });
        let text = match job {
            Job::Say(t) => t,
            Job::Announce(_) | Job::Wake(..) => unreachable!("handled above"),
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
        who.brief = tools.brief();
        who.always = tools.always_labels();
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
                    // Brief mode skips the "hmm" and waits for a tool.
                    let want = if tool_used.load(Ordering::Relaxed) || (!who.brief && waited >= LONG_FILLER_AFTER) {
                        2
                    } else if !who.brief && waited >= SHORT_FILLER_AFTER {
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
                rex.say(&pick(who.persona.errors()));
            }
        }
        if tools.take_switch().is_some() {
            restart(false);
        }
        rex.set(|s| s.phase = Phase::Idle);
        maybe_update_notes(claude, &notes_busy, &who);
    }
}

/// Replace this process with a fresh copy of the program, so a new persona
/// loads from the config. The instance lock is close-on-exec, so it frees
/// itself and the new copy takes it. `listen`: record a request right
/// after the greeting.
fn restart(listen: bool) {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return eprintln!("rex-static: restart: {e}"),
    };
    // If setup.sh replaced the binary while this copy ran, the link reads
    // "<path> (deleted)"; the new file sits at the original path.
    let exe = match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(s) => PathBuf::from(s),
        None => exe,
    };
    let err = std::process::Command::new(&exe).args(restart_args(std::env::args_os().skip(1), listen)).exec();
    eprintln!("rex-static: restart {}: {err}", exe.display());
}

fn restart_args(args: impl Iterator<Item = std::ffi::OsString>, listen: bool) -> Vec<std::ffi::OsString> {
    let mut out: Vec<_> = args.filter(|a| a != LISTEN_FLAG).collect();
    if listen {
        out.push(LISTEN_FLAG.into());
    }
    out
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

/// Lays one turned view over another at a set strength. The render
/// target's own alpha is not 1 everywhere (blended draws lower it), so the
/// shader ignores it and uses the `strength` uniform instead.
const MIX_FRAG: &str = r#"#version 100
precision mediump float;
varying lowp vec2 uv;
uniform sampler2D Texture;
uniform float strength;
void main() {
    gl_FragColor = vec4(texture2D(Texture, uv).rgb, strength);
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
    draw_rectangle(6.0, y0, screen_width() - 12.0, h, Color::new(0.0, 0.0, 0.0, 0.7 * color.a));
    for (i, l) in shown.iter().enumerate() {
        draw_text(l, 12.0, y0 + 26.0 + i as f32 * 29.0, SIZE as f32, color);
    }
}

/// Frame time while something moves fast: talking, listening, typing or a
/// resize. About 60 fps.
const FRAME: Duration = Duration::from_millis(16);
/// Frame time when idle or thinking, to save CPU. The idle sway is slow, so
/// 20 fps looks the same. Retro animates at 12 fps anyway.
const FRAME_IDLE: Duration = Duration::from_millis(50);
const FRAME_IDLE_RETRO: Duration = Duration::from_millis(83);

/// Modern render size: the head's side in real screen pixels, so the
/// compositor shows it 1:1 instead of stretching a fixed-size picture.
/// The persona's built-in turned views, if it has them and they all load.
fn load_views(p: persona::Persona) -> Option<[face::Face; 4]> {
    let img = Image::from_file_with_format(p.image(), None).ok()?;
    let [a, b, c, d] = p.views()?;
    let one = |v: &persona::View| face::turned(&img, v, p.face_spec());
    Some([one(&a)?, one(&b)?, one(&c)?, one(&d)?])
}

/// Draw view `j` of the turn: 0 and 1 face the viewer's left, 2 is the
/// front photo, 3 and 4 face right.
fn draw_view(front: &mut face::Face, views: &mut [face::Face; 4], j: usize, pose: &Pose, t: f32, mouth: &[f32; 8], blink: bool) {
    if j == 2 {
        return face::draw_morph(front, pose, front, pose, 0.0, false, blink);
    }
    let v = &mut views[if j < 2 { j } else { j - 1 }];
    v.animate(t, mouth);
    face::draw_morph(v, pose, v, pose, 0.0, false, blink);
}

fn modern_res() -> u32 {
    let side = screen_width().min(screen_height()) * miniquad::window::dpi_scale();
    (side.round() as u32).clamp(200, 2400)
}

/// A square render target and the 200-unit camera that draws into it.
fn make_target(res: u32, nearest: bool) -> (RenderTarget, Camera2D) {
    let rt = render_target(res, res);
    rt.texture.set_filter(if nearest { FilterMode::Nearest } else { FilterMode::Linear });
    let cam = Camera2D {
        zoom: vec2(2.0 / 200.0, 2.0 / 200.0),
        target: vec2(100.0, 100.0),
        render_target: Some(rt.clone()),
        ..Default::default()
    };
    (rt, cam)
}

fn conf() -> Conf {
    Conf {
        window_title: config::get().persona.default_name().into(),
        // The last size used, in logical px (375 x 1.6 monitor scale = 600
        // screen pixels the first time).
        window_width: saved_size().0,
        window_height: saved_size().1,
        window_resizable: true,
        // Real pixels (buffer scale 2 on a 1.6 monitor, shrunk by the
        // compositor), so the head and captions stay sharp.
        high_dpi: true,
        // Native Wayland, so a left-click drag uses xdg_toplevel.move.
        platform: miniquad::conf::Platform {
            linux_backend: miniquad::conf::LinuxBackend::WaylandWithX11Fallback,
            linux_wm_class: "rex-static",
            // Resize limits in logical px: 250 and 1400 screen pixels at 1.6 scale.
            wayland_min_size: Some((MIN_SIZE, MIN_SIZE)),
            wayland_max_size: Some((MAX_SIZE, MAX_SIZE)),
            // No vsync: with it, eglSwapBuffers blocks while the window is
            // hidden (another workspace), Wayland pings go unanswered and
            // Hyprland shows "not responding". The main loop paces frames.
            swap_interval: Some(0),
            ..Default::default()
        },
        ..Default::default()
    }
}

const MIN_SIZE: i32 = 156;
const MAX_SIZE: i32 = 875;
const DEFAULT_SIZE: i32 = 375;

fn size_path() -> PathBuf {
    memory::data_dir().join("window-size")
}

/// Read "width height" from the size file, kept inside the resize limits.
fn parse_size(text: &str) -> Option<(i32, i32)> {
    let mut it = text.split_whitespace().map(|n| n.parse::<i32>().ok());
    let (w, h) = (it.next()??, it.next()??);
    Some((w.clamp(MIN_SIZE, MAX_SIZE), h.clamp(MIN_SIZE, MAX_SIZE)))
}

/// The window size from the last run, or the default.
fn saved_size() -> (i32, i32) {
    std::fs::read_to_string(size_path())
        .ok()
        .and_then(|t| parse_size(&t))
        .unwrap_or((DEFAULT_SIZE, DEFAULT_SIZE))
}

/// Remember the window size for the next start or persona switch.
fn save_size(w: i32, h: i32) {
    let dir = memory::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("window-size.tmp");
    if std::fs::write(&tmp, format!("{w} {h}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, size_path());
    }
}

/// Ask Hyprland to resize this window, keeping it centred.
fn ask_size((w, h): (i32, i32)) {
    let pid = std::process::id();
    std::thread::spawn(move || {
        let _ = std::process::Command::new("/usr/bin/hyprctl")
            .args([
                "dispatch",
                &format!("hl.dsp.window.resize({{ x = {w}, y = {h}, relative = false, window = \"pid:{pid}\" }})"),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Holds an exclusive lock on a file in the data folder for the life of the
/// process. A second launch finds it taken, focuses the open window and exits.
fn single_instance() -> Option<std::fs::File> {
    let dir = memory::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Can't open lock file ({e}); starting anyway.");
            return None;
        }
    };
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            eprintln!("Rex Static is already running.");
            let _ = std::process::Command::new("/usr/bin/hyprctl")
                .args(["dispatch", "focuswindow", "class:^rex-static$"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Can't lock {} ({e:?}); starting anyway.", dir.display());
            None
        }
    }
}

fn main() {
    let _lock = single_instance();
    macroquad::Window::from_config(conf(), run());
}

async fn run() {
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
    let ctl = Arc::new(wake::Ctl::default());
    ctl.mute.store(true, Ordering::Relaxed);
    if config::get().wake {
        if std::env::args_os().any(|a| a == LISTEN_FLAG) {
            ctl.arm.store(true, Ordering::Relaxed);
        }
        let (ctl, level, tx) = (ctl.clone(), level.clone(), tx.clone());
        let model = models_dir().join(wake::MODEL_DIR);
        thread::spawn(move || {
            let check_tx = tx.clone();
            wake::run(
                ctl,
                level,
                model,
                move |pcm| {
                    let (t, r) = mpsc::channel();
                    check_tx.send(Job::Wake(pcm, t)).ok().map(|_| r)
                },
                move |pcm| {
                    let _ = tx.send(Job::Hear(pcm));
                },
            )
        });
    }
    {
        let rex = Rex {
            shared: shared.clone(),
            cancel: cancel.clone(),
            voice: tts::Voice {
                piper: config::get().piper.clone(),
                model: models_dir().join(config::get().persona.voice_file()),
                pitch: config::get().persona.pitch(),
                pace: config::get().persona.pace(),
                retro: config::get().persona.retro(),
                timed: tts::timed_python(&config::get().piper),
            },
        };
        thread::spawn(move || worker(rex, rx));
    }
    if let Some(conf) = config::get().mqtt.clone() {
        let (atx, arx) = mpsc::channel();
        thread::spawn(move || mqtt::run(conf, atx));
        let tx = tx.clone();
        thread::spawn(move || {
            for text in arx {
                if tx.send(Job::Announce(text)).is_err() {
                    break;
                }
            }
        });
    }

    // Retro personas draw at 200 px and go through the CRT shader; modern
    // ones draw sharp at the window's real pixel size with no glitches.
    let retro = config::get().persona.retro();
    let mut res = if retro { 200 } else { modern_res() };
    let (mut rt, mut cam) = make_target(res, retro);
    // Second target for the next turned view while the head crossfades to it.
    let (mut rt2, mut cam2) = make_target(res, false);
    let mixer = load_material(
        ShaderSource::Glsl { vertex: VERT, fragment: MIX_FRAG },
        MaterialParams {
            uniforms: vec![UniformDesc::new("strength", UniformType::Float1)],
            pipeline_params: PipelineParams {
                color_blend: Some(BlendState::new(
                    Equation::Add,
                    BlendFactor::Value(BlendValue::SourceAlpha),
                    BlendFactor::OneMinusValue(BlendValue::SourceAlpha),
                )),
                // Leave the target's alpha alone, or the screen draw
                // darkens the frame against black.
                alpha_blend: Some(BlendState::new(Equation::Add, BlendFactor::Zero, BlendFactor::One)),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .expect("mix shader");
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
    let mut views: Option<[face::Face; 4]> = None;
    let mut photo = match cfg.face {
        config::FaceChoice::Photo => face::Face::load(&memory::data_dir().join("face.png"), &face::PHOTO),
        config::FaceChoice::Plastic => None,
        config::FaceChoice::Auto => {
            let p = cfg.persona;
            let own = memory::data_dir().join(p.image_file());
            if own.is_file() {
                face::Face::load(&own, p.face_spec())
            } else {
                // Turned views only match the built-in front.
                views = load_views(p);
                face::Face::from_bytes(p.image(), p.face_spec()).map(|mut f| {
                    f.patches = patches::Patches::load(p.image(), &p.patches());
                    f
                })
            }
        }
    };
    if !retro {
        for f in photo.iter_mut().chain(views.iter_mut().flatten()) {
            // The photo carries its own light; no fake sheen on top.
            f.gloss = 0.0;
            f.skin_sides = true;
        }
    }
    let mut dummy = mannequin::Mannequin::new();
    let backdrop = if retro { face::backdrop() } else { face::backdrop_modern() };
    let mut rng = ::rand::rng();
    let mut rec: Option<audio::Recorder> = None;
    let mut typed = String::new();
    let mut pose = Pose::default();
    let mut tick = 0.0f32;
    let mut sway = 1.0f32;
    // Turns through the views: where the head is heading, how far it is,
    // how fast it moves, and when the next change is due. 1 is the far
    // view on the viewer's right, -1 on the left.
    const LOOKS: [f32; 4] = [-1.0, -0.5, 0.5, 1.0];
    // Seconds a caption stays after speech before it starts to fade.
    const CAPTION_HOLD: f32 = 4.0;
    let mut caption_seen = String::new();
    let mut caption_at = 0.0f32;
    let mut look = 0.0f32;
    let mut look_yaw = 0.0f32;
    let mut look_v = 0.0f32;
    let mut look_until = 4.0f32;
    let mut freeze = 0.0f32;
    let mut glitch = 0.0f32;
    let start = Instant::now();

    let mut frame_start = Instant::now();
    // Window size as last saved, and the size waiting to settle before a save.
    let mut size_saved = saved_size();
    let mut size_seen = size_saved;
    let mut size_at = 0.0f32;
    let mut size_asked = false;
    loop {
        let t = start.elapsed().as_secs_f32();
        let dt = get_frame_time();
        // Save the size once a resize settles, so a persona switch (which
        // restarts the program) or the next launch opens at the same size.
        let size = (screen_width().round() as i32, screen_height().round() as i32);
        if !size_asked && t > 0.2 {
            // Hyprland opens a floating window at its smallest size, so ask
            // for the saved one once the window is up.
            size_asked = true;
            if size != size_saved {
                ask_size(size_saved);
            }
        }
        if t < 1.0 {
            // Wait for that resize; don't save the size the window opened at.
        } else if size != size_seen {
            size_seen = size;
            size_at = t;
        } else if size != size_saved && t - size_at > 0.3 {
            save_size(size.0, size.1);
            size_saved = size;
        }
        // Once a resize settles, match the modern render size to it.
        if !retro && t - size_at > 0.3 && modern_res() != res {
            res = modern_res();
            (rt, cam) = make_target(res, false);
            (rt2, cam2) = make_target(res, false);
        }
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

        ctl.mute.store(rec.is_some() || s.phase != Phase::Idle, Ordering::Relaxed);
        let asking = ctl.asking.load(Ordering::Relaxed);
        let listening = rec.is_some() || asking;
        let mood = if listening {
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
        let (env, hit, progress, mouth) = match (&s.clip, s.clip_start) {
            (Some(c), Some(st)) => {
                let e = st.elapsed().as_secs_f32() - 0.08;
                let i = (e * tts::FPS).max(0.0) as usize;
                let m = (e * lipsync::FPS).max(0.0) as usize;
                let p = (e / c.duration().max(0.1)).clamp(0.0, 1.0);
                (
                    c.env.get(i).copied().unwrap_or(0.0),
                    c.glitch.get(i).copied().unwrap_or(false),
                    p,
                    c.mouth.get(m).copied().unwrap_or_default(),
                )
            }
            _ => (0.0, false, 1.0, [0.0; 8]),
        };

        // Retro: choppy 12 fps animation with freezes and twitches.
        // Modern: smooth, every frame.
        tick += dt;
        if !retro || tick >= 1.0 / 12.0 {
            let step = if retro { 1.0 / 12.0 } else { dt };
            tick = 0.0;
            if freeze > 0.0 {
                freeze -= step;
            } else {
                if retro && rng.random_bool(0.03) {
                    freeze = rng.random_range(0.15..0.5);
                }
                let talk = mood == Mood::Speaking;
                // Full sway while talking, calmer when not; eased so the
                // switch doesn't jump.
                let target = if talk { 1.0 } else { IDLE_SWAY };
                sway += (target - sway) * (0.15 * step * 12.0).min(1.0);
                // A flat photo face looks like a mask when it turns far, so
                // the modern personas turn less.
                let turn = if retro { 1.0 } else { 0.6 };
                if views.is_some() {
                    // Turned views: face front with a small drift, now and
                    // then turn to one side, hold, and come back. The views
                    // do the turning; the mesh only drifts.
                    if t >= look_until {
                        let away = look == 0.0;
                        look = if away { LOOKS[rng.random_range(0..4)] } else { 0.0 };
                        look_until = t + if away { rng.random_range(2.5..5.0) } else { rng.random_range(7.0..14.0) };
                    }
                    // Critically damped spring: eases in and out, no overshoot.
                    let k = 4.0;
                    look_v += (k * k * (look - look_yaw) - 2.0 * k * look_v) * step;
                    look_yaw += look_v * step;
                    pose.yaw = ((t * 0.7).sin() * 0.03 + (t * 1.9).sin() * 0.012) * sway;
                } else {
                    pose.yaw = ((t * 0.7).sin() * 0.32 + (t * 1.9).sin() * 0.06) * sway * turn;
                }
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
                    if views.is_none() {
                        pose.yaw += 0.15 * turn;
                    }
                }
                if retro && (hit || rng.random_bool(if talk { 0.05 } else { 0.02 })) {
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
        if retro {
            if hit || rng.random_bool(0.004) {
                glitch = 1.0;
            }
            glitch = (glitch - dt * 3.0).max(base);
        }

        // Draw the low-res broadcast.
        set_camera(&cam);
        face::draw_backdrop(&backdrop);
        match photo.as_mut() {
            Some(f) => {
                f.draw_body(&pose, !retro);
                let blink = f.blink(t);
                f.animate(t, &mouth);
                match views.as_mut() {
                    Some(vs) => {
                        // Views in order: far left, left, front (the photo),
                        // right, far right. Draw the two either side of
                        // the turn and fade the second over the first.
                        let at = (look_yaw * 2.0 + 2.0).clamp(0.0, 4.0);
                        let i = (at.floor() as usize).min(3);
                        let w = at - i as f32;
                        let (a, b) = if w < 0.01 { (i, None) } else if w > 0.99 { (i + 1, None) } else { (i, Some(i + 1)) };
                        for v in vs.iter_mut() {
                            v.next_blink = f.next_blink;
                            if let (Some(vp), Some(fp)) = (v.patches.as_mut(), f.patches.as_ref()) {
                                vp.sync_from(fp);
                            }
                        }
                        draw_view(f, vs, a, &pose, t, &mouth, blink);
                        if let Some(b) = b {
                            set_camera(&cam2);
                            face::draw_backdrop(&backdrop);
                            f.draw_body(&pose, true);
                            draw_view(f, vs, b, &pose, t, &mouth, blink);
                            set_camera(&cam);
                            mixer.set_uniform("strength", w);
                            gl_use_material(&mixer);
                            draw_texture_ex(
                                &rt2.texture,
                                0.0,
                                0.0,
                                WHITE,
                                DrawTextureParams { dest_size: Some(vec2(200.0, 200.0)), ..Default::default() },
                            );
                            gl_use_default_material();
                        }
                    }
                    None => face::draw_morph(f, &pose, f, &pose, 0.0, false, blink),
                }
            }
            None if want_photo => {
                face::draw_body(&pose, Color::new(0.8, 0.62, 0.5, 1.0), Color::new(0.92, 0.9, 0.84, 1.0), face::SHIRT, false);
                head::draw_head(&pose, mood);
            }
            None => {
                face::draw_body(&pose, mannequin::Mannequin::NECK, mannequin::Mannequin::COLLAR, face::SHIRT, false);
                dummy.draw(&pose, t);
            }
        }
        let blink = (t * 2.0).fract() < 0.6;
        if listening {
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
        if retro {
            crt.set_uniform("time", t);
            crt.set_uniform("glitch", glitch);
            gl_use_material(&crt);
        }
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

        // Crisp captions on top. Once she's done talking, a spoken line
        // fades out after a few seconds so it doesn't sit over the face.
        if s.phase != Phase::Idle || s.caption != caption_seen {
            caption_seen.clone_from(&s.caption);
            caption_at = t;
        }
        let caption_fade = 1.0 - face::smoothstep(CAPTION_HOLD, CAPTION_HOLD + 1.5, t - caption_at);
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
            _ if asking => draw_caption("Listening... pause when done.", 0.0, GREEN),
            _ if !s.caption.is_empty() && caption_fade > 0.0 => {
                draw_caption(&s.caption, progress, Color::new(1.0, 1.0, 1.0, caption_fade))
            }
            _ => {}
        }

        // Pace frames ourselves, since vsync is off (see `conf`).
        let busy = listening
            || !typed.is_empty()
            || matches!(s.phase, Phase::Speaking | Phase::Booting)
            || t < 1.5
            || t - size_at < 0.5;
        let frame = match (busy, retro) {
            (true, _) => FRAME,
            (false, false) => FRAME_IDLE,
            (false, true) => FRAME_IDLE_RETRO,
        };
        let spent = frame_start.elapsed();
        if spent < frame {
            thread::sleep(frame - spent);
        }
        frame_start = Instant::now();
        next_frame().await;
    }
    cancel.store(true, Ordering::SeqCst);
    drop(rec);
    thread::sleep(Duration::from_millis(100));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_adds_listen_once() {
        let os = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert_eq!(super::restart_args(os(&["--listen", "x"]).into_iter(), true), os(&["x", "--listen"]));
        assert_eq!(super::restart_args(os(&["--listen"]).into_iter(), false), os(&[]));
    }

    #[test]
    fn size_file_is_in_temp_folder() {
        assert!(size_path().starts_with(std::env::temp_dir()));
    }

    #[test]
    fn size_parses_and_clamps() {
        assert_eq!(parse_size("300 280\n"), Some((300, 280)));
        assert_eq!(parse_size("10 5000"), Some((MIN_SIZE, MAX_SIZE)));
        assert_eq!(parse_size("300"), None);
        assert_eq!(parse_size("wide tall"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn size_round_trips() {
        save_size(250, 260);
        assert_eq!(saved_size(), (250, 260));
        let _ = std::fs::remove_file(size_path());
    }
}
