//! The photo head: a still photo stretched over a lumpy low-poly mesh and
//! lit like shiny plastic, plus the studio backdrop and the shirt.
//! Coordinates are in the 200×200 render target, y pointing down.

use crate::head::Pose;
use macroquad::math::{EulerRot, Mat3};
use macroquad::prelude::*;
use std::f32::consts::TAU;

const RINGS: usize = 14;
const SECTORS: usize = 48;
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
}

/// The original `face.png` photo.
pub const PHOTO: Spec = Spec {
    px: 125.0,
    cx: 145.0,
    cy: 190.0,
    outline: &[
        (145.0, 12.0),
        (190.0, 18.0),
        (230.0, 40.0),
        (255.0, 80.0),
        (266.0, 130.0),
        (264.0, 175.0),
        (260.0, 225.0),
        (250.0, 258.0),
        (235.0, 285.0),
        (212.0, 308.0),
        (182.0, 324.0),
        (145.0, 330.0),
        (110.0, 325.0),
        (84.0, 310.0),
        (64.0, 288.0),
        (50.0, 260.0),
        (32.0, 225.0),
        (26.0, 175.0),
        (25.0, 130.0),
        (35.0, 80.0),
        (62.0, 38.0),
        (100.0, 16.0),
    ],
    eyes: [(-0.36, -0.215), (0.36, -0.2)],
    nose: (0.0, 0.17),
    mouth_y: 0.5,
    mouth_x: (-0.12, 0.28),
    hair_at: (145.0, 40.0),
    lid_at: [(100.0, 147.0), (190.0, 149.0)],
    neck_at: (92.0, 215.0),
    collar_at: (60.0, 345.0),
    shirt: None,
};

/// The male head, `assets/rex.png` (611×694).
pub const GUIDE: Spec = Spec {
    px: 236.0,
    cx: 290.0,
    cy: 365.0,
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
};

/// The female head, `assets/roxy.png` (508×713), facing forward.
pub const ROXY: Spec = Spec {
    px: 229.0,
    cx: 257.5,
    cy: 354.0,
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
    next_blink: f32,
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

impl Face {
    /// Load the photo, or `None` if it's missing or unreadable.
    pub fn load(path: &std::path::Path, spec: &'static Spec) -> Option<Self> {
        Self::from_bytes(&std::fs::read(path).ok()?, spec)
    }

    /// Build from encoded image bytes (PNG or JPEG).
    pub fn from_bytes(bytes: &[u8], spec: &'static Spec) -> Option<Self> {
        let img = Image::from_file_with_format(bytes, None).ok()?;
        if img.width() < 64 || img.height() < 64 {
            return None;
        }
        let tex = Texture2D::from_image(&img);
        tex.set_filter(FilterMode::Linear);
        let at = |p: (f32, f32)| patch(&img, p.0, p.1);
        Some(Self {
            spec,
            tex,
            size: vec2(img.width() as f32, img.height() as f32),
            rim: trace(spec),
            hair: at(spec.hair_at),
            lid: [at(spec.lid_at[0]), at(spec.lid_at[1])],
            neck: scale(at(spec.neck_at), 0.8),
            collar: at(spec.collar_at),
            shirt: spec.shirt.unwrap_or(SHIRT),
            next_blink: 2.0,
        })
    }

    fn rim_at(&self, ang: f32) -> f32 {
        rim_at(&self.rim, ang)
    }

    fn depth(&self, x: f32, y: f32) -> f32 {
        let r = ((x * x + y * y).sqrt() / self.rim_at(y.atan2(x))).min(1.0);
        0.9 * (1.0 - r * r).max(0.0).sqrt() + self.spec.bumps(x, y) * (1.0 - smoothstep(0.75, 1.0, r))
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

    pub fn draw(&mut self, pose: &Pose, t: f32) {
        let view = View::new(pose);
        let open = pose.mouth.clamp(0.0, 1.0);
        let light = vec3(-0.35, -0.5, 0.8).normalize();
        let half = (light + vec3(0.0, 0.0, 1.0)).normalize();
        let shadow = scale(self.neck, 0.6);

        // Back of the head: a flat plate so turning doesn't show the backdrop.
        let mut back = Mesh { vertices: Vec::with_capacity(SECTORS + 1), indices: Vec::new(), texture: None };
        let c = view.screen(view.world(vec3(0.0, -0.1, -0.55)));
        back.vertices.push(Vertex::new(c.x, c.y, 0.0, 0.0, 0.0, self.hair));
        for a in 0..SECTORS {
            let ang = a as f32 / SECTORS as f32 * TAU;
            let dir = vec2(ang.cos(), ang.sin());
            let p = dir * self.rim_at(ang) * 0.98;
            let s = view.screen(view.world(vec3(p.x, p.y, -0.35)));
            let col = mix(self.hair, shadow, smoothstep(0.1, 0.6, dir.y));
            back.vertices.push(Vertex::new(s.x, s.y, 0.0, 0.0, 0.0, col));
            let next = 1 + (a as u16 + 1) % SECTORS as u16;
            back.indices.extend_from_slice(&[0, 1 + a as u16, next]);
        }
        draw_mesh(&back);

        // The face: rings around the nose, textured with the photo.
        let mut skin = Vec::with_capacity(1 + RINGS * SECTORS);
        let mut gloss = Vec::with_capacity(1 + RINGS * SECTORS);
        let mut zs = Vec::with_capacity(1 + RINGS * SECTORS);
        let mut vert = |x: f32, y: f32| {
            let z = self.depth(x, y);
            let w = view.world(vec3(x, y + open * JAW * self.spec.jaw(x, y), z));
            let s = view.screen(w);
            let n = view.rot * self.normal(x, y);
            let b = (0.62 + 0.45 * n.dot(light).max(0.0)).min(1.0);
            // Less shine on the hair, so it stays dark.
            let spec = n.dot(half).max(0.0).powf(24.0) * (0.35 + 0.65 * smoothstep(-0.55, -0.3, y));
            let sp = self.spec;
            let uv = vec2((sp.cx + x * sp.px) / self.size.x, (sp.cy + y * sp.px) / self.size.y);
            skin.push(Vertex::new(s.x, s.y, 0.0, uv.x, uv.y, Color::new(b, b * 0.97, b * 0.95, 1.0)));
            gloss.push(Vertex::new(s.x, s.y, 0.0, 0.0, 0.0, Color::new(1.0, 0.93, 0.88, 0.04 + 0.5 * spec)));
            zs.push(w.z);
        };
        vert(0.0, 0.0);
        for k in 1..=RINGS {
            let r = k as f32 / RINGS as f32;
            for a in 0..SECTORS {
                let ang = a as f32 / SECTORS as f32 * TAU;
                let p = vec2(ang.cos(), ang.sin()) * self.rim_at(ang) * r;
                vert(p.x, p.y);
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
        draw_mesh(&Mesh { vertices: skin, indices: indices.clone(), texture: Some(self.tex.clone()) });
        draw_mesh(&Mesh { vertices: gloss, indices, texture: None });

        // Mouth: a dark gap that opens under the upper teeth.
        if open > 0.05 {
            let (x0, x1) = self.spec.mouth_x;
            let mouth_y = self.spec.mouth_y;
            let mid = (x0 + x1) / 2.0;
            let halfw = (x1 - x0) / 2.0;
            let steps = 10;
            let mut top = Vec::new();
            let mut bot = Vec::new();
            let mut teeth = Vec::new();
            for i in 0..=steps {
                let x = x0 + (x1 - x0) * i as f32 / steps as f32;
                let lens = (1.0 - ((x - mid) / halfw).powi(2)).max(0.0).sqrt();
                let drop = open * JAW * lens * self.spec.jaw_x(x);
                top.push(self.at(&view, x, mouth_y - 0.01 * lens, 0.01));
                bot.push(self.at(&view, x, mouth_y + drop, 0.01));
                teeth.push(self.at(&view, x, mouth_y + drop - 0.025 * lens, 0.01));
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
        if t > self.next_blink + 0.14 {
            self.next_blink = t + ::rand::random_range(2.5..6.0);
        }
        if t >= self.next_blink {
            for (k, &(ex, ey)) in self.spec.eyes.iter().enumerate() {
                let pts: Vec<Vec2> = (0..12)
                    .map(|i| {
                        let a = i as f32 / 12.0 * TAU;
                        self.at(&view, ex + a.cos() * 0.15, ey + a.sin() * 0.065, 0.01)
                    })
                    .collect();
                let c = self.at(&view, ex, ey, 0.01);
                for i in 0..12 {
                    draw_triangle(c, pts[i], pts[(i + 1) % 12], self.lid[k]);
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

pub fn draw_backdrop(tex: &Texture2D) {
    draw_texture_ex(tex, 0.0, 0.0, WHITE, DrawTextureParams { dest_size: Some(vec2(200.0, 200.0)), ..Default::default() });
}

/// Neck and an open-collar shirt.
pub fn draw_body(pose: &Pose, neck: Color, collar: Color, shirt: Color) {
    let x = pose.dx * 0.3 + pose.yaw * 4.0;
    let p = |px: f32, py: f32| vec2(px + x, py);
    let fold = scale(shirt, 0.8);
    draw_rectangle(80.0 + x, 115.0, 40.0, 50.0, scale(neck, 0.85));
    // Shadow under the chin.
    draw_rectangle(80.0 + x, 136.0, 40.0, 8.0, scale(neck, 0.6));
    // Shoulders.
    draw_triangle(p(-5.0, 205.0), p(205.0, 205.0), p(35.0, 160.0), shirt);
    draw_triangle(p(205.0, 205.0), p(165.0, 160.0), p(35.0, 160.0), shirt);
    draw_triangle(p(-5.0, 205.0), p(35.0, 160.0), p(3.0, 178.0), fold);
    draw_triangle(p(205.0, 205.0), p(165.0, 160.0), p(197.0, 178.0), fold);
    // Open neck and collar points.
    draw_triangle(p(84.0, 156.0), p(116.0, 156.0), p(100.0, 192.0), neck);
    draw_triangle(p(84.0, 150.0), p(70.0, 170.0), p(97.0, 186.0), collar);
    draw_triangle(p(116.0, 150.0), p(130.0, 170.0), p(103.0, 186.0), collar);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rim_follows_outline_all_round() {
        for spec in [&PHOTO, &GUIDE, &ROXY] {
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
        for spec in [&PHOTO, &GUIDE, &ROXY] {
            let rim = trace(spec);
            let pts = [spec.eyes[0], spec.eyes[1], spec.nose, (spec.mouth_x.0, spec.mouth_y), (spec.mouth_x.1, spec.mouth_y)];
            for (x, y) in pts {
                assert!(vec2(x, y).length() < rim_at(&rim, y.atan2(x)) * 0.8, "{x},{y}");
            }
        }
    }

    #[test]
    fn jaw_only_below_mouth() {
        let s = &PHOTO;
        assert_eq!(s.jaw(0.08, 0.0), 0.0);
        assert!((s.jaw(0.08, 0.8) - 1.0).abs() < 1e-5);
        assert_eq!(s.jaw(0.9, 0.8), 0.0);
    }
}
