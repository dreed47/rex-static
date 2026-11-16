//! The photo head: a still photo stretched over a lumpy low-poly mesh and
//! lit like shiny plastic, plus the studio backdrop and the shirt.
//! Coordinates are in the 200×200 render target, y pointing down.

use crate::head::Pose;
use macroquad::math::{EulerRot, Mat3};
use macroquad::prelude::*;
use std::f32::consts::TAU;

const RINGS: usize = 14;
/// 60 keeps the face mesh under macroquad's 5000 indices per draw call.
const SECTORS: usize = 60;
/// Render-target pixels per head unit, and the camera distance.
const SCALE: f32 = 47.0;
const D: f32 = 6.0;
/// How far the jaw drops when the mouth is fully open, in head units.
const JAW: f32 = 0.13;

/// Where things are in one source image. Feature positions are in head
/// units from the face centre; sample points and the outline are in
/// image pixels.
pub struct Spec {
    /// Image pixels per head unit, and the face centre in the image.
    px: f32,
    cx: f32,
    cy: f32,
    /// How flat the face is: 2 gives a round dome, higher keeps the middle
    /// flatter and curves only near the edge.
    dome: f32,
    /// Depth scale. A photo already has its own shading, so a real
    /// portrait needs less relief than the plastic look, or the middle of
    /// the face swells towards the camera.
    relief: f32,
    /// How the side of the head falls away: 0.5 drops straight down at the
    /// rim like a ball, 1 slopes off gently so the cheeks don't end in a
    /// hard ridge.
    edge: f32,
    /// How much the 3D lighting darkens the turned-away parts. A real photo
    /// carries its own shading, so extra shade shows as dark patches.
    shade: f32,
    /// Head outline in image pixels, clockwise from the top.
    outline: &'static [(f32, f32)],
    eyes: [(f32, f32); 2],
    nose: (f32, f32),
    mouth_y: f32,
    mouth_x: (f32, f32),
    hair_at: (f32, f32),
    lid_at: [(f32, f32); 2],
    neck_at: (f32, f32),
    collar_at: (f32, f32),
    shirt: Option<Color>,
    /// The photo shows a real neck and shirt below the chin, so draw those
    /// from the photo instead of a made-up neck and collar.
    body: bool,
}

/// The male head, `assets/rex.png` (611×694).
pub const GUIDE: Spec = Spec {
    px: 236.0,
    cx: 290.0,
    cy: 365.0,
    dome: 2.0,
    relief: 1.0,
    edge: 0.5,
    shade: 1.0,
    outline: &[
        (290.0, 26.0),
        (360.0, 30.0),
        (422.0, 54.0),
        (472.0, 100.0),
        (502.0, 160.0),
        (512.0, 230.0),
        (515.0, 300.0),
        (527.0, 340.0),
        (528.0, 400.0),
        (505.0, 452.0),
        (490.0, 495.0),
        (466.0, 552.0),
        (430.0, 600.0),
        (385.0, 633.0),
        (325.0, 648.0),
        (265.0, 638.0),
        (215.0, 605.0),
        (175.0, 560.0),
        (138.0, 505.0),
        (100.0, 480.0),
        (78.0, 420.0),
        (74.0, 350.0),
        (68.0, 280.0),
        (66.0, 210.0),
        (80.0, 150.0),
        (115.0, 95.0),
        (170.0, 50.0),
        (228.0, 31.0),
    ],
    eyes: [(-0.36, -0.157), (0.36, -0.24)],
    nose: (0.04, 0.23),
    mouth_y: 0.59,
    mouth_x: (-0.19, 0.4),
    hair_at: (290.0, 60.0),
    lid_at: [(203.0, 347.0), (377.0, 327.0)],
    neck_at: (262.0, 668.0),
    collar_at: (495.0, 655.0),
    shirt: Some(Color::new(0.12, 0.13, 0.17, 1.0)),
    body: false,
};

/// The female head, `assets/roxy.png` (508×713), facing forward.
pub const ROXY: Spec = Spec {
    px: 229.0,
    cx: 257.5,
    cy: 354.0,
    dome: 2.0,
    relief: 1.0,
    edge: 0.5,
    shade: 1.0,
    outline: &[
        (257.0, 4.0),
        (330.0, 10.0),
        (395.0, 40.0),
        (440.0, 90.0),
        (462.0, 150.0),
        (470.0, 215.0),
        (462.0, 262.0),
        (462.0, 300.0),
        (450.0, 345.0),
        (425.0, 385.0),
        (408.0, 430.0),
        (385.0, 490.0),
        (355.0, 535.0),
        (310.0, 565.0),
        (258.0, 575.0),
        (205.0, 565.0),
        (160.0, 535.0),
        (130.0, 490.0),
        (108.0, 430.0),
        (90.0, 385.0),
        (62.0, 345.0),
        (50.0, 300.0),
        (52.0, 262.0),
        (45.0, 215.0),
        (55.0, 150.0),
        (78.0, 90.0),
        (120.0, 40.0),
        (185.0, 10.0),
    ],
    eyes: [(-0.36, -0.179), (0.36, -0.179)],
    nose: (0.0, 0.245),
    mouth_y: 0.515,
    mouth_x: (-0.238, 0.251),
    hair_at: (330.0, 40.0),
    lid_at: [(175.0, 338.0), (340.0, 338.0)],
    neck_at: (230.0, 600.0),
    collar_at: (150.0, 650.0),
    shirt: Some(Color::new(0.06, 0.06, 0.1, 1.0)),
    body: false,
};

/// The modern male head, `assets/miles.png` (510×730), facing forward.
pub const MILES: Spec = Spec {
    px: 215.0,
    cx: 255.5,
    cy: 383.5,
    dome: 4.0,
    relief: 0.45,
    edge: 1.0,
    shade: 0.0,
    outline: &[
        (256.0, 66.0),
        (291.0, 69.0),
        (325.0, 81.0),
        (356.0, 97.0),
        (383.0, 118.0),
        (407.0, 142.0),
        (427.0, 169.0),
        (438.0, 201.0),
        (444.0, 233.0),
        (443.0, 266.0),
        (445.0, 292.0),
        (443.0, 318.0),
        (455.0, 338.0),
        (457.0, 361.0),
        (456.0, 384.0),
        (453.0, 406.0),
        (450.0, 428.0),
        (445.0, 450.0),
        (426.0, 466.0),
        (414.0, 483.0),
        (410.0, 507.0),
        (402.0, 530.0),
        (393.0, 555.0),
        (379.0, 580.0),
        (361.0, 603.0),
        (339.0, 622.0),
        (313.0, 636.0),
        (285.0, 644.0),
        (256.0, 648.0),
        (226.0, 645.0),
        (198.0, 637.0),
        (172.0, 623.0),
        (149.0, 604.0),
        (132.0, 581.0),
        (120.0, 553.0),
        (111.0, 528.0),
        (106.0, 503.0),
        (102.0, 480.0),
        (85.0, 465.0),
        (70.0, 449.0),
        (64.0, 427.0),
        (62.0, 405.0),
        (58.0, 384.0),
        (59.0, 361.0),
        (59.0, 339.0),
        (67.0, 317.0),
        (65.0, 292.0),
        (62.0, 262.0),
        (62.0, 229.0),
        (70.0, 198.0),
        (81.0, 165.0),
        (100.0, 136.0),
        (123.0, 107.0),
        (153.0, 90.0),
        (186.0, 77.0),
        (220.0, 70.0),
    ],
    eyes: [(-0.36, -0.179), (0.36, -0.179)],
    nose: (0.0, 0.263),
    mouth_y: 0.62,
    mouth_x: (-0.33, 0.32),
    hair_at: (255.0, 100.0),
    lid_at: [(178.0, 372.0), (333.0, 370.0)],
    neck_at: (255.0, 680.0),
    collar_at: (120.0, 700.0),
    shirt: Some(Color::new(0.07, 0.08, 0.12, 1.0)),
    body: true,
};

/// The modern female head, `assets/clara.png` (510×730), facing forward.
pub const CLARA: Spec = Spec {
    px: 211.0,
    cx: 254.0,
    cy: 351.0,
    dome: 4.0,
    relief: 0.45,
    edge: 1.0,
    shade: 0.0,
    outline: &[
        (254.0, 52.0),
        (288.0, 52.0),
        (320.0, 61.0),
        (350.0, 78.0),
        (375.0, 99.0),
        (397.0, 124.0),
        (414.0, 151.0),
        (425.0, 180.0),
        (433.0, 208.0),
        (436.0, 237.0),
        (438.0, 262.0),
        (437.0, 287.0),
        (448.0, 307.0),
        (447.0, 329.0),
        (441.0, 351.0),
        (438.0, 372.0),
        (432.0, 392.0),
        (424.0, 410.0),
        (402.0, 422.0),
        (399.0, 442.0),
        (395.0, 463.0),
        (388.0, 485.0),
        (377.0, 505.0),
        (370.0, 536.0),
        (352.0, 555.0),
        (331.0, 572.0),
        (307.0, 583.0),
        (281.0, 590.0),
        (254.0, 592.0),
        (227.0, 590.0),
        (201.0, 582.0),
        (177.0, 570.0),
        (157.0, 552.0),
        (140.0, 532.0),
        (132.0, 503.0),
        (122.0, 483.0),
        (116.0, 461.0),
        (112.0, 440.0),
        (106.0, 422.0),
        (86.0, 410.0),
        (79.0, 391.0),
        (74.0, 371.0),
        (72.0, 351.0),
        (66.0, 330.0),
        (65.0, 308.0),
        (73.0, 288.0),
        (72.0, 263.0),
        (71.0, 236.0),
        (75.0, 208.0),
        (82.0, 179.0),
        (94.0, 150.0),
        (111.0, 123.0),
        (132.0, 98.0),
        (158.0, 75.0),
        (188.0, 60.0),
        (220.0, 51.0),
    ],
    eyes: [(-0.36, -0.179), (0.36, -0.179)],
    nose: (0.0, 0.28),
    mouth_y: 0.6,
    mouth_x: (-0.3, 0.32),
    hair_at: (180.0, 100.0),
    lid_at: [(178.0, 340.0), (330.0, 340.0)],
    neck_at: (254.0, 620.0),
    collar_at: (150.0, 680.0),
    shirt: Some(Color::new(0.07, 0.08, 0.12, 1.0)),
    body: true,
};

pub(crate) fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::new(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1.0)
}

pub(crate) fn scale(c: Color, k: f32) -> Color {
    Color::new((c.r * k).min(1.0), (c.g * k).min(1.0), (c.b * k).min(1.0), c.a)
}

/// Outline radius in direction `ang` (any range), interpolated around the
/// trace. `rim` is (angle, radius) sorted by angle.
pub(crate) fn rim_at(rim: &[(f32, f32)], ang: f32) -> f32 {
    let n = rim.len();
    let a = rim[0].0 + (ang - rim[0].0).rem_euclid(TAU);
    for i in 0..n {
        let (a0, r0) = rim[i];
        let (mut a1, r1) = rim[(i + 1) % n];
        if i + 1 == n {
            a1 += TAU;
        }
        if a >= a0 && a <= a1 {
            return r0 + (r1 - r0) * (a - a0) / (a1 - a0).max(1e-4);
        }
    }
    rim[0].1
}

fn trace(spec: &Spec) -> Vec<(f32, f32)> {
    let mut rim: Vec<(f32, f32)> = spec
        .outline
        .iter()
        .map(|&(px, py)| {
            let v = vec2((px - spec.cx) / spec.px, (py - spec.cy) / spec.px);
            (v.y.atan2(v.x), v.length())
        })
        .collect();
    rim.sort_by(|a, b| a.0.total_cmp(&b.0));
    rim
}

impl Spec {
    fn mouth_mid(&self) -> f32 {
        (self.mouth_x.0 + self.mouth_x.1) / 2.0
    }

    /// Nose, brow, cheeks and lips pushed out of an otherwise smooth dome.
    fn bumps(&self, x: f32, y: f32) -> f32 {
        let g = |cx: f32, cy: f32, sx: f32, sy: f32| (-((x - cx) / sx).powi(2) - ((y - cy) / sy).powi(2)).exp();
        let (nx, ny) = self.nose;
        let [e0, e1] = self.eyes;
        let brow = (e0.1 + e1.1) / 2.0 - 0.19;
        let mm = self.mouth_mid();
        0.30 * g(nx, ny, 0.09, 0.08)
            + 0.16 * g(nx, ny - 0.17, 0.06, 0.17)
            + 0.08 * (g(nx - 0.12, ny + 0.03, 0.06, 0.05) + g(nx + 0.12, ny + 0.03, 0.06, 0.05))
            - 0.12 * (g(e0.0, e0.1, 0.14, 0.09) + g(e1.0, e1.1, 0.14, 0.09))
            + 0.07 * g(nx, brow, 0.55, 0.07)
            + 0.10 * g(mm - 0.02, self.mouth_y - 0.06, 0.25, 0.1)
            + 0.06 * g(mm - 0.08, self.mouth_y + 0.3, 0.2, 0.08)
            + 0.06 * (g(nx - 0.42, ny - 0.12, 0.15, 0.12) + g(nx + 0.42, ny - 0.12, 0.15, 0.12))
    }

    /// How much of the jaw drop a point gets: everything below the mouth
    /// line, fading out towards the ears.
    fn jaw_x(&self, x: f32) -> f32 {
        1.0 - smoothstep(0.2, 0.55, (x - self.mouth_mid()).abs())
    }

    fn jaw(&self, x: f32, y: f32) -> f32 {
        smoothstep(self.mouth_y - 0.03, self.mouth_y + 0.03, y) * self.jaw_x(x)
    }
}

pub(crate) struct View {
    pub(crate) rot: Mat3,
    center: Vec2,
}

impl View {
    const PIVOT: Vec3 = vec3(0.0, 0.1, -0.6);

    pub(crate) fn new(pose: &Pose) -> Self {
        Self {
            rot: Mat3::from_euler(EulerRot::YXZ, pose.yaw, pose.pitch, pose.roll),
            center: vec2(100.0 + pose.dx, 86.0 + pose.dy),
        }
    }
    pub(crate) fn world(&self, p: Vec3) -> Vec3 {
        self.rot * (p - Self::PIVOT) + Self::PIVOT
    }
    pub(crate) fn screen(&self, w: Vec3) -> Vec2 {
        self.center + vec2(w.x, w.y) * SCALE * (D / (D - w.z))
    }
}

pub struct Face {
    spec: &'static Spec,
    tex: Texture2D,
    size: Vec2,
    /// (angle, radius) of the outline in head units, sorted by angle.
    rim: Vec<(f32, f32)>,
    hair: Color,
    lid: [Color; 2],
    pub neck: Color,
    pub collar: Color,
    pub shirt: Color,
    /// How shiny the skin is: 1 for the glossy plastic look, lower for a
    /// softer, more natural one.
    pub gloss: f32,
    /// Skin-coloured sides behind the cheeks instead of hair all round, so
    /// a turned head doesn't show a dark edge below the hairline.
    pub skin_sides: bool,
    /// When the next blink starts; copied between views so they blink together.
    pub next_blink: f32,
    /// The neck and shirt cut out of the photo, and the image row the
    /// cut-out starts at; `None` when the photo has no usable body.
    body: Option<(Texture2D, f32)>,
    /// LivePortrait mouth and eye frames pasted into `tex`; with them the
    /// front view skips the warped jaw, mouth gap and painted lids.
    pub patches: Option<crate::patches::Patches>,
}

fn patch(img: &Image, x: f32, y: f32) -> Color {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = (x as i32 + dx).clamp(0, w - 1);
            let py = (y as i32 + dy).clamp(0, h - 1);
            let c = img.get_pixel(px as u32, py as u32);
            sum[0] += c.r;
            sum[1] += c.g;
            sum[2] += c.b;
            n += 1.0;
        }
    }
    Color::new(sum[0] / n, sum[1] / n, sum[2] / n, 1.0)
}

/// The lowest row of the head outline, in image pixels.
fn chin_row(s: &Spec) -> f32 {
    s.outline.iter().map(|p| p.1).fold(0.0f32, f32::max)
}

/// Cut the neck and shirt out of the photo: everything from just above the
/// chin down, with the white background and the face itself see-through.
/// Returns the cut-out and the image row it starts at.
/// Photo rows mirrored below the bottom of a body photo.
const BODY_EXT: u32 = 160;

fn body_cutout(img: &Image, s: &Spec) -> Option<(Texture2D, f32)> {
    let (w, h) = (img.width() as u32, img.height() as u32);
    // Start at the mouth, not just above the chin: the neck in the photo
    // reaches up beside the jaw, and a cut lower than that left a gap
    // between the jaw and the neck. The face draws over the rest.
    // `mouth_y` is in head units, so turn it into an image row first.
    let mouth = s.cy + s.mouth_y * s.px;
    let top = mouth.min(chin_row(s) - 50.0).max(0.0) as u32;
    if top + 16 >= h {
        return None;
    }
    let rim = trace(s);
    let inside = |x: u32, y: u32| {
        let v = vec2((x as f32 - s.cx) / s.px, (y as f32 - s.cy) / s.px);
        v.length() < rim_at(&rim, v.y.atan2(v.x))
    };
    // The photo stops above the frame bottom. Mirror its last rows below it
    // (at half speed, so they stay below the chin) and the shirt carries
    // on with real texture and no fade line.
    let ext = BODY_EXT.min(h - top);
    let mut out = Image::gen_image_color(w as u16, (h + ext - top) as u16, Color::new(0.0, 0.0, 0.0, 0.0));
    for x in 0..w {
        // Under the face, carry the neck straight up from just below the
        // jaw, so a turned head never shows a gap under the chin.
        let below = (top..h).rev().find(|&y| inside(x, y)).map_or(top, |y| (y + 3).min(h - 1));
        for y in top..h + ext {
            let y0 = if y < h { y } else { h - 1 - (y - h) / 2 };
            // Only behind the face: beside the jaw keep the photo as it is.
            let c = img.get_pixel(x, if inside(x, y0) { y0.max(below) } else { y0 });
            let lo = c.r.min(c.g).min(c.b);
            let hi = c.r.max(c.g).max(c.b);
            let backdrop = smoothstep(0.62, 0.8, lo) * (1.0 - smoothstep(0.06, 0.12, hi - lo));
            out.set_pixel(x, y - top, Color::new(c.r, c.g, c.b, 1.0 - backdrop));
        }
    }
    let tex = Texture2D::from_image(&out);
    tex.set_filter(FilterMode::Linear);
    Some((tex, top as f32))
}

/// A turned view of a built-in portrait: the view's face crop pasted over
/// `portrait`, with its own mouth and eye frames. `None` if the files don't
/// fit the portrait. The neck and shirt stay the front photo's.
pub fn turned(portrait: &Image, v: &crate::persona::View, spec: &'static Spec) -> Option<Face> {
    let img = turned_image(portrait, v)?;
    let mut f = Face::from_image(&img, spec, false)?;
    f.rim = follow_edge(&f.rim, portrait, &img, spec);
    f.patches = Some(crate::patches::Patches::from_image(&img, &v.files)?);
    Some(f)
}

/// `rim` (traced on `front`) moved to where the head's edge sits in
/// `turned`: along each direction, the outline shifts by however far the
/// edge against the white backdrop moved. Directions with no backdrop edge
/// (chin into neck) keep their radius.
fn follow_edge(rim: &[(f32, f32)], front: &Image, turned: &Image, spec: &Spec) -> Vec<(f32, f32)> {
    let edge = |img: &Image, ang: f32, r: f32| -> Option<f32> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let (dx, dy) = (ang.cos(), ang.sin());
        let mut run = 0;
        let mut px = r * spec.px * 0.8;
        while px < r * spec.px * 1.25 {
            let (x, y) = (spec.cx + dx * px, spec.cy + dy * px);
            if x < 0.0 || y < 0.0 || x >= w || y >= h {
                return None;
            }
            let c = img.get_pixel(x as u32, y as u32);
            if c.r.min(c.g).min(c.b) > 0.88 {
                run += 1;
                if run == 3 {
                    return Some((px - 2.0) / spec.px);
                }
            } else {
                run = 0;
            }
            px += 1.0;
        }
        None
    };
    let shift: Vec<f32> = rim
        .iter()
        .map(|&(a, r)| match (edge(front, a, r), edge(turned, a, r)) {
            (Some(e0), Some(e1)) => e1 - e0,
            _ => 0.0,
        })
        .collect();
    let n = shift.len();
    rim.iter()
        .enumerate()
        .map(|(i, &(a, r))| {
            let s = (shift[(i + n - 1) % n] + 2.0 * shift[i] + shift[(i + 1) % n]) / 4.0;
            (a, r + s)
        })
        .collect()
}

/// `portrait` with the view's face crop pasted over it.
pub fn turned_image(portrait: &Image, v: &crate::persona::View) -> Option<Image> {
    let man: serde_json::Value = serde_json::from_str(v.files.json).ok()?;
    let at = man.get("patches")?.get("face")?;
    let (x0, y0) = (at.get(0)?.as_u64()? as u32, at.get(1)?.as_u64()? as u32);
    let crop = Image::from_file_with_format(v.face, Some(ImageFormat::Png)).ok()?;
    let (cw, ch) = (crop.width() as u32, crop.height() as u32);
    if x0 + cw > portrait.width() as u32 || y0 + ch > portrait.height() as u32 {
        return None;
    }
    let mut img = portrait.clone();
    for y in 0..ch {
        for x in 0..cw {
            let c = crop.get_pixel(x, y);
            if c.a > 0.0 {
                let b = img.get_pixel(x0 + x, y0 + y);
                img.set_pixel(x0 + x, y0 + y, mix(b, c, c.a));
            }
        }
    }
    Some(img)
}

impl Face {
    /// Build from encoded image bytes (PNG or JPEG).
    pub fn from_bytes(bytes: &[u8], spec: &'static Spec) -> Option<Self> {
        Self::from_image(&Image::from_file_with_format(bytes, None).ok()?, spec, spec.body)
    }

    /// Build from a decoded image; `body` cuts the neck and shirt out of it.
    pub fn from_image(img: &Image, spec: &'static Spec, body: bool) -> Option<Self> {
        if img.width() < 64 || img.height() < 64 {
            return None;
        }
        let tex = Texture2D::from_image(img);
        tex.set_filter(FilterMode::Linear);
        let at = |p: (f32, f32)| patch(img, p.0, p.1);
        Some(Self {
            spec,
            tex,
            size: vec2(img.width() as f32, img.height() as f32),
            rim: trace(spec),
            hair: at(spec.hair_at),
            gloss: 1.0,
            skin_sides: false,
            lid: [at(spec.lid_at[0]), at(spec.lid_at[1])],
            neck: scale(at(spec.neck_at), 0.8),
            collar: at(spec.collar_at),
            shirt: spec.shirt.unwrap_or(SHIRT),
            next_blink: 2.0,
            body: if body { body_cutout(img, spec) } else { None },
            patches: None,
        })
    }

    fn rim_at(&self, ang: f32) -> f32 {
        rim_at(&self.rim, ang)
    }

    fn depth(&self, x: f32, y: f32) -> f32 {
        let r = ((x * x + y * y).sqrt() / self.rim_at(y.atan2(x))).min(1.0);
        let z = 0.9 * (1.0 - r.powf(self.spec.dome)).max(0.0).powf(self.spec.edge) + self.spec.bumps(x, y) * (1.0 - smoothstep(0.75, 1.0, r));
        z * self.spec.relief
    }

    fn normal(&self, x: f32, y: f32) -> Vec3 {
        let e = 0.02;
        let zx = (self.depth(x + e, y) - self.depth(x - e, y)) / (2.0 * e);
        let zy = (self.depth(x, y + e) - self.depth(x, y - e)) / (2.0 * e);
        vec3(-zx, -zy, 1.0).normalize()
    }

    /// A point on the face surface (head units), sitting `lift` above it.
    fn at(&self, view: &View, x: f32, y: f32, lift: f32) -> Vec2 {
        view.screen(view.world(vec3(x, y, self.depth(x, y) + lift)))
    }

    /// Neck and shirt. Photos that show their own draw them from the photo,
    /// on plain shoulders that fill out to the frame edge; the others get
    /// the drawn neck and collar.
    pub fn draw_body(&self, pose: &Pose, soft: bool) {
        let Some((tex, top)) = &self.body else {
            draw_body(pose, self.neck, self.collar, self.shirt, soft);
            return;
        };
        let shift = pose.dx * 0.3 + pose.yaw * 4.0;
        draw_shoulders(shift, self.shirt);
        let s = self.spec;
        let view = View::new(pose);
        let rest = View::new(&Pose::default());
        let (w, h) = (self.size.x, self.size.y);
        let bottom = top + tex.height();
        let chin = chin_row(s);
        let mut xs = vec![0.0, 12.0];
        xs.extend((1..8).map(|i| w * i as f32 / 8.0));
        xs.extend([w - 12.0, w]);
        let mut ys = Vec::new();
        let mut y = *top;
        while y < bottom - 24.0 {
            ys.push(y);
            y += 8.0;
        }
        ys.extend([bottom - 24.0, bottom]);
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let n = xs.len() as u16;
        for (i, &y) in ys.iter().enumerate() {
            // The top of the neck goes with the head, the bottom stays with
            // the shoulders.
            let k = 1.0 - smoothstep(chin, h, y);
            let hy = (y - s.cy) / s.px;
            for (j, &x) in xs.iter().enumerate() {
                let p = vec3((x - s.cx) / s.px, hy, 0.0);
                let still = rest.screen(rest.world(p)) + vec2(shift, 0.0);
                let q = still.lerp(view.screen(view.world(p)), k);
                // Fade at the frame edges, where the drawn shoulders carry on.
                let a = if j == 0 || j == xs.len() - 1 { 0.0 } else { 1.0 };
                vertices.push(Vertex::new(q.x, q.y, 0.0, x / w, (y - top) / (bottom - top), Color::new(1.0, 1.0, 1.0, a)));
            }
            if i > 0 {
                let (o, c) = ((i as u16 - 1) * n, i as u16 * n);
                for j in 0..n - 1 {
                    indices.extend_from_slice(&[o + j, o + j + 1, c + j + 1, o + j, c + j + 1, c + j]);
                }
            }
        }
        draw_mesh(&Mesh { vertices, indices, texture: Some(tex.clone()) });
    }

    /// Start or end a blink; true while the lids are down.
    pub fn blink(&mut self, t: f32) -> bool {
        if t > self.next_blink + crate::patches::BLINK {
            self.next_blink = t + ::rand::random_range(2.5..6.0);
        }
        t >= self.next_blink
    }

    /// Paste this frame's mouth and eye frames into the photo, if it has
    /// them. Call after `blink`.
    pub fn animate(&mut self, t: f32, mouth: &[f32; 8]) {
        let u = t - self.next_blink;
        let blink = (0.0..crate::patches::BLINK).contains(&u).then(|| u / crate::patches::BLINK);
        if let Some(p) = self.patches.as_mut() {
            p.apply(&self.tex, t, mouth, blink);
        }
    }
}

/// Draw one of two photos of the same head on a shared shape: every point
/// sits between where photo `a` (posed `pa`) and photo `b` (posed `pb`) put
/// it, `w` of the way to `b`. Drawing both with the same `w` and fading one
/// over the other morphs between them with no double outline.
pub fn draw_morph(a: &Face, pa: &Pose, b: &Face, pb: &Pose, w: f32, show_b: bool, blink: bool) {
    if a.patches.is_some() && !show_b {
        // The frames already show the mouth and the eyes.
        Pair::new(a, pa, b, pb, w).render(false, 0.0, false);
    } else {
        Pair::new(a, pa, b, pb, w).render(show_b, pa.mouth, blink);
    }
}

/// Feature points that should line up between two photos.
fn anchors(s: &Spec) -> [Vec2; 5] {
    [
        vec2(s.eyes[0].0, s.eyes[0].1),
        vec2(s.eyes[1].0, s.eyes[1].1),
        vec2(s.nose.0, s.nose.1),
        vec2(s.mouth_x.0, s.mouth_y),
        vec2(s.mouth_x.1, s.mouth_y),
    ]
}

const RBF: f32 = 0.12;

fn rbf(d2: f32) -> f32 {
    (-d2 / RBF).exp()
}

/// Solve the small dense system `m x = v` in place (Gaussian elimination
/// with partial pivoting).
fn solve<const N: usize>(mut m: [[f32; N]; N], mut v: [Vec2; N]) -> [Vec2; N] {
    for c in 0..N {
        let p = (c..N).max_by(|&i, &j| m[i][c].abs().total_cmp(&m[j][c].abs())).unwrap_or(c);
        m.swap(c, p);
        v.swap(c, p);
        let d = m[c][c];
        if d.abs() < 1e-6 {
            continue;
        }
        for r in 0..N {
            if r != c {
                let k = m[r][c] / d;
                for j in 0..N {
                    m[r][j] -= k * m[c][j];
                }
                v[r] -= v[c] * k;
            }
        }
    }
    let mut x = [Vec2::ZERO; N];
    for i in 0..N {
        x[i] = if m[i][i].abs() < 1e-6 { Vec2::ZERO } else { v[i] / m[i][i] };
    }
    x
}

/// Carries points on one photo's face to the matching points on another's:
/// a stretch so the outlines meet, plus a smooth field (RBF weights and a
/// constant) that lands the eyes, nose and mouth corners on each other.
struct Warp {
    rim_a: Vec<(f32, f32)>,
    rim_b: Vec<(f32, f32)>,
    pts: [Vec2; 5],
    coef: [Vec2; 6],
}

impl Warp {
    fn new(a: &Spec, b: &Spec) -> Self {
        let mut warp = Self { rim_a: trace(a), rim_b: trace(b), pts: anchors(a), coef: [Vec2::ZERO; 6] };
        let fb = anchors(b);
        let mut m = [[0.0f32; 6]; 6];
        let mut v = [Vec2::ZERO; 6];
        for i in 0..5 {
            for j in 0..5 {
                m[i][j] = rbf((warp.pts[i] - warp.pts[j]).length_squared());
            }
            m[i][5] = 1.0;
            m[5][i] = 1.0;
            v[i] = fb[i] - warp.radial(warp.pts[i]);
        }
        warp.coef = solve(m, v);
        warp
    }

    /// Stretch along each direction so `a`'s outline lands on `b`'s.
    fn radial(&self, p: Vec2) -> Vec2 {
        let ang = p.y.atan2(p.x);
        p * (rim_at(&self.rim_b, ang) / rim_at(&self.rim_a, ang))
    }

    /// The point on `b` that matches point `p` on `a` (both head units).
    fn map(&self, p: Vec2) -> Vec2 {
        let r = p.length() / rim_at(&self.rim_a, p.y.atan2(p.x));
        let mut d = self.coef[5];
        for i in 0..5 {
            d += self.coef[i] * rbf((p - self.pts[i]).length_squared());
        }
        // Features move; the outline stays on the outline.
        let q = self.radial(p) + d * (1.0 - smoothstep(0.75, 1.0, r));
        // Moving the features can push points near the far cheek past `b`'s
        // outline, onto the photo's background. Keep them inside it.
        let edge = rim_at(&self.rim_b, q.y.atan2(q.x)) * (0.94 + 0.06 * smoothstep(0.9, 1.0, r));
        let len = q.length();
        if len > edge { q * (edge / len) } else { q }
    }
}

struct Pair<'a> {
    a: &'a Face,
    b: &'a Face,
    va: View,
    vb: View,
    w: f32,
    warp: Option<Warp>,
}

impl<'a> Pair<'a> {
    fn new(a: &'a Face, pa: &Pose, b: &'a Face, pb: &Pose, w: f32) -> Self {
        let warp = (w > 0.0).then(|| Warp::new(a.spec, b.spec));
        Self { a, b, va: View::new(pa), vb: View::new(pb), w, warp }
    }

    fn to_b(&self, p: Vec2) -> Vec2 {
        self.warp.as_ref().map_or(p, |m| m.map(p))
    }

    /// Screen position and depth of `p` (on `a`) and its match `q` (on `b`).
    fn place(&self, p: Vec2, q: Vec2, lift: f32, open: f32) -> (Vec2, f32) {
        let one = |f: &Face, v: &View, p: Vec2| {
            let w = v.world(vec3(p.x, p.y + open * JAW * f.spec.jaw(p.x, p.y), f.depth(p.x, p.y) + lift));
            (v.screen(w), w.z)
        };
        let (sa, za) = one(self.a, &self.va, p);
        if self.w <= 0.0 {
            return (sa, za);
        }
        let (sb, zb) = one(self.b, &self.vb, q);
        (sa.lerp(sb, self.w), za + (zb - za) * self.w)
    }

    /// A surface point with no jaw drop: `p` on `a`, `q` on `b`.
    fn at(&self, p: Vec2, q: Vec2, lift: f32) -> Vec2 {
        let sa = self.a.at(&self.va, p.x, p.y, lift);
        if self.w <= 0.0 {
            return sa;
        }
        sa.lerp(self.b.at(&self.vb, q.x, q.y, lift), self.w)
    }

    fn render(&self, show_b: bool, open: f32, blink: bool) {
        let open = open.clamp(0.0, 1.0);
        let me = if show_b { self.b } else { self.a };
        let light = vec3(-0.35, -0.5, 0.8).normalize();
        let half = (light + vec3(0.0, 0.0, 1.0)).normalize();
        let shadow = scale(me.neck, 0.6);
        let w = self.w;
        let lerp = |x: f32, y: f32| x + (y - x) * w;

        // Back of the head: a flat plate so turning doesn't show the backdrop.
        // Its depth follows the face relief, so a flatter face doesn't leave
        // a deep gap for the plate to show through when the head turns.
        let plate = |ang: f32, k: f32, z: f32| {
            let dir = vec2(ang.cos(), ang.sin());
            let sa = self.va.screen(self.va.world(vec3(0.0, -0.1, 0.0) * (1.0 - k) + (dir * self.a.rim_at(ang) * 0.98 * k).extend(z * self.a.spec.relief)));
            let sb = self.vb.screen(self.vb.world(vec3(0.0, -0.1, 0.0) * (1.0 - k) + (dir * self.b.rim_at(ang) * 0.98 * k).extend(z * self.b.spec.relief)));
            sa.lerp(sb, w)
        };
        let mut back = Mesh { vertices: Vec::with_capacity(SECTORS + 1), indices: Vec::new(), texture: None };
        let c = plate(0.0, 0.0, -0.55);
        back.vertices.push(Vertex::new(c.x, c.y, 0.0, 0.0, 0.0, me.hair));
        for a in 0..SECTORS {
            let ang = a as f32 / SECTORS as f32 * TAU;
            let s = plate(ang, 1.0, -0.35);
            let dy = ang.sin();
            let col = if me.skin_sides {
                mix(me.hair, scale(me.neck, 0.95), smoothstep(-0.5, -0.2, dy))
            } else {
                mix(me.hair, shadow, smoothstep(0.1, 0.6, dy))
            };
            back.vertices.push(Vertex::new(s.x, s.y, 0.0, 0.0, 0.0, col));
            let next = 1 + (a as u16 + 1) % SECTORS as u16;
            back.indices.extend_from_slice(&[0, 1 + a as u16, next]);
        }
        draw_mesh(&back);

        // The face: rings around the nose, textured with the photo.
        let mut skin = Vec::with_capacity(1 + RINGS * SECTORS);
        let mut gloss = Vec::with_capacity(1 + RINGS * SECTORS);
        let mut zs = Vec::with_capacity(1 + RINGS * SECTORS);
        let lit = |f: &Face, v: &View, p: Vec2| {
            let n = v.rot * f.normal(p.x, p.y);
            let b = 1.0 - f.spec.shade * (1.0 - (0.62 + 0.45 * n.dot(light).max(0.0)).min(1.0));
            // Less shine on the hair, so it stays dark.
            let sp = n.dot(half).max(0.0).powf(24.0) * (0.35 + 0.65 * smoothstep(-0.55, -0.3, p.y));
            (b, sp)
        };
        // `r` is how far out the ring is; the outer band samples the photo
        // slightly inside the outline, so the background just past the
        // traced edge never shows on a turned cheek.
        let mut vert = |p: Vec2, r: f32| {
            let q = self.to_b(p);
            let (s, z) = self.place(p, q, 0.0, open);
            let (mut b, mut sp) = lit(self.a, &self.va, p);
            if w > 0.0 {
                let (b2, sp2) = lit(self.b, &self.vb, q);
                b = lerp(b, b2);
                sp = lerp(sp, sp2);
            }
            let own = if show_b { q } else { p } * (1.0 - 0.03 * smoothstep(0.85, 1.0, r));
            let ss = me.spec;
            let uv = vec2((ss.cx + own.x * ss.px) / me.size.x, (ss.cy + own.y * ss.px) / me.size.y);
            skin.push(Vertex::new(s.x, s.y, 0.0, uv.x, uv.y, Color::new(b, b * 0.97, b * 0.95, 1.0)));
            gloss.push(Vertex::new(s.x, s.y, 0.0, 0.0, 0.0, Color::new(1.0, 0.93, 0.88, (0.04 + 0.5 * sp) * me.gloss)));
            zs.push(z);
        };
        vert(Vec2::ZERO, 0.0);
        for k in 1..=RINGS {
            let r = k as f32 / RINGS as f32;
            for a in 0..SECTORS {
                let ang = a as f32 / SECTORS as f32 * TAU;
                vert(vec2(ang.cos(), ang.sin()) * self.a.rim_at(ang) * r, r);
            }
        }
        let id = |k: usize, a: usize| (1 + (k - 1) * SECTORS + a % SECTORS) as u16;
        let mut tris: Vec<[u16; 3]> = Vec::with_capacity(SECTORS * (2 * RINGS - 1));
        for a in 0..SECTORS {
            tris.push([0, id(1, a), id(1, a + 1)]);
        }
        for k in 1..RINGS {
            for a in 0..SECTORS {
                tris.push([id(k, a), id(k + 1, a), id(k + 1, a + 1)]);
                tris.push([id(k, a), id(k + 1, a + 1), id(k, a + 1)]);
            }
        }
        // Painter's order: far triangles first.
        let tz = |t: &[u16; 3]| t.iter().map(|&i| zs[i as usize]).sum::<f32>();
        tris.sort_by(|a, b| tz(a).total_cmp(&tz(b)));
        let indices: Vec<u16> = tris.into_iter().flatten().collect();
        draw_mesh(&Mesh { vertices: skin, indices: indices.clone(), texture: Some(me.tex.clone()) });
        draw_mesh(&Mesh { vertices: gloss, indices, texture: None });

        // Mouth: a dark gap that opens under the upper teeth.
        if open > 0.05 {
            let (sa, sb) = (self.a.spec, self.b.spec);
            let steps = 10;
            let mut top = Vec::new();
            let mut bot = Vec::new();
            let mut teeth = Vec::new();
            for i in 0..=steps {
                let f = i as f32 / steps as f32;
                let lens = (1.0 - (2.0 * f - 1.0).powi(2)).max(0.0).sqrt();
                let line = |s: &Spec| {
                    let x = s.mouth_x.0 + (s.mouth_x.1 - s.mouth_x.0) * f;
                    (x, s.mouth_y, open * JAW * lens * s.jaw_x(x))
                };
                let ((xa, ya, da), (xb, yb, db)) = (line(sa), line(sb));
                let pt = |oa: f32, ob: f32| self.at(vec2(xa, ya + oa), vec2(xb, yb + ob), 0.01);
                top.push(pt(-0.01 * lens, -0.01 * lens));
                bot.push(pt(da, db));
                teeth.push(pt(da - 0.025 * lens, db - 0.025 * lens));
            }
            let dark = Color::new(0.12, 0.03, 0.04, 1.0);
            let tooth = Color::new(0.82, 0.78, 0.68, 1.0);
            for i in 0..steps {
                draw_triangle(top[i], top[i + 1], bot[i + 1], dark);
                draw_triangle(top[i], bot[i + 1], bot[i], dark);
                if open > 0.3 {
                    draw_triangle(teeth[i], teeth[i + 1], bot[i + 1], tooth);
                    draw_triangle(teeth[i], bot[i + 1], bot[i], tooth);
                }
            }
        }

        // Blink now and then: skin-coloured lids over the photo's eyes.
        if blink {
            for k in 0..2 {
                let (ea, eb) = (self.a.spec.eyes[k], self.b.spec.eyes[k]);
                let pts: Vec<Vec2> = (0..12)
                    .map(|i| {
                        let o = vec2((i as f32 / 12.0 * TAU).cos() * 0.15, (i as f32 / 12.0 * TAU).sin() * 0.065);
                        self.at(vec2(ea.0, ea.1) + o, vec2(eb.0, eb.1) + o, 0.01)
                    })
                    .collect();
                let c = self.at(vec2(ea.0, ea.1), vec2(eb.0, eb.1), 0.01);
                for i in 0..12 {
                    draw_triangle(c, pts[i], pts[(i + 1) % 12], me.lid[k]);
                }
                // Lash line along the lower half.
                for i in 0..6 {
                    let (p, q) = (pts[i], pts[i + 1]);
                    draw_line(p.x, p.y, q.x, q.y, 1.0, Color::new(0.1, 0.07, 0.07, 1.0));
                }
            }
        }
    }
}

/// Default shirt colour.
pub const SHIRT: Color = Color::new(0.58, 0.64, 0.74, 1.0);

/// A mottled blue-gray studio backdrop, made once.
pub fn backdrop() -> Texture2D {
    const N: u16 = 64;
    fn hash(x: i32, y: i32, seed: u32) -> f32 {
        let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ seed;
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0
    }
    fn noise(x: f32, y: f32, seed: u32) -> f32 {
        let (ix, iy) = (x.floor() as i32, y.floor() as i32);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let s = |t: f32| t * t * (3.0 - 2.0 * t);
        let a = hash(ix, iy, seed) + (hash(ix + 1, iy, seed) - hash(ix, iy, seed)) * s(fx);
        let b = hash(ix, iy + 1, seed) + (hash(ix + 1, iy + 1, seed) - hash(ix, iy + 1, seed)) * s(fx);
        a + (b - a) * s(fy)
    }
    let mut img = Image::gen_image_color(N, N, BLACK);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (x as f32 / N as f32, y as f32 / N as f32);
            let m = 0.5 * noise(u * 4.0, v * 4.0, 11) + 0.3 * noise(u * 9.0, v * 9.0, 23) + 0.2 * noise(u * 20.0, v * 20.0, 37);
            let glow = (-((u - 0.5).powi(2) + (v - 0.4).powi(2)) / 0.12).exp();
            let k = 0.55 + 0.35 * m + 0.3 * glow;
            img.set_pixel(x as u32, y as u32, Color::new(0.36 * k, 0.43 * k, 0.52 * k, 1.0));
        }
    }
    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Linear);
    tex
}

/// A soft, clean studio gradient for the modern personas, made once.
pub fn backdrop_modern() -> Texture2D {
    const N: u16 = 64;
    let mut img = Image::gen_image_color(N, N, BLACK);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (x as f32 / N as f32, y as f32 / N as f32);
            let glow = (-((u - 0.5).powi(2) + (v - 0.35).powi(2)) / 0.18).exp();
            let k = 0.55 + 0.45 * glow - 0.15 * v;
            img.set_pixel(x as u32, y as u32, Color::new(0.30 + 0.42 * k, 0.36 + 0.42 * k, 0.42 + 0.40 * k, 1.0));
        }
    }
    let tex = Texture2D::from_image(&img);
    tex.set_filter(FilterMode::Linear);
    tex
}

pub fn draw_backdrop(tex: &Texture2D) {
    draw_texture_ex(tex, 0.0, 0.0, WHITE, DrawTextureParams { dest_size: Some(vec2(200.0, 200.0)), ..Default::default() });
}

/// Plain shirt shoulders, shifted `x` across.
fn draw_shoulders(x: f32, shirt: Color) {
    let p = |px: f32, py: f32| vec2(px + x, py);
    let fold = scale(shirt, 0.8);
    draw_triangle(p(-5.0, 205.0), p(205.0, 205.0), p(35.0, 160.0), shirt);
    draw_triangle(p(205.0, 205.0), p(165.0, 160.0), p(35.0, 160.0), shirt);
    draw_triangle(p(-5.0, 205.0), p(35.0, 160.0), p(3.0, 178.0), fold);
    draw_triangle(p(205.0, 205.0), p(165.0, 160.0), p(197.0, 178.0), fold);
}

/// Neck and an open-collar shirt.
/// `soft` swaps the flat-shaded neck for a smooth gradient, for the modern
/// personas that have no CRT to hide the bands.
pub fn draw_body(pose: &Pose, neck: Color, collar: Color, shirt: Color, soft: bool) {
    let x = pose.dx * 0.3 + pose.yaw * 4.0;
    let p = |px: f32, py: f32| vec2(px + x, py);
    // The head turns about a point behind the face, so the chin swings
    // further than the shoulders; the neck follows it part way.
    let x = if soft { x + pose.yaw * 14.0 } else { x };
    if soft {
        // Darkest just under the chin, fading to skin by the collar.
        let rows = [(115.0, 0.6), (140.0, 0.68), (165.0, 0.85)];
        let mut vertices = Vec::new();
        for (y, k) in rows {
            let c = scale(neck, k);
            vertices.push(Vertex::new(80.0 + x, y, 0.0, 0.0, 0.0, c));
            vertices.push(Vertex::new(120.0 + x, y, 0.0, 0.0, 0.0, c));
        }
        draw_mesh(&Mesh { vertices, indices: vec![0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4], texture: None });
    } else {
        draw_rectangle(80.0 + x, 115.0, 40.0, 50.0, scale(neck, 0.85));
        // Shadow under the chin.
        draw_rectangle(80.0 + x, 136.0, 40.0, 8.0, scale(neck, 0.6));
    }
    draw_shoulders(pose.dx * 0.3 + pose.yaw * 4.0, shirt);
    // Open neck and collar points.
    draw_triangle(p(84.0, 156.0), p(116.0, 156.0), p(100.0, 192.0), if soft { scale(neck, 0.85) } else { neck });
    draw_triangle(p(84.0, 150.0), p(70.0, 170.0), p(97.0, 186.0), collar);
    draw_triangle(p(116.0, 150.0), p(130.0, 170.0), p(103.0, 186.0), collar);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_outlines_follow_the_cheek() {
        use crate::persona::Persona;
        for p in [Persona::ModernMale, Persona::ModernFemale] {
            let img = Image::from_file_with_format(p.image(), None).unwrap();
            let rim = trace(p.face_spec());
            for (k, v) in p.views().unwrap().iter().enumerate() {
                let t = turned_image(&img, v).unwrap();
                let moved = follow_edge(&rim, &img, &t, p.face_spec());
                let d: Vec<f32> = rim.iter().zip(&moved).map(|(a, b)| b.1 - a.1).collect();
                eprintln!("{} {k}: {:?}", p.tag(), d.iter().map(|x| (x * 100.0).round() / 100.0).collect::<Vec<_>>());
                assert!(d.iter().all(|x| x.abs() < 0.12), "{} view {k} outline jumps", p.tag());
            }
        }
    }

    #[test]
    fn rim_follows_outline_all_round() {
        for spec in [&GUIDE, &ROXY, &MILES, &CLARA] {
            let rim = trace(spec);
            for &(px, py) in spec.outline {
                let v = vec2((px - spec.cx) / spec.px, (py - spec.cy) / spec.px);
                for ang in [v.y.atan2(v.x), v.y.atan2(v.x) + TAU] {
                    assert!((rim_at(&rim, ang) - v.length()).abs() < 1e-3, "{px},{py} at {ang}");
                }
            }
        }
    }

    #[test]
    fn features_sit_inside_the_outline() {
        for spec in [&GUIDE, &ROXY, &MILES, &CLARA] {
            let rim = trace(spec);
            let pts = [spec.eyes[0], spec.eyes[1], spec.nose, (spec.mouth_x.0, spec.mouth_y), (spec.mouth_x.1, spec.mouth_y)];
            for (x, y) in pts {
                assert!(vec2(x, y).length() < rim_at(&rim, y.atan2(x)) * 0.8, "{x},{y}");
            }
        }
    }

    #[test]
    fn warp_lines_up_features_and_outline() {
        let pairs = [(&MILES, &CLARA), (&CLARA, &MILES)];
        for (a, b) in pairs {
            let m = Warp::new(a, b);
            for (pa, pb) in anchors(a).iter().zip(anchors(b)) {
                let got = m.map(*pa);
                assert!((got - pb).length() < 0.03, "feature {pa} went to {got}, want {pb}");
            }
            for k in 0..36 {
                let ang = k as f32 / 36.0 * TAU;
                let dir = vec2(ang.cos(), ang.sin());
                let got = m.map(dir * rim_at(&m.rim_a, ang)).length();
                let want = rim_at(&m.rim_b, ang);
                assert!((got - want).abs() < 1e-3, "rim at {ang}: {got} vs {want}");
            }
            for k in 1..14 {
                for s in 0..60 {
                    let ang = s as f32 / 60.0 * TAU;
                    let p = vec2(ang.cos(), ang.sin()) * rim_at(&m.rim_a, ang) * (k as f32 / 14.0);
                    let q = m.map(p);
                    assert!(q.length() < rim_at(&m.rim_b, q.y.atan2(q.x)) * 0.999, "{p} left the outline at {q}");
                }
            }
            let same = Warp::new(a, a);
            assert!((same.map(vec2(0.2, -0.3)) - vec2(0.2, -0.3)).length() < 1e-4);
        }
    }

    #[test]
    fn jaw_only_below_mouth() {
        let s = &GUIDE;
        assert_eq!(s.jaw(0.08, 0.0), 0.0);
        assert!((s.jaw(0.08, 0.8) - 1.0).abs() < 1e-5);
        assert_eq!(s.jaw(0.9, 0.8), 0.0);
    }
}
