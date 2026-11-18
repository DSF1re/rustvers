//! Силовая раскладка графа (Barnes–Hut). Без зависимостей от GUI.
//!
//! Повторяет поведение d3-force: пружины на рёбрах, отталкивание узлов,
//! слабое притяжение к центру и «остывание» `alpha`.

const ALPHA_MIN: f32 = 0.001;
const VELOCITY_KEEP: f32 = 0.6;
const CHARGE: f32 = -220.0;
const LINK_DISTANCE: f32 = 70.0;
const GRAVITY: f32 = 0.08;
const THETA2: f32 = 0.81; // точность Barnes–Hut (θ = 0.9)

fn alpha_decay() -> f32 {
    1.0 - ALPHA_MIN.powf(1.0 / 300.0)
}

#[derive(Debug, Clone)]
pub struct Body {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Закреплён (перетаскивается пользователем).
    pub fixed: Option<(f32, f32)>,
    pub degree: u32,
}

#[derive(Debug, Clone)]
struct Link {
    s: usize,
    t: usize,
    strength: f32,
    bias: f32,
}

pub struct Layout {
    pub bodies: Vec<Body>,
    links: Vec<Link>,
    pub alpha: f32,
    pub alpha_target: f32,
    ticks: u64,
}

/// Дешёвый детерминированный шум в [-0.5, 0.5) (splitmix64).
fn jitter(a: u64, b: u64) -> f32 {
    let mut z = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_add(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32 - 0.5
}

impl Layout {
    /// `positions` — стартовые координаты, `edges` — пары индексов.
    pub fn new(positions: Vec<(f32, f32)>, edges: &[(usize, usize)]) -> Self {
        let mut bodies: Vec<Body> = positions
            .into_iter()
            .map(|(x, y)| Body { x, y, vx: 0.0, vy: 0.0, fixed: None, degree: 0 })
            .collect();
        for &(s, t) in edges {
            bodies[s].degree += 1;
            bodies[t].degree += 1;
        }
        let links = edges
            .iter()
            .map(|&(s, t)| {
                let (ds, dt) = (bodies[s].degree as f32, bodies[t].degree as f32);
                Link { s, t, strength: 1.0 / ds.min(dt), bias: ds / (ds + dt) }
            })
            .collect();
        Self { bodies, links, alpha: 1.0, alpha_target: 0.0, ticks: 0 }
    }

    /// Начальная раскладка «подсолнухом», чтобы узлы не совпадали.
    pub fn phyllotaxis(i: usize) -> (f32, f32) {
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        let r = 12.0 * (i as f32 + 0.5).sqrt();
        let a = i as f32 * golden;
        (r * a.cos(), r * a.sin())
    }

    pub fn running(&self) -> bool {
        self.alpha >= ALPHA_MIN || self.alpha_target > 0.0
    }

    pub fn reheat(&mut self, alpha: f32) {
        self.alpha = self.alpha.max(alpha);
    }

    pub fn tick(&mut self) {
        self.alpha += (self.alpha_target - self.alpha) * alpha_decay();
        self.ticks += 1;
        let alpha = self.alpha;
        if self.bodies.is_empty() {
            return;
        }

        // Пружины связей.
        for l in &self.links {
            let (s, t) = (&self.bodies[l.s], &self.bodies[l.t]);
            let mut x = t.x + t.vx - s.x - s.vx;
            let mut y = t.y + t.vy - s.y - s.vy;
            let mut d = (x * x + y * y).sqrt();
            if d == 0.0 {
                d = 1e-3;
            }
            let k = (d - LINK_DISTANCE) / d * alpha * l.strength;
            x *= k;
            y *= k;
            let t = &mut self.bodies[l.t];
            t.vx -= x * l.bias;
            t.vy -= y * l.bias;
            let s = &mut self.bodies[l.s];
            s.vx += x * (1.0 - l.bias);
            s.vy += y * (1.0 - l.bias);
        }

        // Отталкивание через квадродерево.
        let tree = QuadTree::build(&self.bodies);
        let k = CHARGE * alpha;
        for i in 0..self.bodies.len() {
            let (fx, fy) = tree.force(&self.bodies, i, k, self.ticks);
            self.bodies[i].vx += fx;
            self.bodies[i].vy += fy;
        }

        // Притяжение к центру и интегрирование.
        for b in &mut self.bodies {
            b.vx -= b.x * GRAVITY * alpha;
            b.vy -= b.y * GRAVITY * alpha;
            if let Some((x, y)) = b.fixed {
                b.x = x;
                b.y = y;
                b.vx = 0.0;
                b.vy = 0.0;
                continue;
            }
            b.vx *= VELOCITY_KEEP;
            b.vy *= VELOCITY_KEEP;
            b.x += b.vx;
            b.y += b.vy;
        }
    }
}

// ---------------------------------------------------------------------------
// Квадродерево Barnes–Hut
// ---------------------------------------------------------------------------

const NONE: u32 = u32::MAX;

struct Quad {
    x: f32,
    y: f32,
    size: f32,
    mass: f32,
    cx: f32,
    cy: f32,
    body: u32,
    kids: [u32; 4],
}

struct QuadTree {
    quads: Vec<Quad>,
}

impl QuadTree {
    fn build(bodies: &[Body]) -> Self {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for b in bodies {
            x0 = x0.min(b.x);
            y0 = y0.min(b.y);
            x1 = x1.max(b.x);
            y1 = y1.max(b.y);
        }
        let size = (x1 - x0).max(y1 - y0).max(1.0) + 1.0;
        let mut tree = QuadTree { quads: Vec::with_capacity(bodies.len() * 2) };
        tree.quads.push(Quad::new(x0, y0, size));
        for (i, b) in bodies.iter().enumerate() {
            tree.insert(0, i as u32, b.x, b.y, bodies, 0);
        }
        tree
    }

    fn insert(&mut self, q: usize, i: u32, x: f32, y: f32, bodies: &[Body], depth: u32) {
        let quad = &mut self.quads[q];
        if quad.mass == 0.0 {
            quad.body = i;
            quad.mass = 1.0;
            quad.cx = x;
            quad.cy = y;
            return;
        }
        quad.cx = (quad.cx * quad.mass + x) / (quad.mass + 1.0);
        quad.cy = (quad.cy * quad.mass + y) / (quad.mass + 1.0);
        quad.mass += 1.0;
        if depth > 24 {
            return; // совпадающие точки — просто копим массу
        }
        if quad.kids == [NONE; 4] && quad.body != NONE {
            let old = std::mem::replace(&mut self.quads[q].body, NONE);
            let b = &bodies[old as usize];
            self.child(q, old, b.x, b.y, bodies, depth);
        }
        self.child(q, i, x, y, bodies, depth);
    }

    fn child(&mut self, q: usize, i: u32, x: f32, y: f32, bodies: &[Body], depth: u32) {
        let (qx, qy, h) = {
            let quad = &self.quads[q];
            (quad.x, quad.y, quad.size / 2.0)
        };
        let right = x >= qx + h;
        let bottom = y >= qy + h;
        let k = right as usize + 2 * bottom as usize;
        let mut kid = self.quads[q].kids[k];
        if kid == NONE {
            kid = self.quads.len() as u32;
            self.quads.push(Quad::new(qx + if right { h } else { 0.0 }, qy + if bottom { h } else { 0.0 }, h));
            self.quads[q].kids[k] = kid;
        }
        self.insert(kid as usize, i, x, y, bodies, depth + 1);
    }

    fn force(&self, bodies: &[Body], i: usize, k: f32, seed: u64) -> (f32, f32) {
        let b = &bodies[i];
        let mut stack = vec![0u32];
        let (mut fx, mut fy) = (0.0, 0.0);
        while let Some(q) = stack.pop() {
            let quad = &self.quads[q as usize];
            if quad.mass == 0.0 {
                continue;
            }
            let mut dx = quad.cx - b.x;
            let mut dy = quad.cy - b.y;
            let mut d2 = dx * dx + dy * dy;
            let is_leaf = quad.kids == [NONE; 4];
            if !is_leaf && quad.size * quad.size / d2 >= THETA2 {
                stack.extend(quad.kids.iter().copied().filter(|&c| c != NONE));
                continue;
            }
            if quad.body == i as u32 && quad.mass == 1.0 {
                continue;
            }
            if d2 < 1e-6 {
                // Совпадение — небольшой случайный сдвиг, разный на каждом шаге.
                dx = jitter(i as u64, seed) * 1e-2;
                dy = jitter(seed, i as u64 ^ 0xA5A5) * 1e-2;
                d2 = dx * dx + dy * dy;
            }
            if d2 < 100.0 {
                d2 = (100.0 * d2).sqrt(); // смягчение на малых расстояниях
            }
            let w = k * quad.mass / d2;
            fx += dx * w;
            fy += dy * w;
        }
        (fx, fy)
    }
}

impl Quad {
    fn new(x: f32, y: f32, size: f32) -> Self {
        Quad { x, y, size, mass: 0.0, cx: 0.0, cy: 0.0, body: NONE, kids: [NONE; 4] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(l: &Layout, a: usize, b: usize) -> f32 {
        let (p, q) = (&l.bodies[a], &l.bodies[b]);
        ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt()
    }

    #[test]
    fn settles_and_separates() {
        // Треугольник + одиночка.
        let pos = (0..4).map(Layout::phyllotaxis).collect();
        let mut l = Layout::new(pos, &[(0, 1), (1, 2), (2, 0)]);
        for _ in 0..400 {
            l.tick();
        }
        assert!(!l.running());
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let d = dist(&l, a, b);
            assert!((40.0..140.0).contains(&d), "ребро {a}-{b}: {d}");
        }
        assert!(l.bodies.iter().all(|b| b.x.is_finite() && b.y.is_finite()));
    }

    #[test]
    fn coincident_points_do_not_explode() {
        let mut l = Layout::new(vec![(0.0, 0.0); 50], &[]);
        for _ in 0..300 {
            l.tick();
        }
        assert!(l.bodies.iter().all(|b| b.x.is_finite() && b.x.abs() < 1e4));
    }

    #[test]
    fn fixed_body_stays() {
        let mut l = Layout::new(vec![(0.0, 0.0), (10.0, 0.0)], &[(0, 1)]);
        l.bodies[0].fixed = Some((5.0, 5.0));
        l.tick();
        assert_eq!((l.bodies[0].x, l.bodies[0].y), (5.0, 5.0));
    }
}
