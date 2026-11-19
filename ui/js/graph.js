// Граф связей: силовая раскладка (Barnes–Hut) + отрисовка на canvas.
// Без зависимостей. Публичный API — класс GraphView.

const ALPHA_MIN = 0.001;
const ALPHA_DECAY = 1 - Math.pow(ALPHA_MIN, 1 / 300);
const VELOCITY_KEEP = 0.6;
const CHARGE = -220;
const LINK_DISTANCE = 70;
const GRAVITY = 0.08;
const THETA2 = 0.81; // точность Barnes–Hut (θ = 0.9)

// ---------------------------------------------------------------------------
// Симуляция
// ---------------------------------------------------------------------------

class Quad {
  constructor(x, y, size) {
    this.x = x; this.y = y; this.s = size;
    this.m = 0; this.cx = 0; this.cy = 0;
    this.body = null; this.kids = null;
  }
}

function quadInsert(q, n, depth) {
  if (q.m === 0) { q.body = n; q.m = 1; q.cx = n.x; q.cy = n.y; return; }
  q.cx = (q.cx * q.m + n.x) / (q.m + 1);
  q.cy = (q.cy * q.m + n.y) / (q.m + 1);
  q.m++;
  if (depth > 24) return; // совпадающие точки — просто копим массу
  if (!q.kids) {
    q.kids = [null, null, null, null];
    const b = q.body; q.body = null;
    quadChild(q, b, depth);
  }
  quadChild(q, n, depth);
}

function quadChild(q, n, depth) {
  const h = q.s / 2;
  const i = (n.x >= q.x + h ? 1 : 0) + (n.y >= q.y + h ? 2 : 0);
  if (!q.kids[i]) q.kids[i] = new Quad(q.x + (i & 1 ? h : 0), q.y + (i & 2 ? h : 0), h);
  quadInsert(q.kids[i], n, depth + 1);
}

function buildTree(nodes) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const n of nodes) {
    if (n.x < x0) x0 = n.x; if (n.x > x1) x1 = n.x;
    if (n.y < y0) y0 = n.y; if (n.y > y1) y1 = n.y;
  }
  const root = new Quad(x0, y0, Math.max(x1 - x0, y1 - y0, 1) + 1);
  for (const n of nodes) quadInsert(root, n, 0);
  return root;
}

function repel(q, n, k) {
  if (!q || q.m === 0) return;
  let dx = q.cx - n.x, dy = q.cy - n.y;
  let d2 = dx * dx + dy * dy;
  if (q.kids && (q.s * q.s) / d2 >= THETA2) {
    for (const c of q.kids) repel(c, n, k);
    return;
  }
  if (q.body === n && q.m === 1) return;
  if (d2 < 1e-6) { dx = (Math.random() - 0.5) * 1e-2; dy = (Math.random() - 0.5) * 1e-2; d2 = dx * dx + dy * dy; }
  if (d2 < 100) d2 = Math.sqrt(100 * d2); // смягчение на малых расстояниях
  const w = (k * q.m) / d2;
  n.vx += dx * w;
  n.vy += dy * w;
}

class Simulation {
  constructor() {
    this.nodes = [];
    this.links = [];
    this.alpha = 1;
    this.alphaTarget = 0;
  }

  setData(nodes, links) {
    this.nodes = nodes;
    this.links = links;
    for (const l of links) { l.source.deg++; l.target.deg++; }
    for (const l of links) {
      l.strength = 1 / Math.min(l.source.deg, l.target.deg);
      l.bias = l.source.deg / (l.source.deg + l.target.deg);
    }
  }

  get running() { return this.alpha >= ALPHA_MIN || this.alphaTarget > 0; }

  reheat(a = 0.5) { this.alpha = Math.max(this.alpha, a); }

  tick() {
    const { nodes, links } = this;
    this.alpha += (this.alphaTarget - this.alpha) * ALPHA_DECAY;
    const alpha = this.alpha;
    if (!nodes.length) return;

    // Пружины связей.
    for (const l of links) {
      const s = l.source, t = l.target;
      let x = t.x + t.vx - s.x - s.vx;
      let y = t.y + t.vy - s.y - s.vy;
      let d = Math.sqrt(x * x + y * y) || 1e-3;
      d = ((d - LINK_DISTANCE) / d) * alpha * l.strength;
      x *= d; y *= d;
      t.vx -= x * l.bias; t.vy -= y * l.bias;
      s.vx += x * (1 - l.bias); s.vy += y * (1 - l.bias);
    }

    // Отталкивание.
    const tree = buildTree(nodes);
    const k = CHARGE * alpha;
    for (const n of nodes) repel(tree, n, k);

    // Слабое притяжение к центру + интегрирование.
    for (const n of nodes) {
      n.vx -= n.x * GRAVITY * alpha;
      n.vy -= n.y * GRAVITY * alpha;
      if (n.fx != null) { n.x = n.fx; n.y = n.fy; n.vx = n.vy = 0; continue; }
      n.vx *= VELOCITY_KEEP; n.vy *= VELOCITY_KEEP;
      n.x += n.vx; n.y += n.vy;
    }
  }
}

// ---------------------------------------------------------------------------
// Вид
// ---------------------------------------------------------------------------

const ZOOM_MIN = 0.05;
const ZOOM_MAX = 8;
const CAMERA_TAU = 90;    // мс: время сглаживания камеры (зум, автоподгонка)
const INERTIA_TAU = 260;  // мс: затухание инерции после перетаскивания холста
const FADE_TAU = 70;      // мс: появление/исчезновение подсветки при наведении
const DIM = 0.15;         // прозрачность «неактивных» элементов

/** Экспоненциальное сглаживание, не зависящее от частоты кадров. */
const ease = (dt, tau) => 1 - Math.exp(-dt / tau);

export class GraphView {
  /**
   * @param {HTMLCanvasElement} canvas
   * @param {{ onOpen?: (id: string) => void }} opts
   */
  constructor(canvas, { onOpen } = {}) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d");
    this.onOpen = onOpen || (() => {});
    this.sim = new Simulation();
    // Камера: view — то, что на экране; goal — куда она плавно едет.
    this.view = { x: 0, y: 0, k: 1 };
    this.goal = { x: 0, y: 0, k: 1 };
    this.anchor = null;        // точка под курсором, которая не должна «уплывать» при зуме
    this.inertia = { vx: 0, vy: 0 };
    this.autoFit = true;
    this.options = { showMissing: true, showOrphans: true, labels: true };
    this.all = { nodes: [], edges: [] };
    this.positions = new Map(); // id → {x, y}: сохраняем раскладку между пересборками
    this.hover = null;
    this.focus = null;         // последний наведённый узел (живёт, пока гаснет подсветка)
    this.fade = 0;             // 0…1 — сила подсветки
    this.selected = null;
    this.query = "";
    this.dpr = 1;
    this.frame = 0;
    this.last = 0;
    this.visible = true;
    this.readColors();
    this.bindEvents();
    new ResizeObserver(() => this.resize()).observe(canvas);
    matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => { this.readColors(); this.kick(); });
  }

  /** @param {{nodes: {id:string,title:string,kind:string}[], edges: [number,number][]}} graph */
  setData(graph, { fresh = true } = {}) {
    this.all = graph;
    this.rebuild(fresh);
  }

  /** Заметку переименовали: узел сохраняет своё место в раскладке. */
  moveNode(from, to) {
    const n = this.byId?.get(from);
    if (n) this.positions.set(to, { x: n.x, y: n.y });
    if (this.selected?.id === from) this.selected = null;
  }

  setOptions(opts) {
    const structural = opts.showMissing !== this.options.showMissing || opts.showOrphans !== this.options.showOrphans;
    Object.assign(this.options, opts);
    if (structural) this.rebuild(false); else this.kick();
  }

  setSelected(id) {
    this.selected = id ? this.byId.get(id) || null : null;
    this.kick();
  }

  setQuery(q) {
    this.query = q.trim().toLowerCase();
    this.kick();
  }

  setVisible(v) {
    this.visible = v;
    if (v) { this.resize(); this.kick(); }
  }

  /** Вписать граф в экран. `instant` — без анимации. */
  fit(instant = false) {
    const target = this.fitView();
    if (!target) return;
    this.anchor = null;
    this.inertia.vx = this.inertia.vy = 0;
    this.goal = target;
    if (instant) this.view = { ...target };
    this.kick();
  }

  fitView() {
    const nodes = this.sim.nodes;
    const w = this.canvas.clientWidth, h = this.canvas.clientHeight;
    if (!nodes.length || !w || !h) return null;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of nodes) {
      x0 = Math.min(x0, n.x - n.r); x1 = Math.max(x1, n.x + n.r);
      y0 = Math.min(y0, n.y - n.r); y1 = Math.max(y1, n.y + n.r);
    }
    const pad = 70;
    const k = Math.min(2, Math.max(ZOOM_MIN, Math.min((w - pad * 2) / Math.max(x1 - x0, 1), (h - pad * 2) / Math.max(y1 - y0, 1))));
    return { k, x: w / 2 - ((x0 + x1) / 2) * k, y: h / 2 - ((y0 + y1) / 2) * k };
  }

  // -- данные ---------------------------------------------------------------

  rebuild(fresh) {
    const { nodes: rawNodes, edges: rawEdges } = this.all;
    for (const n of this.sim.nodes) this.positions.set(n.id, { x: n.x, y: n.y });

    const degree = new Array(rawNodes.length).fill(0);
    for (const [s, t] of rawEdges) { degree[s]++; degree[t]++; }

    const keep = rawNodes.map((n, i) =>
      (this.options.showMissing || n.kind !== "missing") && (this.options.showOrphans || degree[i] > 0));

    const nodes = [], map = new Map(), golden = Math.PI * (3 - Math.sqrt(5));
    rawNodes.forEach((n, i) => {
      if (!keep[i]) return;
      const old = this.positions.get(n.id);
      const j = nodes.length, r = 12 * Math.sqrt(j + 0.5), a = j * golden; // филлотаксис
      const node = { ...n, x: old ? old.x : r * Math.cos(a), y: old ? old.y : r * Math.sin(a), vx: 0, vy: 0, deg: 0, fx: null, fy: null, neighbors: new Set() };
      map.set(i, node);
      nodes.push(node);
    });
    const links = [];
    for (const [s, t] of rawEdges) {
      const a = map.get(s), b = map.get(t);
      if (!a || !b) continue;
      links.push({ source: a, target: b });
      a.neighbors.add(b); b.neighbors.add(a);
    }

    this.sim.setData(nodes, links);
    for (const n of nodes) n.r = 3.5 + Math.sqrt(n.deg) * 1.8;
    this.byId = new Map(nodes.map((n) => [n.id, n]));
    if (this.selected) this.selected = this.byId.get(this.selected.id) || null;
    this.hover = this.focus = null;
    this.fade = 0;

    if (fresh) {
      // Предрасчёт, чтобы первая картинка была уже почти «разложенной».
      this.sim.alpha = 1;
      const budget = performance.now() + 150;
      while (this.sim.alpha > 0.12 && performance.now() < budget) this.sim.tick();
      this.autoFit = true;
      this.fit(true);
    } else {
      this.sim.reheat(0.3);
    }
    this.kick();
  }

  // -- цикл -------------------------------------------------------------------

  /** Запустить цикл кадров, если он стоит. Сам остановится, когда всё замрёт. */
  kick() {
    if (this.frame || !this.visible) return;
    this.last = performance.now();
    this.frame = requestAnimationFrame((t) => this.step(t));
  }

  step(now) {
    this.frame = 0;
    const dt = Math.min(50, Math.max(1, now - this.last));
    this.last = now;

    if (this.sim.running) this.sim.tick();
    // Пока пользователь не трогал вид, камера мягко следит за раскладкой.
    if (this.autoFit && this.sim.running) this.goal = this.fitView() || this.goal;

    const busy = this.updateCamera(dt) | this.updateFade(dt) | this.sim.running;
    this.draw();
    if (busy && this.visible) this.frame = requestAnimationFrame((t) => this.step(t));
  }

  /** @returns {boolean} камера ещё движется */
  updateCamera(dt) {
    const v = this.view, g = this.goal, inr = this.inertia;

    if (inr.vx || inr.vy) {
      v.x += inr.vx * dt; v.y += inr.vy * dt;
      g.x = v.x; g.y = v.y;
      const decay = Math.exp(-dt / INERTIA_TAU);
      inr.vx *= decay; inr.vy *= decay;
      if (Math.hypot(inr.vx, inr.vy) < 0.01) inr.vx = inr.vy = 0;
      return true;
    }

    const a = ease(dt, CAMERA_TAU);
    // Масштаб интерполируем в логарифмах — так зум ощущается равномерным.
    v.k = Math.exp(Math.log(v.k) + (Math.log(g.k) - Math.log(v.k)) * a);
    if (this.anchor) {
      // Точка под курсором остаётся на месте на протяжении всей анимации.
      const { sx, sy, wx, wy } = this.anchor;
      v.x = sx - wx * v.k; v.y = sy - wy * v.k;
    } else {
      v.x += (g.x - v.x) * a;
      v.y += (g.y - v.y) * a;
    }

    const done = Math.abs(Math.log(g.k / v.k)) < 1e-3 && Math.abs(g.x - v.x) < 0.3 && Math.abs(g.y - v.y) < 0.3;
    if (done) {
      if (this.anchor) { g.x = v.x; g.y = v.y; }
      Object.assign(v, g);
      this.anchor = null;
    }
    return !done;
  }

  /** @returns {boolean} подсветка ещё меняется */
  updateFade(dt) {
    if (this.hover) this.focus = this.hover;
    const target = this.hover ? 1 : 0;
    this.fade += (target - this.fade) * ease(dt, FADE_TAU);
    if (Math.abs(target - this.fade) < 0.01) {
      this.fade = target;
      if (!target) this.focus = null;
      return false;
    }
    return true;
  }

  resize() {
    const c = this.canvas;
    this.dpr = window.devicePixelRatio || 1;
    const w = Math.round(c.clientWidth * this.dpr), h = Math.round(c.clientHeight * this.dpr);
    if (c.width !== w || c.height !== h) {
      c.width = w; c.height = h;
      if (this.autoFit) this.fit(true);
      this.draw();
    }
  }

  readColors() {
    const css = getComputedStyle(document.documentElement);
    const v = (name) => css.getPropertyValue(name).trim();
    this.colors = {
      markdown: v("--node-md"), html: v("--node-html"), missing: v("--node-missing"),
      edge: v("--edge"), edgeHi: v("--edge-hi"), label: v("--label"), bg: v("--bg"), accent: v("--accent"),
    };
  }

  // -- отрисовка ----------------------------------------------------------------

  draw() {
    if (!this.visible) return;
    const { ctx, view, colors, dpr, fade } = this;
    const w = this.canvas.width, h = this.canvas.height;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, w, h);
    ctx.setTransform(dpr * view.k, 0, 0, dpr * view.k, dpr * view.x, dpr * view.y);

    const focus = this.focus;
    const q = this.query;
    const matches = (n) => !q || n.title.toLowerCase().includes(q) || n.id.toLowerCase().includes(q);
    const lit = (n) => !focus || n === focus || focus.neighbors.has(n);
    const dim = 1 - (1 - DIM) * fade; // прозрачность «потухших» элементов
    const touches = (l) => focus && (l.source === focus || l.target === focus);

    // Рёбра: обычные, затем подсвеченные поверх.
    ctx.lineWidth = Math.max(0.6, 1 / view.k);
    ctx.strokeStyle = colors.edge;
    ctx.globalAlpha = (q ? 0.35 : 1) * (focus ? Math.min(dim * 2, 1) : 1);
    ctx.beginPath();
    for (const l of this.sim.links) {
      if (touches(l)) continue;
      ctx.moveTo(l.source.x, l.source.y);
      ctx.lineTo(l.target.x, l.target.y);
    }
    ctx.stroke();
    if (focus) {
      // Сначала обычная толщина/цвет, поверх — акцент с силой fade.
      ctx.beginPath();
      for (const l of this.sim.links) {
        if (!touches(l)) continue;
        ctx.moveTo(l.source.x, l.source.y);
        ctx.lineTo(l.target.x, l.target.y);
      }
      ctx.globalAlpha = 1;
      ctx.stroke();
      ctx.globalAlpha = fade;
      ctx.strokeStyle = colors.edgeHi;
      ctx.lineWidth = Math.max(1.2, 1.6 / view.k);
      ctx.stroke();
    }

    // Узлы.
    for (const n of this.sim.nodes) {
      ctx.globalAlpha = (lit(n) ? 1 : dim) * (matches(n) ? 1 : DIM);
      ctx.fillStyle = colors[n.kind] || colors.markdown;
      ctx.beginPath();
      ctx.arc(n.x, n.y, n === focus ? n.r * (1 + 0.25 * fade) : n.r, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
    if (this.selected) {
      const n = this.selected;
      ctx.strokeStyle = colors.accent;
      ctx.lineWidth = 2 / view.k;
      ctx.beginPath();
      ctx.arc(n.x, n.y, n.r + 4 / view.k, 0, Math.PI * 2);
      ctx.stroke();
    }

    // Подписи: в экранных координатах, чтобы шрифт не масштабировался.
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.font = "12px system-ui, -apple-system, 'Segoe UI', sans-serif";
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    ctx.fillStyle = colors.label;
    ctx.strokeStyle = colors.bg;
    ctx.lineWidth = 3;
    ctx.lineJoin = "round";
    const zoomFade = Math.min(1, Math.max(0, (view.k - 0.55) / 0.35));
    const base = this.options.labels ? zoomFade : 0;
    const cw = this.canvas.clientWidth, ch = this.canvas.clientHeight;
    for (const n of this.sim.nodes) {
      const near = focus && lit(n);
      const pinned = n === this.selected || (q && matches(n));
      // Подписи соседей наведённого узла плавно проявляются вместе с подсветкой.
      let a = pinned ? 1 : near ? base + (1 - base) * fade : base;
      if (focus && !near) a *= dim;
      if (q && !matches(n)) a *= DIM;
      if (a < 0.03) continue;
      const sx = n.x * view.k + view.x, sy = (n.y + n.r) * view.k + view.y + 4;
      if (sx < -100 || sx > cw + 100 || sy < -20 || sy > ch + 20) continue;
      ctx.globalAlpha = a;
      const text = n.title.length > 40 ? n.title.slice(0, 39) + "…" : n.title;
      ctx.strokeText(text, sx, sy);
      ctx.fillText(text, sx, sy);
    }
    ctx.globalAlpha = 1;
  }

  // -- ввод ---------------------------------------------------------------------

  toWorld(e) {
    const r = this.canvas.getBoundingClientRect();
    return { x: (e.clientX - r.left - this.view.x) / this.view.k, y: (e.clientY - r.top - this.view.y) / this.view.k };
  }

  nodeAt(p) {
    let best = null, bestD = Infinity;
    const slop = 4 / this.view.k;
    for (const n of this.sim.nodes) {
      const dx = n.x - p.x, dy = n.y - p.y, d = dx * dx + dy * dy, r = n.r + slop;
      if (d < r * r && d < bestD) { best = n; bestD = d; }
    }
    return best;
  }

  setHover(n) {
    if (n === this.hover) return;
    this.hover = n;
    this.canvas.classList.toggle("over-node", !!n);
    this.canvas.title = n ? n.id.replace(/^\?/, "") : "";
    this.kick();
  }

  /** Остановить любое движение камеры (пользователь взялся за управление). */
  takeControl() {
    this.autoFit = false;
    this.anchor = null;
    this.inertia.vx = this.inertia.vy = 0;
    this.goal = { ...this.view };
  }

  bindEvents() {
    const c = this.canvas;
    let drag = null;

    c.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      c.setPointerCapture(e.pointerId);
      this.takeControl();
      const node = this.nodeAt(this.toWorld(e));
      drag = { node, sx: e.clientX, sy: e.clientY, x: this.view.x, y: this.view.y, moved: false, vx: 0, vy: 0, t: e.timeStamp, px: e.clientX, py: e.clientY };
      if (node) { node.fx = node.x; node.fy = node.y; }
      else c.classList.add("panning");
    });

    c.addEventListener("pointermove", (e) => {
      if (!drag) { this.setHover(this.nodeAt(this.toWorld(e))); return; }
      const dx = e.clientX - drag.sx, dy = e.clientY - drag.sy;
      if (!drag.moved && dx * dx + dy * dy < 9) return;
      drag.moved = true;
      if (drag.node) {
        const p = this.toWorld(e);
        drag.node.fx = p.x; drag.node.fy = p.y;
        this.sim.alphaTarget = 0.1;
        this.sim.reheat(0.1);
      } else {
        this.view.x = this.goal.x = drag.x + dx;
        this.view.y = this.goal.y = drag.y + dy;
        // Скорость для инерции (сглаженная, px/мс).
        const dtm = Math.max(1, e.timeStamp - drag.t);
        drag.vx = drag.vx * 0.6 + ((e.clientX - drag.px) / dtm) * 0.4;
        drag.vy = drag.vy * 0.6 + ((e.clientY - drag.py) / dtm) * 0.4;
        drag.t = e.timeStamp; drag.px = e.clientX; drag.py = e.clientY;
      }
      this.kick();
    });

    const end = (e) => {
      if (!drag) return;
      // Сначала отпускаем узел: ошибка в onOpen не должна оставить его на курсоре.
      const d = drag;
      drag = null;
      c.classList.remove("panning");
      if (c.hasPointerCapture(e.pointerId)) c.releasePointerCapture(e.pointerId);
      if (d.node) {
        d.node.fx = d.node.fy = null;
        this.sim.alphaTarget = 0;
      } else if (d.moved && e.timeStamp - d.t < 80 && Math.hypot(d.vx, d.vy) > 0.15) {
        // Отпустили «на ходу» — холст продолжает ехать и плавно тормозит.
        this.inertia.vx = d.vx;
        this.inertia.vy = d.vy;
      }
      this.kick();
      if (d.node && !d.moved) this.onOpen(d.node.id);
    };
    c.addEventListener("pointerup", end);
    c.addEventListener("pointercancel", end);
    c.addEventListener("pointerleave", () => { if (!drag) this.setHover(null); });

    c.addEventListener("wheel", (e) => {
      e.preventDefault();
      this.autoFit = false;
      this.inertia.vx = this.inertia.vy = 0;
      const r = c.getBoundingClientRect();
      const sx = e.clientX - r.left, sy = e.clientY - r.top;
      // Мышь даёт крупные шаги, тачпад — мелкие; оба превращаются в плавный зум.
      const delta = e.deltaMode === 1 ? e.deltaY * 33 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
      const k = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, this.goal.k * Math.exp(-delta * 0.0022)));
      // Якорь — мировая точка под курсором в текущем кадре.
      this.anchor = { sx, sy, wx: (sx - this.view.x) / this.view.k, wy: (sy - this.view.y) / this.view.k };
      this.goal.k = k;
      this.goal.x = sx - this.anchor.wx * k;
      this.goal.y = sy - this.anchor.wy * k;
      this.kick();
    }, { passive: false });
  }
}
