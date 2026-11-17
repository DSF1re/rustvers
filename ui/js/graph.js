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
    this.view = { x: 0, y: 0, k: 1 };
    this.options = { showMissing: true, showOrphans: true, labels: true };
    this.all = { nodes: [], edges: [] };
    this.positions = new Map(); // id → {x, y}: сохраняем раскладку между пересборками
    this.hover = null;
    this.selected = null;
    this.query = "";
    this.dpr = 1;
    this.frame = 0;
    this.visible = true;
    this.readColors();
    this.bindEvents();
    new ResizeObserver(() => this.resize()).observe(canvas);
    matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => { this.readColors(); this.redraw(); });
  }

  /** @param {{nodes: {id:string,title:string,kind:string}[], edges: [number,number][]}} graph */
  setData(graph) {
    this.all = graph;
    this.rebuild(true);
  }

  setOptions(opts) {
    const structural = opts.showMissing !== this.options.showMissing || opts.showOrphans !== this.options.showOrphans;
    Object.assign(this.options, opts);
    if (structural) this.rebuild(false); else this.redraw();
  }

  setSelected(id) {
    this.selected = id ? this.byId.get(id) || null : null;
    this.redraw();
  }

  setQuery(q) {
    this.query = q.trim().toLowerCase();
    this.redraw();
  }

  setVisible(v) {
    this.visible = v;
    if (v) { this.resize(); this.redraw(); }
  }

  fit(animate = true) {
    const nodes = this.sim.nodes;
    const w = this.canvas.clientWidth, h = this.canvas.clientHeight;
    if (!nodes.length || !w || !h) return;
    if (animate) this.autoFit = false;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of nodes) {
      x0 = Math.min(x0, n.x); x1 = Math.max(x1, n.x);
      y0 = Math.min(y0, n.y); y1 = Math.max(y1, n.y);
    }
    const pad = 60;
    const k = Math.min(2, (w - pad * 2) / Math.max(x1 - x0, 1), (h - pad * 2) / Math.max(y1 - y0, 1));
    const target = { k, x: w / 2 - ((x0 + x1) / 2) * k, y: h / 2 - ((y0 + y1) / 2) * k };
    if (!animate) { this.view = target; this.redraw(); return; }
    const from = { ...this.view }, t0 = performance.now();
    const step = (now) => {
      const t = Math.min(1, (now - t0) / 350), e = 1 - Math.pow(1 - t, 3);
      this.view = { k: from.k + (target.k - from.k) * e, x: from.x + (target.x - from.x) * e, y: from.y + (target.y - from.y) * e };
      this.draw();
      if (t < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
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
    this.hover = null;

    if (fresh) {
      // Предрасчёт, чтобы первая картинка была уже «разложенной».
      this.sim.alpha = 1;
      const budget = performance.now() + 120;
      while (this.sim.alpha > 0.25 && performance.now() < budget) this.sim.tick();
      this.autoFit = true;
      this.fit(false);
    } else {
      this.sim.reheat(0.4);
    }
    this.loop();
  }

  // -- цикл -------------------------------------------------------------------

  loop() {
    if (this.frame) return;
    const step = () => {
      this.frame = 0;
      if (this.sim.running) this.sim.tick();
      // Пока пользователь не трогал вид, камера следит за раскладкой.
      if (this.autoFit) this.fit(false); else this.draw();
      if (this.sim.running && this.visible) this.frame = requestAnimationFrame(step);
    };
    this.frame = requestAnimationFrame(step);
  }

  redraw() {
    if (this.frame || this.pending) return;
    this.pending = requestAnimationFrame(() => { this.pending = 0; this.draw(); });
  }

  resize() {
    const c = this.canvas;
    this.dpr = window.devicePixelRatio || 1;
    const w = Math.round(c.clientWidth * this.dpr), h = Math.round(c.clientHeight * this.dpr);
    if (c.width !== w || c.height !== h) {
      c.width = w; c.height = h;
      if (this.autoFit) this.fit(false); else this.draw();
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
    const { ctx, view, colors, dpr } = this;
    const w = this.canvas.width, h = this.canvas.height;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, w, h);
    ctx.setTransform(dpr * view.k, 0, 0, dpr * view.k, dpr * view.x, dpr * view.y);

    const focus = this.hover || null;
    const q = this.query;
    const matches = (n) => !q || n.title.toLowerCase().includes(q) || n.id.toLowerCase().includes(q);
    const lit = (n) => !focus || n === focus || focus.neighbors.has(n);

    // Рёбра: сначала обычные, затем подсвеченные поверх.
    ctx.lineWidth = Math.max(0.6, 1 / view.k);
    ctx.strokeStyle = colors.edge;
    ctx.globalAlpha = focus || q ? 0.35 : 1;
    ctx.beginPath();
    for (const l of this.sim.links) {
      if (focus && (l.source === focus || l.target === focus)) continue;
      ctx.moveTo(l.source.x, l.source.y);
      ctx.lineTo(l.target.x, l.target.y);
    }
    ctx.stroke();
    if (focus) {
      ctx.globalAlpha = 1;
      ctx.strokeStyle = colors.edgeHi;
      ctx.lineWidth = Math.max(1.2, 1.6 / view.k);
      ctx.beginPath();
      for (const l of this.sim.links) {
        if (l.source !== focus && l.target !== focus) continue;
        ctx.moveTo(l.source.x, l.source.y);
        ctx.lineTo(l.target.x, l.target.y);
      }
      ctx.stroke();
    }

    // Узлы.
    for (const n of this.sim.nodes) {
      ctx.globalAlpha = lit(n) && matches(n) ? 1 : 0.18;
      ctx.fillStyle = colors[n.kind] || colors.markdown;
      ctx.beginPath();
      ctx.arc(n.x, n.y, n.r, 0, Math.PI * 2);
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
    const zoomFade = Math.min(1, Math.max(0, (view.k - 0.55) / 0.35));
    const cw = this.canvas.clientWidth, ch = this.canvas.clientHeight;
    for (const n of this.sim.nodes) {
      const important = n === focus || n === this.selected || (focus && focus.neighbors.has(n)) || (q && matches(n));
      let a = important ? 1 : this.options.labels ? zoomFade : 0;
      if (focus && !lit(n)) a *= 0.15;
      if (q && !matches(n)) a *= 0.15;
      if (a < 0.03) continue;
      const sx = n.x * view.k + view.x, sy = (n.y + n.r) * view.k + view.y + 4;
      if (sx < -100 || sx > cw + 100 || sy < -20 || sy > ch + 20) continue;
      ctx.globalAlpha = a;
      ctx.fillStyle = colors.label;
      ctx.lineWidth = 3;
      ctx.strokeStyle = colors.bg;
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

  bindEvents() {
    const c = this.canvas;
    let drag = null; // {node?, startX, startY, viewX, viewY, moved}

    c.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      c.setPointerCapture(e.pointerId);
      const node = this.nodeAt(this.toWorld(e));
      this.autoFit = false;
      drag = { node, sx: e.clientX, sy: e.clientY, vx: this.view.x, vy: this.view.y, moved: false };
      if (node) { node.fx = node.x; node.fy = node.y; }
      else c.classList.add("panning");
    });

    c.addEventListener("pointermove", (e) => {
      if (!drag) {
        const n = this.nodeAt(this.toWorld(e));
        if (n !== this.hover) {
          this.hover = n;
          c.classList.toggle("over-node", !!n);
          c.title = n ? n.id.replace(/^\?/, "") : "";
          this.redraw();
        }
        return;
      }
      const dx = e.clientX - drag.sx, dy = e.clientY - drag.sy;
      if (!drag.moved && dx * dx + dy * dy < 9) return;
      drag.moved = true;
      if (drag.node) {
        const p = this.toWorld(e);
        drag.node.fx = p.x; drag.node.fy = p.y;
        this.sim.alphaTarget = 0.25;
        this.sim.reheat(0.25);
        this.loop();
      } else {
        this.view.x = drag.vx + dx;
        this.view.y = drag.vy + dy;
        this.redraw();
      }
    });

    const end = () => {
      if (!drag) return;
      if (drag.node) {
        drag.node.fx = drag.node.fy = null;
        this.sim.alphaTarget = 0;
        if (!drag.moved) this.onOpen(drag.node.id);
      }
      c.classList.remove("panning");
      drag = null;
    };
    c.addEventListener("pointerup", end);
    c.addEventListener("pointercancel", end);
    c.addEventListener("pointerleave", () => { if (!drag && this.hover) { this.hover = null; this.redraw(); } });

    c.addEventListener("wheel", (e) => {
      e.preventDefault();
      this.autoFit = false;
      const r = c.getBoundingClientRect();
      const mx = e.clientX - r.left, my = e.clientY - r.top;
      const factor = Math.exp(-e.deltaY * (e.deltaMode === 1 ? 0.05 : 0.0015));
      const k = Math.min(8, Math.max(0.05, this.view.k * factor));
      this.view.x = mx - ((mx - this.view.x) * k) / this.view.k;
      this.view.y = my - ((my - this.view.y) * k) / this.view.k;
      this.view.k = k;
      this.redraw();
    }, { passive: false });
  }
}
