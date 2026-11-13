//! The generic plastic human: a smooth, shiny mannequin head that isn't
//! based on anyone. Same mesh approach as the photo head, but the shape,
//! colours and features are built in code. Coordinates are in the 200×200
//! render target, y pointing down; head units are 47 render-target pixels.
//!
//! Proportions follow the classic thirds: hairline to brow, brow to the
//! base of the nose, and nose to chin are about equal, with the eyes
//! halfway down the whole head.

use crate::face::{mix, rim_at, scale, smoothstep, View};
use crate::head::Pose;
use macroquad::prelude::*;
use std::f32::consts::{PI, TAU};

const RINGS: usize = 32;
const SECTORS: usize = 80;
/// Outline samples used to look up the head radius at any angle.
const RIM_SAMPLES: usize = 160;
/// How far the jaw drops when the mouth is fully open, in head units.
const JAW: f32 = 0.13;
const MOUTH_Y: f32 = 0.6;
const MOUTH_W: f32 = 0.21;
const EYE_X: f32 = 0.34;
const EYE_Y: f32 = -0.08;
/// Half the width of one eye.
const EYE_W: f32 = 0.15;
const NOSE_Y: f32 = 0.38;

const SKIN: Color = Color::new(0.92, 0.74, 0.62, 1.0);
const BLUSH: Color = Color::new(0.9, 0.58, 0.52, 1.0);
const HAIR: Color = Color::new(0.19, 0.13, 0.09, 1.0);
const LIP_UP: Color = Color::new(0.68, 0.38, 0.37, 1.0);
const LIP_LO: Color = Color::new(0.76, 0.46, 0.44, 1.0);
const IRIS: Color = Color::new(0.33, 0.4, 0.3, 1.0);
const BROW: Color = Color::new(0.22, 0.15, 0.11, 1.0);

fn g(x: f32, y: f32, cx: f32, cy: f32, sx: f32, sy: f32) -> f32 {
    (-((x - cx) / sx).powi(2) - ((y - cy) / sy).powi(2)).exp()
}

fn pair(x: f32, y: f32, cx: f32, cy: f32, sx: f32, sy: f32) -> f32 {
    g(x, y, -cx, cy, sx, sy) + g(x, y, cx, cy, sx, sy)
}

/// A soft ridge or groove along the segment a..b.
fn line_g(x: f32, y: f32, a: Vec2, b: Vec2, s: f32) -> f32 {
    let p = vec2(x, y);
    let t = ((p - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0);
    let d = (p - (a + (b - a) * t)).length();
    (-(d / s).powi(2)).exp()
}

fn mix_f(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Half the head's width at height `y`: round skull, wide cheekbones,
/// a square jaw angle, then a firm chin.
fn half_width(y: f32) -> f32 {
    const TOP: f32 = -1.15;
    const JAW_ANGLE: f32 = 0.62;
    const CHIN: f32 = 0.98;
    const CHIN_W: f32 = 0.32;
    if y < -0.1 {
        let u = (y + 0.1) / (-0.1 - TOP);
        0.9 * (1.0 - u * u).max(0.0).sqrt()
    } else if y < 0.1 {
        0.9
    } else if y < JAW_ANGLE {
        mix_f(0.9, 0.8, (y - 0.1) / (JAW_ANGLE - 0.1))
    } else if y < CHIN {
        let u = (y - JAW_ANGLE) / (CHIN - JAW_ANGLE);
        mix_f(0.8, CHIN_W, u.powf(1.3))
    } else {
        let u = (y - CHIN) / 0.1;
        CHIN_W * (1.0 - u * u).max(0.0).sqrt()
    }
}

fn inside(x: f32, y: f32) -> bool {
    x.abs() <= half_width(y)
}

/// (angle, radius) around the outline, sorted by angle.
fn trace() -> Vec<(f32, f32)> {
    (0..RIM_SAMPLES)
        .map(|i| {
            let ang = -PI + i as f32 / RIM_SAMPLES as f32 * TAU;
            let dir = vec2(ang.cos(), ang.sin());
            let (mut lo, mut hi) = (0.0f32, 2.0f32);
            for _ in 0..30 {
                let mid = (lo + hi) / 2.0;
                if inside(dir.x * mid, dir.y * mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            (ang, lo)
        })
        .collect()
}

/// Hollows and creases: eye sockets, cheek hollows, the lines from nose to
/// mouth, the philtrum, the dip under the lower lip and the temples. Also
/// used to darken those spots, like soft shadow.
fn cavity(x: f32, y: f32) -> f32 {
    let fold = |s: f32| line_g(x, y, vec2(s * 0.15, 0.37), vec2(s * 0.27, 0.64), 0.035);
    0.15 * pair(x, y, EYE_X, EYE_Y, 0.15, 0.09)
        + 0.05 * pair(x, y, 0.55, 0.42, 0.13, 0.13)
        + 0.035 * (fold(-1.0) + fold(1.0))
        + 0.02 * g(x, y, 0.0, 0.49, 0.025, 0.05)
        + 0.03 * g(x, y, 0.0, 0.71, 0.13, 0.03)
        + 0.03 * pair(x, y, 0.82, -0.3, 0.1, 0.2)
}

/// Bones and soft parts pushed out of the base shape.
fn bumps(x: f32, y: f32) -> f32 {
    0.15 * line_g(x, y, vec2(0.0, -0.08), vec2(0.0, 0.28), 0.06)
        + 0.22 * g(x, y, 0.0, 0.3, 0.075, 0.07)
        + 0.08 * pair(x, y, 0.11, 0.34, 0.05, 0.04)
        + 0.09 * g(x, y, 0.0, -0.24, 0.6, 0.07)
        + 0.04 * g(x, y, 0.0, -0.18, 0.1, 0.06)
        + 0.08 * pair(x, y, 0.5, 0.1, 0.16, 0.1)
        + 0.07 * g(x, y, 0.0, MOUTH_Y, 0.21, 0.07)
        + 0.1 * g(x, y, 0.0, 0.9, 0.2, 0.1)
        + 0.04 * pair(x, y, 0.62, 0.68, 0.12, 0.12)
        - cavity(x, y)
}

fn jaw_x(x: f32) -> f32 {
    1.0 - smoothstep(0.25, 0.6, x.abs())
}

fn jaw(x: f32, y: f32) -> f32 {
    smoothstep(MOUTH_Y - 0.03, MOUTH_Y + 0.03, y) * jaw_x(x)
}

/// How much hair covers a point: a moulded cap with a hairline that
/// recedes a little at the temples, plus short sideburns.
fn hair(x: f32, y: f32) -> f32 {
    let line = -0.6 + 0.3 * x * x;
    let top = 1.0 - smoothstep(line - 0.03, line + 0.03, y);
    let side = smoothstep(0.76, 0.84, x.abs()) * (1.0 - smoothstep(-0.02, 0.06, y));
    top.max(side)
}

pub struct Mannequin {
    rim: Vec<(f32, f32)>,
    next_blink: f32,
}

impl Mannequin {
    pub const NECK: Color = Color::new(0.78, 0.6, 0.5, 1.0);
    pub const COLLAR: Color = Color::new(0.92, 0.92, 0.9, 1.0);

    pub fn new() -> Self {
        Self { rim: trace(), next_blink: 2.0 }
    }

    fn rim_at(&self, ang: f32) -> f32 {
        rim_at(&self.rim, ang)
    }

    /// Height of the surface toward the viewer: flat across the front and
    /// turning away quickly at the sides, like a real face.
    fn depth(&self, x: f32, y: f32) -> f32 {
        let r = ((x * x + y * y).sqrt() / self.rim_at(y.atan2(x))).min(1.0);
        let dome = 0.9 * (1.0 - r.powf(2.6)).max(0.0).powf(0.45);
        dome + bumps(x, y) * (1.0 - smoothstep(0.8, 1.0, r))
    }

    fn normal(&self, x: f32, y: f32) -> Vec3 {
        let e = 0.015;
        let zx = (self.depth(x + e, y) - self.depth(x - e, y)) / (2.0 * e);
        let zy = (self.depth(x, y + e) - self.depth(x, y - e)) / (2.0 * e);
        vec3(-zx, -zy, 1.0).normalize()
    }

    /// A point on the face surface, sitting `lift` above it.
    fn at(&self, view: &View, x: f32, y: f32, lift: f32) -> Vec2 {
        view.screen(view.world(vec3(x, y, self.depth(x, y) + lift)))
    }

    /// Plastic shading: soft diffuse, a tight hot highlight and a broad sheen.
    fn lit(base: Color, n: Vec3, light: Vec3, half: Vec3, shine: f32) -> Color {
        let diffuse = 0.45 + 0.6 * n.dot(light).max(0.0);
        let edge = 0.7 + 0.3 * n.z.max(0.0);
        let nh = n.dot(half).max(0.0);
        let spec = shine * nh.powf(50.0) + 0.1 * nh.powf(6.0);
        let c = scale(base, diffuse * edge);
        Color::new((c.r + spec).min(1.0), (c.g + spec).min(1.0), (c.b + spec * 0.95).min(1.0), 1.0)
    }

    pub fn draw(&mut self, pose: &Pose, t: f32) {
        let view = View::new(pose);
        let open = pose.mouth.clamp(0.0, 1.0);
        let light = vec3(-0.35, -0.5, 0.8).normalize();
        let half = (light + vec3(0.0, 0.0, 1.0)).normalize();
        let hair_dark = scale(HAIR, 0.7);

        // Back of the head: a flat plate so turning doesn't show the backdrop.
        let mut back = Mesh { vertices: Vec::with_capacity(SECTORS + 1), indices: Vec::new(), texture: None };
        let c = view.screen(view.world(vec3(0.0, -0.1, -0.55)));
        back.vertices.push(Vertex::new(c.x, c.y, 0.0, 0.0, 0.0, HAIR));
        for a in 0..SECTORS {
            let ang = a as f32 / SECTORS as f32 * TAU;
            let dir = vec2(ang.cos(), ang.sin());
            let p = dir * self.rim_at(ang) * 0.98;
            let s = view.screen(view.world(vec3(p.x, p.y, -0.35)));
            let col = mix(hair_dark, scale(Self::NECK, 0.6), smoothstep(0.1, 0.6, dir.y));
            back.vertices.push(Vertex::new(s.x, s.y, 0.0, 0.0, 0.0, col));
            let next = 1 + (a as u16 + 1) % SECTORS as u16;
            back.indices.extend_from_slice(&[0, 1 + a as u16, next]);
        }
        draw_mesh(&back);

        // Ears, mostly hidden behind the cheeks.
        for side in [-1.0f32, 1.0] {
            let n = view.rot * vec3(side, 0.0, 0.3).normalize();
            let col = Self::lit(scale(SKIN, 0.9), n, light, half, 0.5);
            let inner = scale(col, 0.75);
            let ring = |k: f32| -> Vec<Vec2> {
                (0..14)
                    .map(|i| {
                        let a = i as f32 / 14.0 * TAU;
                        // Wider at the top, narrow lobe at the bottom.
                        let w = 0.1 * k * (1.0 - 0.35 * smoothstep(0.0, 1.0, a.sin()));
                        view.screen(view.world(vec3(side * (0.88 + w * a.cos()), 0.12 + 0.24 * k * a.sin(), -0.08)))
                    })
                    .collect()
            };
            let (outer, hole) = (ring(1.0), ring(0.55));
            let c = view.screen(view.world(vec3(side * 0.88, 0.12, -0.08)));
            for i in 0..14 {
                draw_triangle(c, outer[i], outer[(i + 1) % 14], col);
            }
            for i in 0..14 {
                draw_triangle(c, hole[i], hole[(i + 1) % 14], inner);
            }
        }

        // The face: rings around the nose, coloured and lit per vertex.
        let mut verts = Vec::with_capacity(1 + RINGS * SECTORS);
        let mut zs = Vec::with_capacity(1 + RINGS * SECTORS);
        let mut vert = |x: f32, y: f32| {
            let z = self.depth(x, y);
            let w = view.world(vec3(x, y + open * JAW * jaw(x, y), z));
            let s = view.screen(w);
            let n = view.rot * self.normal(x, y);
            let blush = 0.35 * pair(x, y, 0.45, 0.25, 0.15, 0.1) + 0.25 * g(x, y, 0.0, 0.3, 0.07, 0.06);
            let h = hair(x, y);
            let base = mix(mix(SKIN, BLUSH, blush), HAIR, h);
            let shadow = 1.0 - 2.2 * cavity(x, y) * (1.0 - h);
            let col = scale(Self::lit(base, n, light, half, 0.8 - 0.35 * h), shadow);
            verts.push(Vertex::new(s.x, s.y, 0.0, 0.0, 0.0, col));
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
        // macroquad caps one draw at 5000 indices, so draw in sorted chunks.
        let mut mesh = Mesh { vertices: verts, indices: Vec::new(), texture: None };
        for chunk in tris.chunks(1600) {
            mesh.indices = chunk.iter().flatten().copied().collect();
            draw_mesh(&mesh);
        }

        // Features only make sense while the face points our way.
        if view.world(vec3(0.0, 0.0, 1.0)).z < 0.3 {
            return;
        }
        self.draw_nostrils(&view);
        self.draw_eyes(&view, pose, t);
        self.draw_mouth(&view, open);
    }

    fn draw_nostrils(&self, view: &View) {
        let dark = Color::new(0.35, 0.2, 0.17, 0.85);
        for side in [-1.0f32, 1.0] {
            let pts: Vec<Vec2> = (0..8)
                .map(|i| {
                    let a = i as f32 / 8.0 * TAU;
                    self.at(view, side * 0.075 + 0.03 * a.cos(), NOSE_Y + 0.012 * a.sin(), 0.01)
                })
                .collect();
            let c = self.at(view, side * 0.075, NOSE_Y, 0.01);
            for i in 0..8 {
                draw_triangle(c, pts[i], pts[(i + 1) % 8], dark);
            }
        }
    }

    fn draw_eyes(&mut self, view: &View, pose: &Pose, t: f32) {
        if t > self.next_blink + 0.14 {
            self.next_blink = t + ::rand::random_range(2.5..6.0);
        }
        let shut = t >= self.next_blink;
        let lash = Color::new(0.1, 0.07, 0.06, 1.0);
        let crease = Color::new(0.55, 0.38, 0.32, 0.8);
        // Look back at the viewer when the head turns.
        let gaze = vec2(-pose.yaw * 0.05, -pose.pitch * 0.03);
        let n = 14;
        for side in [-1.0f32, 1.0] {
            let (ex, ey) = (side * EYE_X, EYE_Y);
            // Almond: the upper arc peaks toward the inner corner, the lower
            // toward the outer one, and the outer corner sits a touch higher.
            let curve = |lift: f32, low: bool, peak: f32| -> Vec<Vec2> {
                (0..=n)
                    .map(|i| {
                        let u = i as f32 / n as f32;
                        let x = ex + side * (-EYE_W + 2.0 * EYE_W * u);
                        let skew = if low { u.powf(0.8) } else { u.powf(1.2) };
                        let h = (skew * PI).sin() * peak;
                        let tilt = -0.015 * u;
                        self.at(view, x, ey + tilt + if low { h } else { -h }, lift)
                    })
                    .collect()
            };
            let (top, bot) = (curve(0.012, false, 0.065), curve(0.012, true, 0.04));
            let band = |a: &[Vec2], b: &[Vec2], col: Color| {
                for i in 0..n {
                    draw_triangle(a[i], a[i + 1], b[i + 1], col);
                    draw_triangle(a[i], b[i + 1], b[i], col);
                }
            };
            let stroke = |p: &[Vec2], w: f32, col: Color| {
                for i in 0..n {
                    draw_line(p[i].x, p[i].y, p[i + 1].x, p[i + 1].y, w, col);
                }
            };
            // Upper lid crease.
            stroke(&curve(0.012, false, 0.1), 0.8, crease);
            if shut {
                band(&top, &bot, scale(SKIN, 0.82));
                stroke(&bot, 1.2, lash);
            } else {
                band(&top, &bot, Color::new(0.95, 0.93, 0.9, 1.0));
                let c = self.at(view, ex + gaze.x, ey + 0.005 + gaze.y, 0.014);
                let px = |r: f32| (self.at(view, ex + gaze.x + r, ey + gaze.y, 0.014) - c).length();
                draw_circle(c.x, c.y, px(0.055), scale(IRIS, 0.7));
                draw_circle(c.x, c.y, px(0.045), IRIS);
                draw_circle(c.x, c.y, px(0.022), Color::new(0.03, 0.03, 0.04, 1.0));
                draw_circle(c.x - px(0.02), c.y - px(0.02), px(0.012).max(0.6), WHITE);
                // The upper lid covers the top of the iris.
                let lid = curve(0.016, false, 0.03);
                band(&top, &lid, scale(SKIN, 0.78));
                stroke(&lid, 1.4, lash);
                stroke(&bot, 0.6, Color::new(0.5, 0.33, 0.3, 0.8));
            }

            // Brow: thick at the inner end, arching, tapering outward.
            let by = EYE_Y - 0.16 - pose.brow * 0.04;
            let k = 8;
            let brow: Vec<Vec2> = (0..=k)
                .map(|i| {
                    let u = i as f32 / k as f32;
                    let x = ex + side * (-0.17 + 0.36 * u);
                    let arch = (u.powf(0.7) * PI).sin() * 0.045;
                    self.at(view, x, by - arch + 0.02 * u, 0.02)
                })
                .collect();
            for i in 0..k {
                let w = 3.0 - 2.0 * i as f32 / k as f32;
                draw_line(brow[i].x, brow[i].y, brow[i + 1].x, brow[i + 1].y, w, BROW);
            }
        }
    }

    fn draw_mouth(&self, view: &View, open: f32) {
        let steps = 14;
        let lift = 0.012;
        let mut up_top = Vec::new();
        let mut up_bot = Vec::new();
        let mut lo_top = Vec::new();
        let mut lo_bot = Vec::new();
        let mut teeth = Vec::new();
        for i in 0..=steps {
            let x = -MOUTH_W + 2.0 * MOUTH_W * i as f32 / steps as f32;
            let lens = (1.0 - (x / MOUTH_W).powi(2)).max(0.0).sqrt();
            // Cupid's bow: two peaks either side of a small dip.
            let bow = 0.018 * pair(x, 0.0, 0.045, 0.0, 0.04, 1.0) - 0.008 * g(x, 0.0, 0.0, 0.0, 0.02, 1.0);
            let gap = open * JAW * lens.sqrt() * jaw_x(x);
            up_top.push(self.at(view, x, MOUTH_Y - 0.035 * lens - bow, lift));
            up_bot.push(self.at(view, x, MOUTH_Y, lift));
            // The lower lip rides the jaw.
            lo_top.push(self.at(view, x, MOUTH_Y + gap, lift));
            lo_bot.push(self.at(view, x, MOUTH_Y + gap + 0.06 * lens, lift));
            teeth.push(self.at(view, x, MOUTH_Y + 0.03 * lens, lift));
        }
        let band = |a: &[Vec2], b: &[Vec2], col: Color| {
            for i in 0..steps {
                draw_triangle(a[i], a[i + 1], b[i + 1], col);
                draw_triangle(a[i], b[i + 1], b[i], col);
            }
        };
        if open > 0.05 {
            band(&up_bot, &lo_top, Color::new(0.14, 0.04, 0.05, 1.0));
            if open > 0.3 {
                band(&up_bot, &teeth, Color::new(0.93, 0.91, 0.86, 1.0));
            }
        }
        band(&up_top, &up_bot, LIP_UP);
        band(&lo_top, &lo_bot, LIP_LO);
        // Shine on the lower lip, the line where the lips meet, and corners.
        let m = steps / 2;
        let a = lo_top[m - 2].lerp(lo_bot[m - 2], 0.4);
        let b = lo_top[m + 1].lerp(lo_bot[m + 1], 0.4);
        draw_line(a.x, a.y, b.x, b.y, 1.0, Color::new(1.0, 0.9, 0.88, 0.7));
        for i in 0..steps {
            draw_line(up_bot[i].x, up_bot[i].y, up_bot[i + 1].x, up_bot[i + 1].y, 0.8, Color::new(0.35, 0.15, 0.16, 1.0));
        }
        for x in [-MOUTH_W - 0.01, MOUTH_W + 0.01] {
            let p = self.at(view, x, MOUTH_Y + 0.005, lift);
            draw_circle(p.x, p.y, 0.7, Color::new(0.45, 0.25, 0.22, 0.8));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_has_skull_cheeks_jaw_and_chin() {
        let rim = trace();
        assert_eq!(rim.len(), RIM_SAMPLES);
        assert!(rim.iter().all(|&(_, r)| r > 0.2 && r < 1.5));
        let top = rim_at(&rim, -PI / 2.0);
        let chin = rim_at(&rim, PI / 2.0);
        let side = rim_at(&rim, 0.0);
        assert!(top > chin && chin > side, "top {top} chin {chin} side {side}");
        // Square jaw: still wide at the jaw angle, narrow at the chin.
        assert!(inside(0.75, 0.6));
        assert!(!inside(0.5, 1.0));
    }

    #[test]
    fn features_sit_on_skin_inside_the_head() {
        for (x, y) in [(EYE_X, EYE_Y), (-EYE_X, EYE_Y), (EYE_X + EYE_W, EYE_Y), (0.0, MOUTH_Y), (MOUTH_W, MOUTH_Y)] {
            assert!(inside(x, y), "{x},{y}");
            assert!(hair(x, y) < 0.01, "hair over feature at {x},{y}");
        }
        assert!(hair(0.0, -1.0) > 0.99);
    }

    #[test]
    fn nose_stands_out_and_sockets_sink() {
        let m = Mannequin::new();
        assert!(m.depth(0.0, 0.3) > m.depth(0.0, -0.5));
        assert!(m.depth(EYE_X, EYE_Y) < m.depth(0.0, EYE_Y));
    }
}
