//! Rex's head: a low-poly lathe mesh with flat, posterized shading, drawn
//! in 2D with painter's sort. All coordinates are in the 200×200 render
//! target, y pointing down.

use macroquad::math::{EulerRot, Mat3};
use macroquad::prelude::*;
use std::f32::consts::{PI, TAU};

const LAT: usize = 11;
const LON: usize = 18;
const H: f32 = 1.25;
const DEPTH: f32 = 0.9;
const D: f32 = 6.0;
const SCALE: f32 = 86.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Booting,
    Idle,
    Listening,
    Thinking,
    Speaking,
    Error,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// 0 closed .. 1 wide open.
    pub mouth: f32,
    /// Brow lift, 0..1.
    pub brow: f32,
    /// Head offset in render-target pixels.
    pub dx: f32,
    pub dy: f32,
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn radius(v: f32) -> f32 {
    let base = (PI * v).sin().max(0.0).powf(0.8);
    let chin = 1.0 - 0.38 * smoothstep(0.55, 1.0, v);
    let cheek = 1.0 + 0.06 * (-((v - 0.62) / 0.1).powi(2)).exp();
    0.5 * base * chin * cheek
}

/// Point on the head surface: `v` 0 (crown) .. 1 (chin), `th` around the
/// vertical axis (0 = facing the viewer), `out` pushes it off the surface.
fn surf(v: f32, th: f32, out: f32) -> Vec3 {
    let r = radius(v) * out;
    vec3(r * th.sin(), -H / 2.0 + v * H, r * th.cos() * DEPTH)
}

fn shade(base: Color, b: f32) -> Color {
    let b = ((b * 5.0).floor() / 5.0).clamp(0.15, 1.1);
    Color::new(base.r * b, base.g * b, base.b * b, 1.0)
}

pub fn mood_colors(m: Mood) -> (Color, Color) {
    match m {
        Mood::Listening => (Color::new(0.1, 1.0, 0.35, 1.0), Color::new(0.0, 0.5, 0.2, 1.0)),
        Mood::Thinking => (Color::new(1.0, 0.15, 0.2, 1.0), Color::new(1.0, 0.6, 0.0, 1.0)),
        Mood::Error => (Color::new(1.0, 0.0, 0.0, 1.0), Color::new(0.3, 0.0, 0.0, 1.0)),
        Mood::Booting => (Color::new(0.5, 0.5, 0.6, 1.0), Color::new(0.2, 0.2, 0.3, 1.0)),
        _ => (Color::new(0.0, 0.9, 1.0, 1.0), Color::new(1.0, 0.1, 0.8, 1.0)),
    }
}

struct View {
    rot: Mat3,
    center: Vec2,
}

impl View {
    fn new(pose: &Pose) -> Self {
        Self {
            rot: Mat3::from_euler(EulerRot::YXZ, pose.yaw, pose.pitch, pose.roll),
            center: vec2(100.0 + pose.dx, 82.0 + pose.dy),
        }
    }
    fn world(&self, p: Vec3) -> Vec3 {
        self.rot * p
    }
    fn screen(&self, w: Vec3) -> Vec2 {
        let s = D / (D - w.z);
        self.center + vec2(w.x, w.y) * SCALE * s
    }
    fn at(&self, v: f32, th: f32, out: f32) -> Vec2 {
        self.screen(self.world(surf(v, th, out)))
    }
}

fn quad(a: Vec2, b: Vec2, c: Vec2, d: Vec2, col: Color) {
    draw_triangle(a, b, c, col);
    draw_triangle(a, c, d, col);
}

pub fn draw_head(pose: &Pose, mood: Mood) {
    let view = View::new(pose);
    let skin = Color::new(0.95, 0.84, 0.76, 1.0);
    let hair = Color::new(0.2, 0.16, 0.14, 1.0);
    let light = vec3(-0.45, -0.55, 0.75).normalize();
    let eye = vec3(0.0, 0.0, D);

    // Ears go behind the mesh.
    for side in [-1.0f32, 1.0] {
        let w = view.world(surf(0.47, side * 1.5, 1.02));
        if w.z > -0.15 {
            let p = view.screen(w);
            draw_circle(p.x, p.y, 6.5, shade(skin, 0.7));
        }
    }

    // Build the lathe mesh in world space.
    let mut verts = vec![[Vec3::ZERO; LON]; LAT + 1];
    for (i, ring) in verts.iter_mut().enumerate() {
        let v = i as f32 / LAT as f32;
        for (j, p) in ring.iter_mut().enumerate() {
            let th = j as f32 / LON as f32 * TAU;
            *p = view.world(surf(v, th, 1.0));
        }
    }
    let middle = view.world(vec3(0.0, 0.0, 0.0));
    let mut faces: Vec<(f32, [Vec2; 4], Color)> = Vec::with_capacity(LAT * LON);
    for i in 0..LAT {
        for j in 0..LON {
            let jn = (j + 1) % LON;
            let (a, b, c, d) = (verts[i][j], verts[i][jn], verts[i + 1][jn], verts[i + 1][j]);
            let centroid = (a + b + c + d) / 4.0;
            let mut n = (c - a).cross(d - b);
            if n.length_squared() < 1e-9 {
                continue;
            }
            n = n.normalize();
            if n.dot(centroid - middle) < 0.0 {
                n = -n;
            }
            if n.dot(eye - centroid) <= 0.0 {
                continue;
            }
            let th = j as f32 / LON as f32 * TAU;
            let back = th.cos() < -0.1;
            let is_hair = i < 2 || (i < 5 && back) || (i < 3 && th.cos() < 0.55);
            let base = if is_hair { hair } else { skin };
            let b_ = 0.35 + 0.75 * n.dot(light).max(0.0);
            let pts = [view.screen(a), view.screen(b), view.screen(c), view.screen(d)];
            faces.push((centroid.z, pts, shade(base, b_)));
        }
    }
    faces.sort_by(|x, y| x.0.total_cmp(&y.0));
    for (_, p, col) in &faces {
        quad(p[0], p[1], p[2], p[3], *col);
    }

    // Front features only make sense while the face points our way.
    let front = view.world(surf(0.5, 0.0, 1.0));
    if front.z < 0.05 {
        return;
    }

    // Quiff.
    let hair_hi = shade(hair, 1.0);
    let tip = view.screen(view.world(surf(0.03, 0.2, 1.0) + vec3(0.12, -0.12, 0.38)));
    let l = view.at(0.16, -0.6, 1.03);
    let r = view.at(0.15, 0.6, 1.03);
    let m = view.at(0.2, 0.05, 1.04);
    draw_triangle(l, m, tip, shade(hair, 0.75));
    draw_triangle(m, r, tip, hair_hi);

    // Brows.
    let brow_v = 0.335 - pose.brow * 0.035;
    let bc = Color::new(0.12, 0.09, 0.08, 1.0);
    for side in [-1.0f32, 1.0] {
        let a = view.at(brow_v + 0.01, side * 0.18, 1.04);
        let b = view.at(brow_v - 0.005 * side, side * 0.58, 1.04);
        draw_line(a.x, a.y, b.x, b.y, 2.5, bc);
    }

    // Shades.
    let lens = Color::new(0.04, 0.04, 0.07, 1.0);
    let n = 9;
    let ths: Vec<f32> = (0..n).map(|k| -0.66 + 1.32 * k as f32 / (n - 1) as f32).collect();
    let top: Vec<Vec2> = ths.iter().map(|&t| view.at(0.37, t, 1.06)).collect();
    let bot: Vec<Vec2> = ths
        .iter()
        .map(|&t| {
            let bridge = (-(t / 0.12).powi(2)).exp();
            view.at(0.475 - 0.06 * bridge, t, 1.06)
        })
        .collect();
    for k in 0..n - 1 {
        quad(top[k], top[k + 1], bot[k + 1], bot[k], lens);
    }
    let (mc, _) = mood_colors(mood);
    let h1 = view.at(0.39, -0.5, 1.07);
    let h2 = view.at(0.43, -0.32, 1.07);
    draw_line(h1.x, h1.y, h2.x, h2.y, 1.5, Color::new(0.9, 0.95, 1.0, 0.9));
    let g1 = view.at(0.395, 0.28, 1.07);
    let g2 = view.at(0.395, 0.52, 1.07);
    draw_line(g1.x, g1.y, g2.x, g2.y, 1.0, Color::new(mc.r, mc.g, mc.b, 0.6));

    // Nose.
    let bridge = view.at(0.47, 0.0, 1.07);
    let tipn = view.at(0.585, 0.0, 1.2);
    let nl = view.at(0.615, -0.14, 1.03);
    let nr = view.at(0.615, 0.14, 1.03);
    let lit = pose.yaw > 0.0;
    draw_triangle(bridge, tipn, nl, shade(skin, if lit { 0.75 } else { 1.0 }));
    draw_triangle(bridge, tipn, nr, shade(skin, if lit { 1.0 } else { 0.75 }));
    draw_triangle(nl, tipn, nr, shade(skin, 0.55));

    // Smirking mouth: the right corner rides higher.
    let open = pose.mouth.clamp(0.0, 1.0);
    let ml = view.at(0.735, -0.3, 1.0);
    let mr = view.at(0.705, 0.32, 1.0);
    let up = view.at(0.715, 0.02, 1.03);
    let lo = view.at(0.72 + 0.11 * open, 0.02, 1.03);
    let lip = Color::new(0.55, 0.22, 0.24, 1.0);
    if open > 0.04 {
        draw_triangle(ml, up, lo, Color::new(0.2, 0.03, 0.05, 1.0));
        draw_triangle(up, mr, lo, Color::new(0.2, 0.03, 0.05, 1.0));
        if open > 0.15 {
            let tl = view.at(0.725, -0.2, 1.02);
            let tr = view.at(0.71, 0.22, 1.02);
            let tm = view.at(0.72 + 0.03 * open, 0.02, 1.03);
            draw_triangle(tl, up, tm, WHITE);
            draw_triangle(up, tr, tm, WHITE);
        }
        draw_line(ml.x, ml.y, up.x, up.y, 1.5, lip);
        draw_line(up.x, up.y, mr.x, mr.y, 1.5, lip);
        draw_line(ml.x, ml.y, lo.x, lo.y, 2.0, lip);
        draw_line(lo.x, lo.y, mr.x, mr.y, 2.0, lip);
    } else {
        draw_line(ml.x, ml.y, up.x, up.y, 2.0, lip);
        draw_line(up.x, up.y, mr.x, mr.y, 2.0, lip);
        let dimple = view.at(0.69, 0.36, 1.0);
        draw_line(mr.x, mr.y, dimple.x, dimple.y, 1.0, shade(skin, 0.6));
    }
}
