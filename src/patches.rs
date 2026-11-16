//! Mouth and eye frames pasted onto a persona's portrait.
//!
//! The frames come from LivePortrait (see `tools/`): the same portrait with
//! only the mouth or the eyes changed, cut down to one small patch per
//! frame. Each patch group shares a mask, so the app mixes the frames of a
//! group by weight and writes the result into the face texture, then draws
//! the face as usual. The nose and cheeks never move or change colour.

use macroquad::prelude::*;

/// Built-in frames: the `faces.json` manifest and the PNGs, mouth frames in
/// `lipsync::SHAPES` order after `rest`, eye frames in `EYES` order after
/// `rest`.
pub struct Files {
    pub json: &'static str,
    pub mouth: [&'static [u8]; 9],
    pub eyes: [&'static [u8]; 6],
}

/// Eye frames after `rest`.
#[allow(dead_code)] // Documents the order.
pub const EYES: [&str; 5] = ["blink06", "blink10", "blink14", "look_l14", "look_r14"];
/// How long a blink takes (s).
pub const BLINK: f32 = 0.17;
/// Eye frame per sixth of a blink: shutting, shut, opening.
const BLINK_STAGES: [usize; 6] = [0, 1, 2, 2, 1, 0];
/// How long a glance holds (s), and how long it takes to get there.
const GLANCE: f32 = 0.9;
const GLANCE_EASE: f32 = 0.12;

/// One group: where it sits, the portrait under it with the rest frame
/// already mixed in (`base`), and each frame's change from rest (`delta`),
/// all premultiplied by the mask.
struct Group {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    base: Vec<f32>,
    delta: Vec<Vec<f32>>,
    alpha: Vec<u8>,
    last: Vec<f32>,
    scratch: Image,
}

impl Group {
    fn new(portrait: &Image, at: (u32, u32), frames: &[&[u8]]) -> Option<Self> {
        let decode = |b: &[u8]| Image::from_file_with_format(b, Some(ImageFormat::Png)).ok();
        let rest = decode(frames[0])?;
        let (w, h) = (rest.width as u32, rest.height as u32);
        if w == 0 || h == 0 || at.0 + w > portrait.width as u32 || at.1 + h > portrait.height as u32 {
            return None;
        }
        let n = (w * h) as usize;
        let mut base = vec![0.0; n * 3];
        let mut alpha = vec![0; n];
        let mut mask = vec![0.0; n];
        for i in 0..n {
            let (px, py) = (at.0 + i as u32 % w, at.1 + i as u32 / w);
            let o = ((py * portrait.width as u32 + px) * 4) as usize;
            let a = rest.bytes[i * 4 + 3] as f32 / 255.0;
            mask[i] = a;
            alpha[i] = portrait.bytes[o + 3];
            for c in 0..3 {
                base[i * 3 + c] = portrait.bytes[o + c] as f32 * (1.0 - a) + rest.bytes[i * 4 + c] as f32 * a;
            }
        }
        let mut delta = Vec::new();
        for f in &frames[1..] {
            let img = decode(f)?;
            if (img.width as u32, img.height as u32) != (w, h) {
                return None;
            }
            let d: Vec<f32> = (0..n * 3)
                .map(|k| mask[k / 3] * (img.bytes[k / 3 * 4 + k % 3] as f32 - rest.bytes[k / 3 * 4 + k % 3] as f32))
                .collect();
            delta.push(d);
        }
        let scratch = Image { bytes: vec![0; n * 4], width: w as u16, height: h as u16 };
        Some(Self { x: at.0, y: at.1, w, h, base, delta, alpha, last: vec![-1.0; frames.len() - 1], scratch })
    }

    /// Mix the frames by `weights` into the texture, if they changed.
    fn apply(&mut self, tex: &Texture2D, weights: &[f32]) {
        if self.last.iter().zip(weights).all(|(a, b)| (a - b).abs() < 1e-3) {
            return;
        }
        self.last.copy_from_slice(weights);
        self.mix(weights);
        tex.update_part(&self.scratch, self.x as i32, self.y as i32, self.w as i32, self.h as i32);
    }

    fn mix(&mut self, weights: &[f32]) {
        let mut acc = self.base.clone();
        for (d, &wt) in self.delta.iter().zip(weights) {
            if wt > 0.0 {
                for (a, v) in acc.iter_mut().zip(d) {
                    *a += wt * v;
                }
            }
        }
        for (i, px) in self.scratch.bytes.chunks_exact_mut(4).enumerate() {
            for c in 0..3 {
                px[c] = acc[i * 3 + c].round().clamp(0.0, 255.0) as u8;
            }
            px[3] = self.alpha[i];
        }
    }
}

pub struct Patches {
    mouth: Group,
    eyes: Group,
    /// Next glance: when it starts and which way (frame index).
    glance_at: f32,
    glance_to: usize,
}

/// Top-left corner of `name` in the manifest.
fn spot(man: &serde_json::Value, name: &str) -> Option<(u32, u32)> {
    let p = man.get("patches")?.get(name)?;
    Some((p.get(0)?.as_u64()? as u32, p.get(1)?.as_u64()? as u32))
}

impl Patches {
    /// Load the frames for `portrait` (the persona's built-in image). `None`
    /// if they don't fit it, so a changed portrait just goes without.
    pub fn load(portrait: &[u8], files: &Files) -> Option<Self> {
        Self::from_image(&Image::from_file_with_format(portrait, None).ok()?, files)
    }

    /// Same as `load`, from the decoded image.
    pub fn from_image(img: &Image, files: &Files) -> Option<Self> {
        let man: serde_json::Value = serde_json::from_str(files.json).ok()?;
        let size = man.get("size")?;
        if (size.get(0)?.as_u64()?, size.get(1)?.as_u64()?) != (img.width as u64, img.height as u64) {
            return None;
        }
        Some(Self {
            mouth: Group::new(img, spot(&man, "mouth_rest")?, &files.mouth)?,
            eyes: Group::new(img, spot(&man, "eyes_rest")?, &files.eyes)?,
            glance_at: ::rand::random_range(3.0..6.0),
            glance_to: 3,
        })
    }

    /// Glance when `other` does, so two views of one face look together.
    pub fn sync_from(&mut self, other: &Self) {
        self.glance_at = other.glance_at;
        self.glance_to = other.glance_to;
    }

    /// Eye weights at time `t`: the blink stage while `blink` (0..1 through
    /// it) is set, otherwise now and then a glance to one side.
    fn eye_weights(&mut self, t: f32, blink: Option<f32>) -> [f32; 5] {
        let mut w = [0.0; 5];
        if let Some(p) = blink {
            w[BLINK_STAGES[((p * 6.0) as usize).min(5)]] = 1.0;
            return w;
        }
        if t > self.glance_at + GLANCE {
            self.glance_at = t + ::rand::random_range(6.0..10.0);
            self.glance_to = if ::rand::random_bool(0.5) { 3 } else { 4 };
        }
        let u = t - self.glance_at;
        if (0.0..GLANCE).contains(&u) {
            w[self.glance_to] = (u / GLANCE_EASE).min((GLANCE - u) / GLANCE_EASE).min(1.0);
        }
        w
    }

    /// Update the face texture for this frame.
    pub fn apply(&mut self, tex: &Texture2D, t: f32, mouth: &[f32; 8], blink: Option<f32>) {
        let eyes = self.eye_weights(t, blink);
        self.mouth.apply(tex, mouth);
        self.eyes.apply(tex, &eyes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One-colour RGBA PNG, so the tests need no image files.
    fn png(w: u16, h: u16, rgba: [u8; 4]) -> Vec<u8> {
        let pixels: Vec<u8> = std::iter::repeat_n(rgba, w as usize * h as usize).flatten().collect();
        let mut out = Vec::new();
        finish(&mut out, w, h, &pixels);
        out
    }

    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }

    fn adler(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &d in data {
            a = (a + d as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc(&body).to_be_bytes());
    }

    fn finish(out: &mut Vec<u8>, w: u16, h: u16, pixels: &[u8]) {
        out.extend(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend((w as u32).to_be_bytes());
        ihdr.extend((h as u32).to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        chunk(out, b"IHDR", &ihdr);
        let mut raw = Vec::new();
        for row in pixels.chunks(w as usize * 4) {
            raw.push(0);
            raw.extend(row);
        }
        // zlib with stored (uncompressed) blocks.
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
        for (i, b) in blocks.iter().enumerate() {
            z.push((i == blocks.len() - 1) as u8);
            z.extend((b.len() as u16).to_le_bytes());
            z.extend((!(b.len() as u16)).to_le_bytes());
            z.extend(*b);
        }
        z.extend(adler(&raw).to_be_bytes());
        chunk(out, b"IDAT", &z);
        chunk(out, b"IEND", &[]);
    }

    fn leak(v: Vec<u8>) -> &'static [u8] {
        Box::leak(v.into_boxed_slice())
    }

    fn files(json: &'static str) -> Files {
        let half = leak(png(4, 2, [100, 100, 100, 128]));
        let white = leak(png(4, 2, [250, 250, 250, 128]));
        Files {
            json,
            mouth: [half, white, half, half, half, half, half, half, half],
            eyes: [half, half, white, half, half, half],
        }
    }

    const JSON: &str = r#"{"size":[8,8],"patches":{"mouth_rest":[2,5],"eyes_rest":[2,1]}}"#;

    #[test]
    fn mixes_by_weight() {
        let portrait = png(8, 8, [0, 0, 0, 255]);
        let mut p = Patches::load(&portrait, &files(JSON)).expect("loads");
        let mut w = [0.0; 8];
        p.mouth.mix(&w);
        // Half mask over black: rest frame at half strength.
        assert!((p.mouth.scratch.bytes[0] as i32 - 50).abs() <= 1, "{}", p.mouth.scratch.bytes[0]);
        assert_eq!(p.mouth.scratch.bytes[3], 255);
        w[0] = 1.0;
        p.mouth.mix(&w);
        assert!((p.mouth.scratch.bytes[0] as i32 - 125).abs() <= 1, "{}", p.mouth.scratch.bytes[0]);
        w[0] = 0.5;
        p.mouth.mix(&w);
        assert!((p.mouth.scratch.bytes[0] as i32 - 88).abs() <= 1, "{}", p.mouth.scratch.bytes[0]);
    }

    #[test]
    fn wrong_size_portrait_goes_without() {
        let portrait = png(10, 8, [0, 0, 0, 255]);
        assert!(Patches::load(&portrait, &files(JSON)).is_none());
        // A patch that runs off the portrait.
        let portrait = png(8, 8, [0, 0, 0, 255]);
        let off = r#"{"size":[8,8],"patches":{"mouth_rest":[6,7],"eyes_rest":[2,1]}}"#;
        assert!(Patches::load(&portrait, &files(off)).is_none());
    }

    #[test]
    fn blink_stages_then_glances() {
        let portrait = png(8, 8, [0, 0, 0, 255]);
        let mut p = Patches::load(&portrait, &files(JSON)).expect("loads");
        assert_eq!(p.eye_weights(0.0, Some(0.0)), [1.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(p.eye_weights(0.0, Some(0.45)), [0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.eye_weights(0.0, Some(0.99)), [1.0, 0.0, 0.0, 0.0, 0.0]);
        p.glance_at = 10.0;
        p.glance_to = 4;
        assert_eq!(p.eye_weights(9.0, None), [0.0; 5]);
        assert_eq!(p.eye_weights(10.45, None)[4], 1.0);
        assert!((p.eye_weights(10.06, None)[4] - 0.5).abs() < 1e-3);
    }
}

#[cfg(test)]
mod builtin {
    use crate::persona::Persona;

    #[test]
    fn every_persona_loads_its_frames() {
        for p in Persona::ALL {
            assert!(super::Patches::load(p.image(), &p.patches()).is_some(), "{}", p.tag());
        }
    }
}
