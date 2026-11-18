// Редактор исходника заметки (Markdown или HTML): автосохранение,
// продолжение списков, подсказки при вводе [[ссылки]].

const { invoke } = window.__TAURI__.core;

const SAVE_DELAY = 700; // мс после последнего нажатия
const MAX_SUGGESTIONS = 8;

export class Editor {
  /**
   * @param {{
   *   textarea: HTMLTextAreaElement,
   *   suggest: HTMLElement,
   *   status: HTMLElement,
   *   notes: () => {id: string, title: string, kind: string}[],
   *   onSaved: (vault: object) => void,
   *   onError: (message: string) => void,
   * }} opts
   */
  constructor(opts) {
    Object.assign(this, opts);
    this.path = null;
    this.mtime = null;
    this.dirty = false;
    this.saving = null; // Promise текущего сохранения
    this.timer = 0;
    this.items = [];    // текущие подсказки
    this.active = 0;

    const ta = this.textarea;
    ta.addEventListener("input", () => this.onInput());
    ta.addEventListener("keydown", (e) => this.onKeyDown(e));
    ta.addEventListener("blur", () => { this.hideSuggest(); this.flush(); });
    ta.addEventListener("scroll", () => this.hideSuggest());
    this.suggest.addEventListener("mousedown", (e) => {
      const li = e.target.closest("li[data-index]");
      if (!li) return;
      e.preventDefault(); // не уводить фокус из textarea
      this.accept(Number(li.dataset.index));
    });
  }

  get isMarkdown() {
    return /\.(md|markdown)$/i.test(this.path || "");
  }

  /** Загрузить заметку в редактор (предыдущая сохраняется). */
  async open(path, { discard = false } = {}) {
    await this.flush();
    if (this.dirty && !discard) throw new Error("Правки не сохранены — сначала разберитесь с ошибкой сохранения");
    const { content, mtime } = await invoke("note_source", { path });
    this.path = path;
    this.mtime = mtime;
    this.dirty = false;
    this.textarea.value = content;
    this.textarea.setSelectionRange(0, 0);
    this.textarea.scrollTop = 0;
    this.setStatus("Сохранено", "saved");
  }

  focus() {
    this.textarea.focus();
  }

  /** Сохранить немедленно, если есть изменения. */
  async flush() {
    clearTimeout(this.timer);
    if (this.saving) await this.saving.catch(() => {});
    if (!this.dirty || !this.path) return;
    const path = this.path, content = this.textarea.value;
    this.dirty = false;
    this.setStatus("Сохранение…", "saving");
    this.saving = invoke("save_note", { path, content, mtime: this.mtime });
    try {
      const { mtime, vault } = await this.saving;
      if (path === this.path) this.mtime = mtime;
      if (!this.dirty) this.setStatus("Сохранено", "saved");
      this.onSaved(vault);
    } catch (e) {
      if (path === this.path) this.dirty = true; // не терять правки
      this.setStatus("Не сохранено", "error");
      this.onError(String(e));
    } finally {
      this.saving = null;
    }
  }

  setStatus(text, kind) {
    this.status.textContent = text;
    this.status.dataset.kind = kind;
  }

  onInput() {
    this.dirty = true;
    this.setStatus("Изменено", "dirty");
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.flush(), SAVE_DELAY);
    this.updateSuggest();
  }

  // -- клавиши ---------------------------------------------------------------

  onKeyDown(e) {
    if (!this.suggest.hidden) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const n = this.items.length;
        this.active = (this.active + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
        this.renderSuggest();
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        this.accept(this.active);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.hideSuggest();
        return;
      }
    }
    if ((e.ctrlKey || e.metaKey) && (e.key === "s" || e.key === "ы")) {
      e.preventDefault();
      this.flush();
      return;
    }
    if (e.key === "Tab" && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      if (!e.shiftKey) insert("\t");
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !e.ctrlKey && !e.altKey && this.isMarkdown && this.continueList()) {
      e.preventDefault();
    }
  }

  /** Enter в пункте списка продолжает список; в пустом пункте — завершает. */
  continueList() {
    const ta = this.textarea;
    if (ta.selectionStart !== ta.selectionEnd) return false;
    const pos = ta.selectionStart;
    const lineStart = ta.value.lastIndexOf("\n", pos - 1) + 1;
    const line = ta.value.slice(lineStart, pos);
    const m = line.match(/^(\s*)([-*+]|(\d+)([.)]))(\s+\[[ xX]\])?(\s+)(.*)$/);
    if (!m) return false;
    const [, indent, bullet, num, delim, task, , rest] = m;
    if (!rest.trim()) {
      // Пустой пункт: убираем маркер и выходим из списка.
      ta.setSelectionRange(lineStart, pos);
      insert(indent);
      return true;
    }
    const marker = num ? `${Number(num) + 1}${delim}` : bullet;
    insert(`\n${indent}${marker}${task ? " [ ]" : ""} `);
    return true;
  }

  // -- подсказки [[ ------------------------------------------------------------

  /** Текст, набранный после «[[» до курсора, или null. */
  linkQuery() {
    const ta = this.textarea;
    if (ta.selectionStart !== ta.selectionEnd) return null;
    const before = ta.value.slice(Math.max(0, ta.selectionStart - 200), ta.selectionStart);
    const m = before.match(/\[\[([^\[\]|#\n]*)$/);
    return m ? m[1] : null;
  }

  updateSuggest() {
    const q = this.linkQuery();
    if (q === null) return this.hideSuggest();
    const query = q.toLowerCase();
    const notes = this.notes().filter((n) => n.kind !== "missing");
    // Имя без расширения уникально? Тогда ссылка по имени, иначе по пути.
    const counts = new Map();
    for (const n of notes) counts.set(n.title.toLowerCase(), (counts.get(n.title.toLowerCase()) || 0) + 1);
    this.items = notes
      .filter((n) => n.title.toLowerCase().includes(query) || n.id.toLowerCase().includes(query))
      .map((n) => ({
        title: n.title,
        hint: n.id.includes("/") ? n.id.slice(0, n.id.lastIndexOf("/")) : "",
        link: counts.get(n.title.toLowerCase()) > 1 ? n.id.replace(/\.(md|markdown|html?)$/i, "") : n.title,
        score: n.title.toLowerCase().startsWith(query) ? 0 : 1,
      }))
      .sort((a, b) => a.score - b.score || a.title.localeCompare(b.title))
      .slice(0, MAX_SUGGESTIONS);
    if (!this.items.length) return this.hideSuggest();
    this.active = 0;
    this.renderSuggest();
    this.placeSuggest();
  }

  renderSuggest() {
    this.suggest.replaceChildren(
      ...this.items.map((item, i) => {
        const li = document.createElement("li");
        li.dataset.index = i;
        if (i === this.active) li.className = "active";
        const name = document.createElement("span");
        name.textContent = item.title;
        li.append(name);
        if (item.hint) {
          const hint = document.createElement("small");
          hint.textContent = item.hint;
          li.append(hint);
        }
        return li;
      }),
    );
    this.suggest.hidden = false;
    this.suggest.querySelector(".active")?.scrollIntoView({ block: "nearest" });
  }

  /** Поставить список под курсором (через «зеркало» textarea). */
  placeSuggest() {
    const ta = this.textarea;
    const { top, left, height } = caretPosition(ta);
    const wrap = this.suggest.offsetParent?.getBoundingClientRect() || { left: 0, top: 0 };
    const box = ta.getBoundingClientRect();
    const x = Math.min(box.left + left - wrap.left, box.right - wrap.left - 280);
    this.suggest.style.left = `${Math.max(8, x)}px`;
    this.suggest.style.top = `${box.top + top + height + 4 - wrap.top}px`;
  }

  hideSuggest() {
    this.suggest.hidden = true;
    this.items = [];
  }

  accept(i) {
    const item = this.items[i];
    const q = this.linkQuery();
    if (!item || q === null) return this.hideSuggest();
    const ta = this.textarea;
    const pos = ta.selectionStart;
    const closed = ta.value.slice(pos, pos + 2) === "]]";
    ta.setSelectionRange(pos - q.length, pos);
    insert(item.link + (closed ? "" : "]]"));
    if (closed) ta.setSelectionRange(pos - q.length + item.link.length + 2, pos - q.length + item.link.length + 2);
    this.hideSuggest();
  }
}

/** Вставка с сохранением истории отмены (Ctrl+Z). */
function insert(text) {
  document.execCommand("insertText", false, text);
}

/** Координаты курсора внутри textarea (относительно её рамки). */
function caretPosition(ta) {
  const div = document.createElement("div");
  const cs = getComputedStyle(ta);
  for (const p of ["boxSizing", "width", "padding", "border", "font", "lineHeight", "letterSpacing", "tabSize", "whiteSpace", "wordWrap", "overflowWrap"]) {
    div.style[p] = cs[p];
  }
  Object.assign(div.style, { position: "absolute", visibility: "hidden", top: "0", left: "-9999px", whiteSpace: "pre-wrap" });
  div.textContent = ta.value.slice(0, ta.selectionStart);
  const mark = document.createElement("span");
  mark.textContent = "​";
  div.append(mark);
  document.body.append(div);
  const pos = { top: mark.offsetTop - ta.scrollTop, left: mark.offsetLeft - ta.scrollLeft, height: mark.offsetHeight };
  div.remove();
  return pos;
}
