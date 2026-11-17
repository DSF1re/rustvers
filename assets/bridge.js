// Мост между заметкой (sandbox iframe) и приложением.
// Перехватывает клики по ссылкам и пересылает горячие клавиши.
(() => {
  const here = new URL(location.href);
  const send = (msg) => parent.postMessage({ rustvers: 1, ...msg }, "*");
  const notePath = (url) => decodeURIComponent(url.pathname.replace(/^\/+/, ""));

  document.addEventListener("click", (e) => {
    if (e.defaultPrevented || e.button !== 0) return;
    const a = e.target instanceof Element ? e.target.closest("a") : null;
    if (!a) return;
    const href = a.getAttribute("href");
    if (!href) {
      if (a.classList.contains("unresolved")) e.preventDefault();
      return;
    }
    if (href.startsWith("#")) return; // якорь внутри заметки

    let url;
    try { url = new URL(href, location.href); } catch { return; }
    e.preventDefault();

    if (url.protocol === here.protocol && url.host === here.host) {
      const path = notePath(url);
      if (path === notePath(here) && url.hash) {
        location.hash = url.hash;
      } else {
        send({ type: "open", path, hash: decodeURIComponent(url.hash.slice(1)) });
      }
    } else if (/^(https?|mailto):$/.test(url.protocol)) {
      send({ type: "external", url: url.href });
    }
  }, false);

  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey || e.key === "Escape") {
      send({ type: "key", key: e.key, ctrl: e.ctrlKey || e.metaKey, alt: e.altKey, shift: e.shiftKey });
    }
  });

  // Боковые кнопки мыши — назад/вперёд по истории приложения.
  document.addEventListener("mouseup", (e) => {
    if (e.button === 3) send({ type: "key", key: "ArrowLeft", alt: true });
    if (e.button === 4) send({ type: "key", key: "ArrowRight", alt: true });
  });
})();
