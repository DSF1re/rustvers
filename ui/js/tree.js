// Дерево файлов в боковой панели (папки сворачиваются, состояние запоминается).

const openFolders = new Set();
let initialized = false;

const chevron = () => {
  const s = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  s.setAttribute("viewBox", "0 0 24 24");
  s.classList.add("chev");
  s.innerHTML = '<path d="M9 6l6 6-6 6"/>';
  return s;
};

function noteRow(node, active, onOpen, hint) {
  const row = document.createElement("div");
  row.className = "row" + (node.id === active ? " active" : "");
  row.dataset.id = node.id;
  row.title = node.id;
  const name = document.createElement("span");
  name.className = "name";
  name.textContent = node.title;
  row.append(name);
  if (node.kind === "html") {
    const b = document.createElement("span");
    b.className = "badge";
    b.textContent = "HTML";
    row.append(b);
  }
  if (hint) {
    const h = document.createElement("span");
    h.className = "hint";
    h.textContent = hint;
    row.append(h);
  }
  row.onclick = () => onOpen(node.id);
  return row;
}

/**
 * @param {HTMLElement} root
 * @param {{id:string,title:string,kind:string}[]} nodes
 * @param {{query:string, active:string|null, onOpen:(id:string)=>void}} opts
 */
export function renderTree(root, nodes, { query, active, onOpen }) {
  const notes = nodes.filter((n) => n.kind !== "missing");
  const q = query.trim().toLowerCase();
  root.replaceChildren();

  if (!initialized) {
    // По умолчанию раскрыты папки верхнего уровня.
    for (const n of notes) if (n.id.includes("/")) openFolders.add(n.id.split("/")[0]);
    initialized = true;
  }

  if (q) {
    const hits = notes
      .filter((n) => n.title.toLowerCase().includes(q) || n.id.toLowerCase().includes(q))
      .sort((a, b) => score(a, q) - score(b, q) || a.title.localeCompare(b.title));
    for (const n of hits) {
      const dir = n.id.includes("/") ? n.id.slice(0, n.id.lastIndexOf("/")) : "";
      root.append(noteRow(n, active, onOpen, dir));
    }
    if (!hits.length) root.append(empty("Ничего не найдено"));
    return;
  }
  if (!notes.length) {
    root.append(empty("В папке нет .md или .html файлов"));
    return;
  }

  // Строим дерево папок.
  const tree = { dirs: new Map(), files: [] };
  for (const n of notes) {
    const parts = n.id.split("/");
    parts.pop();
    let cur = tree;
    for (const p of parts) {
      if (!cur.dirs.has(p)) cur.dirs.set(p, { dirs: new Map(), files: [] });
      cur = cur.dirs.get(p);
    }
    cur.files.push(n);
  }
  // Раскрываем путь к активной заметке.
  if (active) {
    const parts = active.split("/");
    for (let i = 1; i < parts.length; i++) openFolders.add(parts.slice(0, i).join("/"));
  }

  const build = (dir, prefix, container) => {
    const dirs = [...dir.dirs.keys()].sort((a, b) => a.localeCompare(b));
    for (const name of dirs) {
      const path = prefix ? `${prefix}/${name}` : name;
      const det = document.createElement("details");
      det.className = "folder";
      det.open = openFolders.has(path);
      det.ontoggle = () => (det.open ? openFolders.add(path) : openFolders.delete(path));
      const sum = document.createElement("summary");
      sum.className = "row";
      const label = document.createElement("span");
      label.className = "name";
      label.textContent = name;
      sum.append(chevron(), label);
      const kids = document.createElement("div");
      kids.className = "children";
      build(dir.dirs.get(name), path, kids);
      det.append(sum, kids);
      container.append(det);
    }
    for (const n of dir.files.sort((a, b) => a.title.localeCompare(b.title))) {
      container.append(noteRow(n, active, onOpen));
    }
  };
  build(tree, "", root);

  root.querySelector(".row.active")?.scrollIntoView({ block: "nearest" });
}

function score(n, q) {
  const t = n.title.toLowerCase();
  return t === q ? 0 : t.startsWith(q) ? 1 : t.includes(q) ? 2 : 3;
}

function empty(text) {
  const d = document.createElement("div");
  d.className = "empty";
  d.textContent = text;
  return d;
}
