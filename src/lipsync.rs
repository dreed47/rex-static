//! Mouth shapes over time, from Piper's phoneme timings (or, without them,
//! from how loud the audio is).
//!
//! Each phoneme picks one of eight mouth frames. The targets are laid out at
//! 1 ms, moved a little earlier (the mouth shapes a sound before you hear
//! it), then blurred so each shape flows into the next instead of cutting.

/// Mouth frames, in the order `Clip::mouth` weights them.
#[allow(dead_code)] // Documents the order.
pub const SHAPES: [&str; 8] = ["aa", "ah", "ee", "ih", "oh", "oo", "mm", "ff"];
/// Mouth weights per second.
pub const FPS: f32 = 60.0;
/// How far (ms) the mouth moves ahead of the sound.
const LEAD: usize = 40;
/// Blur between shapes (ms, Gaussian sigma).
const SIGMA: f32 = 35.0;

/// espeak phonemes for each shape, same order as `SHAPES`.
const PHONES: [&str; 8] = ["aɑæɐ", "ɛɜɝʌ", "ij", "ɪəɚᵻtdnlszkɡghθðŋɾx", "oɔɒ", "uʊwɹrʃʒ", "mbp", "fv"];

/// One phoneme: its symbol and its first and end sample in Piper's audio.
pub type Phone = (String, u32, u32);

fn shape_of(ph: &str) -> Option<u8> {
    let c = ph.chars().next()?;
    PHONES.iter().position(|s| s.contains(c)).map(|i| i as u8)
}

/// The shape for each phoneme. Stress marks take the next phoneme's shape;
/// length marks and spaces hold the previous one; anything else rests.
pub fn shapes(phones: &[Phone]) -> Vec<Option<u8>> {
    let mut out: Vec<Option<u8>> = Vec::with_capacity(phones.len());
    for (i, (ph, _, _)) in phones.iter().enumerate() {
        let s = match ph.as_str() {
            "ˈ" | "ˌ" => phones[i + 1..].iter().map(|p| p.0.as_str()).find(|p| !matches!(*p, "ˈ" | "ˌ")).and_then(shape_of),
            "ː" | "ˑ" | " " => out.last().copied().flatten(),
            p => shape_of(p),
        };
        out.push(s);
    }
    out
}

/// Shape (or rest) at sample `at` of Piper's audio.
pub fn shape_at(phones: &[Phone], shapes: &[Option<u8>], at: f32) -> Option<u8> {
    let at = at.max(0.0) as u32;
    let i = phones.partition_point(|p| p.1 <= at);
    let i = i.checked_sub(1)?;
    if at < phones[i].2 { shapes[i] } else { None }
}

/// Shape for one ms of audio from its loudness envelope (0..1): quiet is
/// rest, then a small, half and wide open mouth.
pub fn shape_of_level(env: f32) -> Option<u8> {
    match env {
        e if e < 0.15 => None,
        e if e < 0.4 => Some(3),
        e if e < 0.7 => Some(1),
        _ => Some(0),
    }
}

/// Blurred weights at `FPS` from per-ms targets (`None` = rest).
pub fn weights(targets: &[Option<u8>]) -> Vec<[f32; 8]> {
    let reach = (3.0 * SIGMA) as i64;
    let kernel: Vec<f32> = (-reach..=reach).map(|k| (-0.5 * (k as f32 / SIGMA).powi(2)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    let ms = targets.len() as i64;
    let frames = (ms as f32 / 1000.0 * FPS).ceil() as usize + 1;
    (0..frames)
        .map(|f| {
            let c = (f as f32 * 1000.0 / FPS) as i64 + LEAD as i64;
            let mut w = [0.0f32; 8];
            for (j, &g) in kernel.iter().enumerate() {
                let m = c + j as i64 - reach;
                if (0..ms).contains(&m)
                    && let Some(s) = targets[m as usize]
                {
                    w[s as usize] += g / sum;
                }
            }
            for v in w.iter_mut() {
                if *v < 0.01 {
                    *v = 0.0;
                }
            }
            w
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ph(p: &str, a: u32, b: u32) -> Phone {
        (p.to_string(), a, b)
    }

    #[test]
    fn marks_follow_neighbours() {
        let p = [ph("h", 0, 10), ph("ˈ", 10, 20), ph("ɛ", 20, 30), ph("ː", 30, 40), ph("m", 40, 50), ph(",", 50, 60)];
        assert_eq!(shapes(&p), vec![Some(3), Some(1), Some(1), Some(1), Some(6), None]);
    }

    #[test]
    fn lookup_by_sample() {
        let p = [ph("a", 100, 200), ph("m", 300, 400)];
        let s = shapes(&p);
        assert_eq!(shape_at(&p, &s, 50.0), None);
        assert_eq!(shape_at(&p, &s, 150.0), Some(0));
        assert_eq!(shape_at(&p, &s, 250.0), None);
        assert_eq!(shape_at(&p, &s, 399.0), Some(6));
        assert_eq!(shape_at(&p, &s, 400.0), None);
    }

    #[test]
    fn weights_blend_and_lead() {
        // 300 ms of "a", then 300 ms of "m".
        let t: Vec<Option<u8>> = (0..600).map(|m| Some(if m < 300 { 0 } else { 6 })).collect();
        let w = weights(&t);
        assert_eq!(w.len(), 37);
        // Middle of the "a": all "a".
        assert!(w[9][0] > 0.99);
        // At 300 - LEAD ms the two shapes share the frame.
        let f = ((300 - LEAD) as f32 / 1000.0 * FPS).round() as usize;
        assert!(w[f][0] > 0.3 && w[f][6] > 0.3, "{:?}", w[f]);
        assert!(w.iter().all(|w| w.iter().sum::<f32>() <= 1.0001));
        // Past the end it eases to rest.
        assert!(w[36].iter().sum::<f32>() < 0.2);
    }
}
