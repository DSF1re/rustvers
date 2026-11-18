// Нативный граф (Wayland): рисует Rust в подповерхности поверх #graph
// (src/app/native). Здесь — только передача состояния и положения области.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

export class NativeGraphView {
  /**
   * @param {HTMLElement} el — область, поверх которой рисуется граф
   * @param {{ onOpen?: (id: string) => void }} opts
   */
  constructor(el, { onOpen } = {}) {
    this.el = el;
    this.visible = true;
    this.fps = false;
    this.options = { showMissing: true, showOrphans: true, labels: true };
    this.lastRect = undefined;
    listen("graph-open", (e) => onOpen && onOpen(e.payload));
    // Положение и размер области меняются при ресайзе окна и боковой панели.
    new ResizeObserver(() => this.sync()).observe(el);
    window.addEventListener("resize", () => this.sync());
    matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => this.sendTheme());
  }

  /** Создать подповерхность и передать ей текущее состояние. */
  async start() {
    await invoke("graph_native_init");
    this.sendTheme();
    this.lastRect = undefined;
    this.sync();
  }

  send(msg) {
    invoke("graph_native", { msg }).catch((e) => console.error("graph_native", e));
  }

  /** `fresh: false` — обновление после правки: без перестройки вида. */
  setData(_vault, { fresh = true } = {}) {
    this.send({ type: "data", fresh });
    this.sync();
  }

  setOptions(opts) {
    Object.assign(this.options, opts);
    const o = this.options;
    this.send({ type: "options", show_missing: o.showMissing, show_orphans: o.showOrphans, labels: o.labels });
  }

  setSelected(id) {
    this.send({ type: "selected", id: id || null });
  }

  setQuery(text) {
    this.send({ type: "query", text });
  }

  setVisible(v) {
    this.visible = v;
    this.sync();
  }

  fit() {
    this.send({ type: "fit" });
  }

  toggleFps() {
    this.fps = !this.fps;
    this.send({ type: "fps", on: this.fps });
  }

  /** Сообщить Rust, где сейчас область графа (или что она скрыта). */
  sync() {
    const r = this.el.getBoundingClientRect();
    const rect = this.visible && r.width > 0 && r.height > 0
      ? { x: r.left, y: r.top, w: r.width, h: r.height }
      : null;
    const key = JSON.stringify(rect);
    if (key === this.lastRect) return;
    this.lastRect = key;
    this.send({ type: "layout", rect });
  }

  sendTheme() {
    const css = getComputedStyle(document.documentElement);
    const v = (name) => css.getPropertyValue(name).trim();
    this.send({
      type: "theme",
      colors: {
        markdown: v("--node-md"), html: v("--node-html"), missing: v("--node-missing"),
        edge: v("--edge"), edgeHi: v("--edge-hi"), label: v("--label"), bg: v("--bg"), accent: v("--accent"),
      },
    });
  }
}
