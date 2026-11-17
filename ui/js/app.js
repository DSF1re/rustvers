// Оболочка приложения: состояние, боковая панель, навигация, просмотр заметок.
import { GraphView } from "./graph.js";
import { renderTree } from "./tree.js";

const tauri = window.__TAURI__;
const invoke = tauri.core.invoke;
const $ = (id) => document.getElementById(id);

const state = {
  vault: null,         // { name, root, base_url, nodes, edges }
  byId: new Map(),     // id → node
  out: new Map(),      // id → Set(id) исходящие
  back: new Map(),     // id → Set(id) входящие
  current: null,       // открытая заметка
  view: "graph",
  history: [],
  pos: -1,
  reload: 0,           // для сброса кэша iframe после пересканирования
};

const graph = new GraphView($("graph"), { onOpen: (id) => openNote(id) });

// ---------------------------------------------------------------------------
// Хранилище
// ---------------------------------------------------------------------------

function loadVault(vault, { keepNote = false } = {}) {
  state.vault = vault;
  state.byId = new Map(vault.nodes.map((n) => [n.id, n]));
  state.out = new Map(vault.nodes.map((n) => [n.id, new Set()]));
  state.back = new Map(vault.nodes.map((n) => [n.id, new Set()]));
  for (const [s, t] of vault.edges) {
    const a = vault.nodes[s].id, b = vault.nodes[t].id;
    state.out.get(a).add(b);
    state.back.get(b).add(a);
  }
  state.reload++;

  $("welcome").hidden = true;
  $("app").hidden = false;
  $("vault-name").textContent = vault.name;
  $("vault-name").title = vault.root;
  const notes = vault.nodes.filter((n) => n.kind !== "missing").length;
  $("stats").textContent = `${notes} ${plural(notes, "заметка", "заметки", "заметок")} · ${vault.edges.length} ${plural(vault.edges.length, "связь", "связи", "связей")}`;

  if (keepNote && state.current && state.byId.has(state.current)) {
    graph.setData(vault);
    showNote(state.current, "");
  } else {
    state.current = null;
    state.history = [];
    state.pos = -1;
    graph.setData(vault);
    setView("graph");
  }
  refreshTree();
  updateChrome();
}

async function pickVault() {
  try {
    const dir = await tauri.dialog.open({ directory: true, title: "Папка с заметками" });
    if (!dir) return;
    loadVault(await invoke("open_vault", { path: dir }));
  } catch (e) {
    fail(e);
  }
}

async function rescan() {
  if (!state.vault) return;
  try {
    loadVault(await invoke("rescan"), { keepNote: true });
    toast("Хранилище обновлено");
  } catch (e) {
    fail(e);
  }
}

// ---------------------------------------------------------------------------
// Навигация
// ---------------------------------------------------------------------------

function openNote(id, hash = "", { push = true } = {}) {
  const node = state.byId.get(id);
  if (!node) return;
  if (node.kind === "missing") {
    toast(`Заметки «${node.title}» ещё нет`);
    return;
  }
  if (push) {
    state.history = state.history.slice(0, state.pos + 1);
    if (state.history[state.history.length - 1] !== id) state.history.push(id);
    state.pos = state.history.length - 1;
  }
  showNote(id, hash);
}

function showNote(id, hash) {
  state.current = id;
  const frame = $("note-frame");
  const url = state.vault.base_url + encodePath(id) + `?v=${state.reload}` + (hash ? "#" + encodeURIComponent(hash) : "");
  if (frame.dataset.src !== url) {
    frame.dataset.src = url;
    frame.src = url;
  }
  renderLinks(id);
  graph.setSelected(id);
  setView("note");
  refreshTree();
  updateChrome();
}

function go(delta) {
  const pos = state.pos + delta;
  if (pos < 0 || pos >= state.history.length) return;
  state.pos = pos;
  openNote(state.history[pos], "", { push: false });
}

function setView(view) {
  if (view === "note" && !state.current) view = "graph";
  state.view = view;
  $("graph-view").hidden = view !== "graph";
  $("note-view").hidden = view !== "note";
  graph.setVisible(view === "graph");
  updateChrome();
}

function toggleView() {
  setView(state.view === "graph" ? "note" : "graph");
}

function updateChrome() {
  $("btn-back").disabled = state.pos <= 0;
  $("btn-forward").disabled = state.pos >= state.history.length - 1;
  $("tab-graph").setAttribute("aria-selected", state.view === "graph");
  $("tab-note").setAttribute("aria-selected", state.view === "note");
  $("tab-note").disabled = !state.current;

  const crumbs = $("crumbs");
  crumbs.replaceChildren();
  if (state.view === "note" && state.current) {
    const parts = state.current.split("/");
    const name = parts.pop();
    if (parts.length) crumbs.append(parts.join(" / ") + " / ");
    const b = document.createElement("b");
    b.textContent = name;
    crumbs.append(b);
    document.title = `${state.byId.get(state.current)?.title} — Rustvers`;
  } else if (state.vault) {
    crumbs.textContent = "Граф связей";
    document.title = `${state.vault.name} — Rustvers`;
  }
}

// ---------------------------------------------------------------------------
// Панели
// ---------------------------------------------------------------------------

function refreshTree() {
  renderTree($("tree"), state.vault.nodes, {
    query: $("search").value,
    active: state.current,
    onOpen: (id) => openNote(id),
  });
}

function renderLinks(id) {
  const fill = (list, countEl, ids) => {
    const items = [...ids].map((i) => state.byId.get(i)).sort((a, b) => a.title.localeCompare(b.title));
    countEl.textContent = items.length || "";
    list.replaceChildren();
    if (!items.length) {
      const li = document.createElement("li");
      li.className = "none";
      li.textContent = "Нет";
      list.append(li);
    }
    for (const n of items) {
      const li = document.createElement("li");
      li.textContent = n.title;
      li.title = n.id.replace(/^\?/, "");
      if (n.kind === "missing") li.className = "missing";
      else li.onclick = () => openNote(n.id);
      list.append(li);
    }
  };
  fill($("backlinks"), $("backlinks-count"), state.back.get(id) || []);
  fill($("outlinks"), $("outlinks-count"), state.out.get(id) || []);
}

// ---------------------------------------------------------------------------
// Сообщения из заметки (iframe)
// ---------------------------------------------------------------------------

window.addEventListener("message", (e) => {
  if (e.source !== $("note-frame").contentWindow || !e.data || e.data.rustvers !== 1) return;
  const msg = e.data;
  if (msg.type === "open" && typeof msg.path === "string") {
    if (state.byId.has(msg.path)) openNote(msg.path, msg.hash || "");
    else toast(`Не заметка: ${msg.path}`);
  } else if (msg.type === "external" && /^(https?|mailto):/i.test(msg.url)) {
    tauri.opener.openUrl(msg.url).catch(fail);
  } else if (msg.type === "key") {
    handleKey(msg);
  }
});

// ---------------------------------------------------------------------------
// Клавиатура и элементы управления
// ---------------------------------------------------------------------------

function handleKey({ key, ctrl, alt, shift }) {
  const k = key.length === 1 ? key.toLowerCase() : key;
  if (ctrl && (k === "k" || k === "p" || k === "л" || k === "з")) { $("search").focus(); $("search").select(); return true; }
  if (ctrl && (k === "g" || k === "п")) { toggleView(); return true; }
  if (ctrl && (k === "o" || k === "щ")) { pickVault(); return true; }
  if ((ctrl && (k === "r" || k === "к")) || k === "F5") { rescan(); return true; }
  if (alt && k === "ArrowLeft") { go(-1); return true; }
  if (alt && k === "ArrowRight") { go(1); return true; }
  if (k === "Escape" && document.activeElement === $("search")) { $("search").value = ""; onSearch(); $("search").blur(); return true; }
  return false;
}

document.addEventListener("keydown", (e) => {
  if (!state.vault) return;
  if (handleKey({ key: e.key, ctrl: e.ctrlKey || e.metaKey, alt: e.altKey, shift: e.shiftKey })) e.preventDefault();
});
document.addEventListener("mouseup", (e) => {
  if (e.button === 3) go(-1);
  if (e.button === 4) go(1);
});

function onSearch() {
  refreshTree();
  graph.setQuery($("search").value);
}

$("search").addEventListener("input", onSearch);
$("search").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    const first = $("tree").querySelector(".row[data-id]");
    if (first) openNote(first.dataset.id);
  }
});
$("btn-open").onclick = pickVault;
$("btn-welcome-open").onclick = pickVault;
$("btn-rescan").onclick = rescan;
$("btn-back").onclick = () => go(-1);
$("btn-forward").onclick = () => go(1);
$("tab-graph").onclick = () => setView("graph");
$("tab-note").onclick = () => setView("note");
$("btn-fit").onclick = () => graph.fit();
$("opt-missing").onchange = (e) => graph.setOptions({ showMissing: e.target.checked });
$("opt-orphans").onchange = (e) => graph.setOptions({ showOrphans: e.target.checked });
$("opt-labels").onchange = (e) => graph.setOptions({ labels: e.target.checked });

// Изменение ширины боковой панели.
(() => {
  const resizer = $("resizer");
  let startX = 0, startW = 0;
  const move = (e) => document.documentElement.style.setProperty("--sidebar-w", `${Math.max(180, startW + e.clientX - startX)}px`);
  resizer.addEventListener("pointerdown", (e) => {
    startX = e.clientX;
    startW = $("sidebar").getBoundingClientRect().width;
    resizer.setPointerCapture(e.pointerId);
    resizer.classList.add("drag");
    resizer.addEventListener("pointermove", move);
  });
  resizer.addEventListener("pointerup", () => {
    resizer.classList.remove("drag");
    resizer.removeEventListener("pointermove", move);
  });
})();

// ---------------------------------------------------------------------------
// Утилиты
// ---------------------------------------------------------------------------

function encodePath(p) {
  return p.split("/").map(encodeURIComponent).join("/");
}

function plural(n, one, few, many) {
  const m10 = n % 10, m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 10 || m100 >= 20)) return few;
  return many;
}

let toastTimer = 0;
function toast(text) {
  const el = $("toast");
  el.textContent = text;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.hidden = true), 2600);
}

function fail(e) {
  console.error(e);
  const msg = String(e?.message || e);
  if (state.vault) toast(msg);
  else $("welcome-error").textContent = msg;
}

// ---------------------------------------------------------------------------
// Старт
// ---------------------------------------------------------------------------

(async () => {
  try {
    const vault = await invoke("current_vault");
    if (vault) return loadVault(vault);
  } catch (e) {
    fail(e);
  }
  $("welcome").hidden = false;
})();
