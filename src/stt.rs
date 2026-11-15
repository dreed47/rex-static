//! Speech to text with whisper.cpp (via whisper-rs).

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const RATE: usize = 16_000;

pub struct Stt {
    ctx: WhisperContext,
}

impl Stt {
    pub fn new(model: &std::path::Path) -> Result<Self, String> {
        whisper_rs::install_logging_hooks(); // no logger installed, so this silences it
        let path = model.to_str().ok_or("model path is not UTF-8")?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| format!("whisper model: {e}"))?;
        Ok(Self { ctx })
    }

    /// 16 kHz mono PCM in, cleaned-up text out (may be empty).
    pub fn transcribe(&self, pcm: &[i16]) -> Result<String, String> {
        self.transcribe_with(pcm, None)
    }

    /// Like [`Self::transcribe`], with a prompt that nudges spelling (names).
    pub fn transcribe_with(&self, pcm: &[i16], prompt: Option<&str>) -> Result<String, String> {
        let mut audio: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
        if audio.len() < RATE + RATE / 10 {
            audio.resize(RATE + RATE / 10, 0.0);
        }
        let mut state = self.ctx.create_state().map_err(|e| e.to_string())?;
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_language(Some("en"));
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        p.set_n_threads(threads.min(8) as i32);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        p.set_no_context(true);
        p.set_suppress_blank(true);
        p.set_suppress_nst(true);
        if let Some(prompt) = prompt {
            p.set_initial_prompt(prompt);
        }
        state.full(p, &audio).map_err(|e| e.to_string())?;
        let mut text = String::new();
        for seg in state.as_iter() {
            if let Ok(s) = seg.to_str_lossy() {
                text.push_str(&s);
            }
        }
        Ok(clean(&text))
    }
}

/// Drop whisper's bracketed noise tags like [BLANK_AUDIO] or (music).
pub fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    let mut star = false;
    for c in s.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            '*' => star = !star,
            _ if depth == 0 && !star => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn clean_tags() {
        assert_eq!(super::clean(" [BLANK_AUDIO] "), "");
        assert_eq!(super::clean(" Hello (music) Rex  "), "Hello Rex");
        assert_eq!(super::clean("*laughs* hi"), "hi");
    }
}
