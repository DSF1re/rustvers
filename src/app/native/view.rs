//! Состояние и отрисовка нативного графа: камера, ввод, подсветка.
//! Повторяет поведение `ui/js/graph.js`, но рисует через femtovg (OpenGL).

use std::collections::HashMap;

use femtovg::{Align, Baseline, Canvas, Color, FontId, LineJoin, Paint, Path, Renderer};

use crate::core::graph::{Graph, NodeKind};
use crate::core::layout::Layout;

const ZOOM_MIN: f32 = 0.05;
const ZOOM_MAX: f32 = 8.0;
const CAMERA_TAU: f32 = 90.0; // мс: сглаживание камеры
const INERTIA_TAU: f32 = 260.0; // мс: затухание инерции
const FADE_TAU: f32 = 70.0; // мс: подсветка при наведении
const DIM: f32 = 0.15; // прозрачность «потухших» элементов
const LABEL_SIZE: f32 = 12.0;

fn ease(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt / tau).exp()
}

fn with_alpha(mut c: Color, a: f32) -> Color {
    c.a *= a;
    c
}

/// Цвета берутся из CSS-переменных интерфейса.
#[derive(Clone)]
pub struct Theme {
    pub markdown: Color,
    pub html: Color,
    pub missing: Color,
    pub edge: Color,
    pub edge_hi: Color,
    pub label: Color,
    pub bg: Color,
    pub accent: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            markdown: Color::hex("#a598ff"),
            html: Color::hex("#4fd1b5"),
            missing: Color::hex("#5a5a66"),
            edge: Color::hex("#3a3a46"),
            edge_hi: Color::hex("#a598ffb0"),
            label: Color::hex("#c9c9d4"),
            bg: Color::hex("#1b1b21"),
            accent: Color::hex("#a598ff"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Options {
    pub show_missing: bool,
    pub show_orphans: bool,
    pub labels: bool,
}

/// Что граф просит сделать снаружи.
pub enum Effect {
    Open(String),
    /// Курсор «рука» (над узлом) или обычный.
    Cursor(bool),
}

#[derive(Clone, Copy, PartialEq)]
struct Cam {
    x: f32,
    y: f32,
    k: f32,
}

struct Anchor {
    sx: f32,
    sy: f32,
    wx: f32,
    wy: f32,
}

struct Node {
    id: String,
    title: String,
    kind: NodeKind,
    r: f32,
    neighbors: Vec<usize>,
}

struct Drag {
    node: Option<usize>,
    sx: f32,
    sy: f32,
    cam_x: f32,
    cam_y: f32,
    moved: bool,
    vx: f32,
    vy: f32,
    t: u32,
    px: f32,
    py: f32,
}

pub struct GraphView {
    all: Graph,
    nodes: Vec<Node>,
    layout: Layout,
    edges: Vec<(usize, usize)>,
    positions: HashMap<String, (f32, f32)>,
    view: Cam,
    goal: Cam,
    anchor: Option<Anchor>,
    inertia: (f32, f32),
    auto_fit: bool,
    hover: Option<usize>,
    focus: Option<usize>,
    fade: f32,
    selected: Option<String>,
    query: String,
    options: Options,
    pub theme: Theme,
    /// Логический размер области (CSS-пиксели).
    size: (f32, f32),
    drag: Option<Drag>,
    /// Нужно перерисовать, даже если ничего не анимируется.
    pub dirty: bool,
    pub effects: Vec<Effect>,
    pub show_fps: bool,
    fps: (u32, f32, u32),
}

impl GraphView {
    pub fn new() -> Self {
        GraphView {
            all: Graph::default(),
            nodes: Vec::new(),
            layout: Layout::new(Vec::new(), &[]),
            edges: Vec::new(),
            positions: HashMap::new(),
            view: Cam { x: 0.0, y: 0.0, k: 1.0 },
            goal: Cam { x: 0.0, y: 0.0, k: 1.0 },
            anchor: None,
            inertia: (0.0, 0.0),
            auto_fit: true,
            hover: None,
            focus: None,
            fade: 0.0,
            selected: None,
            query: String::new(),
            options: Options { show_missing: true, show_orphans: true, labels: true },
            theme: Theme::default(),
            size: (0.0, 0.0),
            drag: None,
            dirty: true,
            effects: Vec::new(),
            show_fps: false,
            fps: (0, 0.0, 0),
        }
    }

    // -- данные и настройки -----------------------------------------------------

    pub fn set_data(&mut self, graph: Graph, fresh: bool) {
        self.all = graph;
        self.rebuild(fresh);
    }

    pub fn set_options(&mut self, options: Options) {
        let structural = options.show_missing != self.options.show_missing
            || options.show_orphans != self.options.show_orphans;
        self.options = options;
        if structural {
            self.rebuild(false);
        }
        self.dirty = true;
    }

    pub fn set_selected(&mut self, id: Option<String>) {
        self.selected = id;
        self.dirty = true;
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = q.trim().to_lowercase();
        self.dirty = true;
    }

    pub fn set_size(&mut self, w: f32, h: f32) {
        if (w, h) == self.size {
            return;
        }
        self.size = (w, h);
        if self.auto_fit {
            self.fit(true);
        }
        self.dirty = true;
    }

    pub fn fit(&mut self, instant: bool) {
        let Some(target) = self.fit_view() else { return };
        self.anchor = None;
        self.inertia = (0.0, 0.0);
        if !instant {
            self.auto_fit = false;
        }
        self.goal = target;
        if instant {
            self.view = target;
        }
        self.dirty = true;
    }

    fn fit_view(&self) -> Option<Cam> {
        let (w, h) = self.size;
        if self.nodes.is_empty() || w <= 0.0 || h <= 0.0 {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (b, n) in self.layout.bodies.iter().zip(&self.nodes) {
            x0 = x0.min(b.x - n.r);
            x1 = x1.max(b.x + n.r);
            y0 = y0.min(b.y - n.r);
            y1 = y1.max(b.y + n.r);
        }
        let pad = 70.0;
        let k = ((w - pad * 2.0) / (x1 - x0).max(1.0))
            .min((h - pad * 2.0) / (y1 - y0).max(1.0))
            .clamp(ZOOM_MIN, 2.0);
        Some(Cam { k, x: w / 2.0 - (x0 + x1) / 2.0 * k, y: h / 2.0 - (y0 + y1) / 2.0 * k })
    }

    fn rebuild(&mut self, fresh: bool) {
        for (b, n) in self.layout.bodies.iter().zip(&self.nodes) {
            self.positions.insert(n.id.clone(), (b.x, b.y));
        }
        let raw = &self.all;
        let mut degree = vec![0u32; raw.nodes.len()];
        for &[s, t] in &raw.edges {
            degree[s] += 1;
            degree[t] += 1;
        }

        let mut map = vec![usize::MAX; raw.nodes.len()];
        let mut nodes = Vec::new();
        let mut start = Vec::new();
        for (i, n) in raw.nodes.iter().enumerate() {
            let keep = (self.options.show_missing || n.kind != NodeKind::Missing)
                && (self.options.show_orphans || degree[i] > 0);
            if !keep {
                continue;
            }
            map[i] = nodes.len();
            start.push(self.positions.get(&n.id).copied().unwrap_or_else(|| Layout::phyllotaxis(nodes.len())));
            nodes.push(Node { id: n.id.clone(), title: n.title.clone(), kind: n.kind, r: 0.0, neighbors: Vec::new() });
        }
        let mut edges = Vec::new();
        for &[s, t] in &raw.edges {
            let (a, b) = (map[s], map[t]);
            if a == usize::MAX || b == usize::MAX {
                continue;
            }
            edges.push((a, b));
            nodes[a].neighbors.push(b);
            nodes[b].neighbors.push(a);
        }

        self.layout = Layout::new(start, &edges);
        for (n, b) in nodes.iter_mut().zip(&self.layout.bodies) {
            n.r = 3.5 + (b.degree as f32).sqrt() * 1.8;
        }
        self.nodes = nodes;
        self.edges = edges;
        self.hover = None;
        self.focus = None;
        self.fade = 0.0;
        self.drag = None;

        if fresh {
            // Предрасчёт, чтобы первая картинка была уже почти «разложенной».
            let budget = std::time::Instant::now() + std::time::Duration::from_millis(150);
            while self.layout.alpha > 0.12 && std::time::Instant::now() < budget {
                self.layout.tick();
            }
            self.auto_fit = true;
            self.fit(true);
        } else {
            self.layout.reheat(0.3);
        }
        self.dirty = true;
    }

    // -- анимация ---------------------------------------------------------------

    /// Есть ли что анимировать (иначе поток может спать).
    pub fn busy(&self) -> bool {
        self.layout.running()
            || self.inertia != (0.0, 0.0)
            || self.view != self.goal
            || (self.fade - if self.hover.is_some() { 1.0 } else { 0.0 }).abs() > 0.0
            || self.show_fps
    }

    pub fn step(&mut self, dt_ms: f32) {
        let dt = dt_ms.clamp(1.0, 50.0);
        if self.layout.running() {
            self.layout.tick();
            if self.auto_fit
                && let Some(g) = self.fit_view() {
                    self.goal = g;
                }
        }
        self.update_camera(dt);
        self.update_fade(dt);

        let (frames, acc, _) = &mut self.fps;
        *frames += 1;
        *acc += dt_ms;
        if *acc >= 500.0 {
            self.fps.2 = (self.fps.0 as f32 * 1000.0 / self.fps.1).round() as u32;
            self.fps.0 = 0;
            self.fps.1 = 0.0;
        }
    }

    fn update_camera(&mut self, dt: f32) {
        let (vx, vy) = self.inertia;
        if vx != 0.0 || vy != 0.0 {
            self.view.x += vx * dt;
            self.view.y += vy * dt;
            self.goal.x = self.view.x;
            self.goal.y = self.view.y;
            let decay = (-dt / INERTIA_TAU).exp();
            self.inertia = (vx * decay, vy * decay);
            if vx.hypot(vy) < 0.01 {
                self.inertia = (0.0, 0.0);
            }
            return;
        }
        let a = ease(dt, CAMERA_TAU);
        let (v, g) = (&mut self.view, self.goal);
        // Масштаб — в логарифмах, так зум ощущается равномерным.
        v.k = (v.k.ln() + (g.k.ln() - v.k.ln()) * a).exp();
        if let Some(an) = &self.anchor {
            v.x = an.sx - an.wx * v.k;
            v.y = an.sy - an.wy * v.k;
        } else {
            v.x += (g.x - v.x) * a;
            v.y += (g.y - v.y) * a;
        }
        let done = (g.k / v.k).ln().abs() < 1e-3 && (g.x - v.x).abs() < 0.3 && (g.y - v.y).abs() < 0.3;
        if done {
            if self.anchor.take().is_some() {
                self.goal.x = self.view.x;
                self.goal.y = self.view.y;
            }
            self.view = self.goal;
        }
    }

    fn update_fade(&mut self, dt: f32) {
        if self.hover.is_some() {
            self.focus = self.hover;
        }
        let target = if self.hover.is_some() { 1.0 } else { 0.0 };
        self.fade += (target - self.fade) * ease(dt, FADE_TAU);
        if (target - self.fade).abs() < 0.01 {
            self.fade = target;
            if target == 0.0 {
                self.focus = None;
            }
        }
    }

    // -- ввод (координаты — логические пиксели внутри области) -----------------

    fn to_world(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.view.x) / self.view.k, (y - self.view.y) / self.view.k)
    }

    fn node_at(&self, x: f32, y: f32) -> Option<usize> {
        let (wx, wy) = self.to_world(x, y);
        let slop = 4.0 / self.view.k;
        let mut best = None;
        let mut best_d = f32::MAX;
        for (i, (b, n)) in self.layout.bodies.iter().zip(&self.nodes).enumerate() {
            let d = (b.x - wx).powi(2) + (b.y - wy).powi(2);
            let r = n.r + slop;
            if d < r * r && d < best_d {
                best = Some(i);
                best_d = d;
            }
        }
        best
    }

    fn set_hover(&mut self, n: Option<usize>) {
        if n != self.hover {
            if n.is_some() != self.hover.is_some() {
                self.effects.push(Effect::Cursor(n.is_some()));
            }
            self.hover = n;
            self.dirty = true;
        }
    }

    fn take_control(&mut self) {
        self.auto_fit = false;
        self.anchor = None;
        self.inertia = (0.0, 0.0);
        self.goal = self.view;
    }

    pub fn pointer_down(&mut self, x: f32, y: f32, t: u32) {
        self.take_control();
        let node = self.node_at(x, y);
        if let Some(i) = node {
            let b = &mut self.layout.bodies[i];
            b.fixed = Some((b.x, b.y));
        }
        self.drag = Some(Drag {
            node, sx: x, sy: y, cam_x: self.view.x, cam_y: self.view.y,
            moved: false, vx: 0.0, vy: 0.0, t, px: x, py: y,
        });
        self.dirty = true;
    }

    pub fn pointer_move(&mut self, x: f32, y: f32, t: u32) {
        let Some(mut drag) = self.drag.take() else {
            let n = self.node_at(x, y);
            self.set_hover(n);
            return;
        };
        let (dx, dy) = (x - drag.sx, y - drag.sy);
        if drag.moved || dx * dx + dy * dy >= 9.0 {
            drag.moved = true;
            if let Some(i) = drag.node {
                let p = self.to_world(x, y);
                self.layout.bodies[i].fixed = Some(p);
                self.layout.alpha_target = 0.1;
                self.layout.reheat(0.1);
            } else {
                self.view.x = drag.cam_x + dx;
                self.view.y = drag.cam_y + dy;
                self.goal = self.view;
                // Скорость для инерции (сглаженная, px/мс).
                let dtm = t.wrapping_sub(drag.t).max(1) as f32;
                drag.vx = drag.vx * 0.6 + (x - drag.px) / dtm * 0.4;
                drag.vy = drag.vy * 0.6 + (y - drag.py) / dtm * 0.4;
                drag.t = t;
                drag.px = x;
                drag.py = y;
            }
            self.dirty = true;
        }
        self.drag = Some(drag);
    }

    pub fn pointer_up(&mut self, t: u32) {
        let Some(drag) = self.drag.take() else { return };
        if let Some(i) = drag.node {
            self.layout.bodies[i].fixed = None;
            self.layout.alpha_target = 0.0;
            if !drag.moved {
                self.effects.push(Effect::Open(self.nodes[i].id.clone()));
            }
        } else if drag.moved && t.wrapping_sub(drag.t) < 80 && drag.vx.hypot(drag.vy) > 0.15 {
            // Отпустили «на ходу» — холст едет дальше и плавно тормозит.
            self.inertia = (drag.vx, drag.vy);
        }
        self.dirty = true;
    }

    pub fn pointer_leave(&mut self) {
        if self.drag.is_none() {
            self.set_hover(None);
        }
    }

    /// `dy` > 0 — от себя (уменьшить).
    pub fn scroll(&mut self, x: f32, y: f32, dy: f32) {
        self.auto_fit = false;
        self.inertia = (0.0, 0.0);
        let k = (self.goal.k * (-dy * 0.22).exp()).clamp(ZOOM_MIN, ZOOM_MAX);
        let (wx, wy) = self.to_world(x, y);
        self.anchor = Some(Anchor { sx: x, sy: y, wx, wy });
        self.goal = Cam { k, x: x - wx * k, y: y - wy * k };
        self.dirty = true;
    }

    // -- отрисовка ----------------------------------------------------------------

    pub fn draw<T: Renderer>(&self, canvas: &mut Canvas<T>, font: Option<FontId>, dpr: f32) {
        let th = &self.theme;
        let (w, h) = (canvas.width(), canvas.height());
        canvas.reset_transform();
        canvas.clear_rect(0, 0, w, h, th.bg);
        let v = self.view;

        let focus = self.focus;
        let fade = self.fade;
        let q = &self.query;
        let matches = |n: &Node| q.is_empty() || n.title.to_lowercase().contains(q) || n.id.to_lowercase().contains(q);
        let lit = |i: usize| focus.is_none_or(|f| f == i || self.nodes[f].neighbors.contains(&i));
        let touches = |a: usize, b: usize| focus.is_some_and(|f| a == f || b == f);
        let dim = 1.0 - (1.0 - DIM) * fade;
        let bodies = &self.layout.bodies;

        canvas.save();
        canvas.scale(dpr, dpr);
        canvas.translate(v.x, v.y);
        canvas.scale(v.k, v.k);

        // Рёбра: обычные, затем подсвеченные поверх.
        let width = (1.0 / v.k).max(0.6);
        let mut path = Path::new();
        let mut hi = Path::new();
        for &(a, b) in &self.edges {
            let p = if touches(a, b) { &mut hi } else { &mut path };
            p.move_to(bodies[a].x, bodies[a].y);
            p.line_to(bodies[b].x, bodies[b].y);
        }
        let base_alpha = if q.is_empty() { 1.0 } else { 0.35 } * if focus.is_some() { (dim * 2.0).min(1.0) } else { 1.0 };
        canvas.stroke_path(&path, &Paint::color(with_alpha(th.edge, base_alpha)).with_line_width(width));
        if focus.is_some() {
            canvas.stroke_path(&hi, &Paint::color(th.edge).with_line_width(width));
            let hw = (1.6 / v.k).max(1.2);
            canvas.stroke_path(&hi, &Paint::color(with_alpha(th.edge_hi, fade)).with_line_width(hw));
        }

        // Узлы.
        for (i, (n, b)) in self.nodes.iter().zip(bodies).enumerate() {
            let a = (if lit(i) { 1.0 } else { dim }) * if matches(n) { 1.0 } else { DIM };
            let color = match n.kind {
                NodeKind::Markdown => th.markdown,
                NodeKind::Html => th.html,
                NodeKind::Missing => th.missing,
            };
            let r = if focus == Some(i) { n.r * (1.0 + 0.25 * fade) } else { n.r };
            let mut p = Path::new();
            p.circle(b.x, b.y, r);
            canvas.fill_path(&p, &Paint::color(with_alpha(color, a)));
        }
        if let Some(i) = self.selected.as_ref().and_then(|id| self.nodes.iter().position(|n| &n.id == id)) {
            let mut p = Path::new();
            p.circle(bodies[i].x, bodies[i].y, self.nodes[i].r + 4.0 / v.k);
            canvas.stroke_path(&p, &Paint::color(th.accent).with_line_width(2.0 / v.k));
        }
        canvas.restore();

        // Подписи — в экранных координатах, шрифт не масштабируется.
        let Some(font) = font else { return };
        canvas.save();
        canvas.scale(dpr, dpr);
        let text = Paint::color(th.label)
            .with_font(&[font])
            .with_font_size(LABEL_SIZE)
            .with_text_align(Align::Center)
            .with_text_baseline(Baseline::Top);
        let outline = text.clone().with_color(th.bg).with_line_width(3.0).with_line_join(LineJoin::Round);
        let zoom_fade = ((v.k - 0.55) / 0.35).clamp(0.0, 1.0);
        let base = if self.options.labels { zoom_fade } else { 0.0 };
        let (cw, ch) = self.size;
        let selected = self.selected.as_deref();
        for (i, (n, b)) in self.nodes.iter().zip(bodies).enumerate() {
            let near = focus.is_some() && lit(i);
            let pinned = Some(n.id.as_str()) == selected || (!q.is_empty() && matches(n));
            let mut a = if pinned { 1.0 } else if near { base + (1.0 - base) * fade } else { base };
            if focus.is_some() && !near {
                a *= dim;
            }
            if !q.is_empty() && !matches(n) {
                a *= DIM;
            }
            if a < 0.03 {
                continue;
            }
            let sx = b.x * v.k + v.x;
            let sy = (b.y + n.r) * v.k + v.y + 4.0;
            if sx < -100.0 || sx > cw + 100.0 || sy < -20.0 || sy > ch + 20.0 {
                continue;
            }
            let label: String = if n.title.chars().count() > 40 {
                n.title.chars().take(39).chain(std::iter::once('…')).collect()
            } else {
                n.title.clone()
            };
            let _ = canvas.stroke_text(sx, sy, &label, &outline.clone().with_color(with_alpha(th.bg, a)));
            let _ = canvas.fill_text(sx, sy, &label, &text.clone().with_color(with_alpha(th.label, a)));
        }
        self.draw_legend(canvas, font);
        canvas.restore();
    }

    /// Легенда и счётчик кадров (HTML поверх подповерхности не виден).
    fn draw_legend<T: Renderer>(&self, canvas: &mut Canvas<T>, font: FontId) {
        let th = &self.theme;
        let (_, h) = self.size;
        let text = Paint::color(with_alpha(th.label, 0.8))
            .with_font(&[font])
            .with_font_size(LABEL_SIZE)
            .with_text_align(Align::Left)
            .with_text_baseline(Baseline::Middle);
        let mut x = 16.0;
        let y = h - 22.0;
        for (color, label) in [(th.markdown, "Markdown"), (th.html, "HTML"), (th.missing, "Нет файла")] {
            let mut p = Path::new();
            p.circle(x + 4.5, y, 4.5);
            canvas.fill_path(&p, &Paint::color(color));
            let m = canvas.fill_text(x + 14.0, y, label, &text);
            x += 14.0 + m.map_or(60.0, |m| m.width()) + 14.0;
        }
        if self.show_fps {
            let p = Paint::color(Color::hex("#6ee7a8"))
                .with_font(&[font])
                .with_font_size(13.0)
                .with_text_align(Align::Left)
                .with_text_baseline(Baseline::Top);
            let _ = canvas.fill_text(12.0, 10.0, format!("{} fps (нативный)", self.fps.2), &p);
        }
    }
}
