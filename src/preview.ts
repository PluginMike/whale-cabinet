// Previews: list/grid thumbnails (freedesktop cache), info-panel preview (F11), Quick Look overlay (Space),
// Markdown rendering (sanitized), syntax-highlighted text, PDF first page, audio/video players, folder stats.
import { marked } from "marked";
import DOMPurify from "dompurify";
import hljs from "highlight.js/lib/common";
import { $, Entry, invoke, esc, shown, ext, kindOf, parentOf, fmtSize, fmtDate, typeName, assetUrl } from "./util";
import { hooks, pane, allPanes, dl, glyph, flash, host } from "./app";

const THUMB_KINDS = new Set(["img", "vid"]);
const canThumb = (e: Entry) => !e.dir && !e.special && !e.broken && !e.trashId && (THUMB_KINDS.has(kindOf(e)) || ext(e) === "pdf") && ext(e) !== "svg";

// ---------- thumbnails (lazy, bounded concurrency, newest request first) ----------
const thumbs = new Map<string, string | null>(); // key → asset url, or null = none/failed
const queue: { key: string; path: string; size: number }[] = [];
let running = 0, rerender = 0;
const key = (e: Entry, size: number) => `${e.path}\0${e.mtime}\0${size}`;
function pump() {
  while (running < 4 && queue.length) {
    const job = queue.pop()!; // LIFO: what's on screen now beats what scrolled past
    running++;
    invoke<string>("thumbnail", { path: job.path, size: job.size })
      .then((p) => thumbs.set(job.key, assetUrl(p)))
      .catch(() => thumbs.set(job.key, null))
      .finally(() => {
        running--;
        pump();
        if (!rerender) rerender = requestAnimationFrame(() => { rerender = 0; allPanes().forEach((p) => p.render()); });
      });
  }
}
hooks.thumb = (e) => {
  if (ext(e) === "svg" && !e.trashId) return assetUrl(e.path);
  if (!canThumb(e)) return;
  const size = pane().zoom > 1.3 ? 256 : 128;
  const k = key(e, size);
  if (thumbs.has(k)) return thumbs.get(k) ?? undefined;
  if (!queue.some((q) => q.key === k)) {
    queue.push({ key: k, path: e.path, size });
    if (queue.length > 300) queue.shift(); // NOTE: drop the oldest when flinging through huge folders
    pump();
  }
};

// ---------- folder item counts ("12 items"), lazily like thumbnails ----------
const counts = new Map<string, string | null>();
const countQueue: { key: string; path: string }[] = [];
let countRunning = 0;
function pumpCounts() {
  while (countRunning < 6 && countQueue.length) {
    const job = countQueue.pop()!;
    countRunning++;
    invoke<number>("dir_count", { path: job.path, hidden: host.showHidden })
      .then((n) => counts.set(job.key, n === 1 ? "1 item" : `${n.toLocaleString()} items`))
      .catch(() => counts.set(job.key, null))
      .finally(() => {
        countRunning--;
        pumpCounts();
        if (!rerender) rerender = requestAnimationFrame(() => { rerender = 0; allPanes().forEach((p) => p.render()); });
      });
  }
}
hooks.count = (e) => {
  const k = `${e.path}\0${e.mtime}\0${host.showHidden}`;
  if (counts.has(k)) return counts.get(k) ?? undefined;
  if (!countQueue.some((q) => q.key === k)) {
    countQueue.push({ key: k, path: e.path });
    if (countQueue.length > 300) countQueue.shift();
    pumpCounts();
  }
};

// ---------- rendering helpers ----------
const LANG: Record<string, string> = { rs: "rust", ts: "typescript", js: "javascript", mjs: "javascript", py: "python", sh: "bash", zsh: "bash", fish: "bash", yml: "yaml", md: "markdown", h: "c", hpp: "cpp", cs: "csharp", kt: "kotlin", rb: "ruby", toml: "ini", conf: "ini", desktop: "ini", qml: "javascript", lua: "lua", nix: "nix" };
export function highlight(text: string, extension: string) {
  const lang = LANG[extension] ?? extension;
  const pre = document.createElement("pre");
  const code = document.createElement("code");
  if (text.length > 150_000) code.textContent = text; // too big to colour quickly
  else if (hljs.getLanguage(lang)) code.innerHTML = hljs.highlight(text, { language: lang }).value;
  else if (text.length < 20_000) code.innerHTML = hljs.highlightAuto(text).value;
  else code.textContent = text;
  pre.append(code);
  return pre;
}

/** Resolve a link/src from a Markdown file relative to its folder. */
function resolveRel(base: string, rel: string) {
  let p = decodeURIComponent(rel.split("#")[0].split("?")[0]);
  if (p.startsWith("file://")) p = p.slice(7);
  const parts = (p.startsWith("/") ? p : `${base}/${p}`).split("/");
  const out: string[] = [];
  for (const s of parts) { if (s === "..") out.pop(); else if (s && s !== ".") out.push(s); }
  return "/" + out.join("/");
}

export function renderMarkdown(src: string, file: string) {
  const base = parentOf(file);
  const div = document.createElement("div");
  div.className = "md";
  // Sanitize: the page can call file commands, so no script/handlers from a document may survive.
  div.innerHTML = DOMPurify.sanitize(marked.parse(src, { gfm: true, async: false }) as string, { ADD_ATTR: ["checked"] });
  div.querySelectorAll("img").forEach((img) => {
    const s = img.getAttribute("src") ?? "";
    if (s && !/^(https?:|data:)/i.test(s)) img.src = assetUrl(resolveRel(base, s));
  });
  div.querySelectorAll<HTMLElement>("pre code").forEach((c) => {
    const lang = [...c.classList].find((x) => x.startsWith("language-"))?.slice(9);
    if (lang && hljs.getLanguage(LANG[lang] ?? lang)) c.innerHTML = hljs.highlight(c.textContent ?? "", { language: LANG[lang] ?? lang }).value;
    else if ((c.textContent ?? "").length < 5000) c.innerHTML = hljs.highlightAuto(c.textContent ?? "").value;
  });
  div.querySelectorAll("li input[type=checkbox]").forEach((i) => i.closest("li")?.classList.add("task-list-item"));
  div.addEventListener("click", (ev) => {
    const a = (ev.target as HTMLElement).closest("a"); if (!a) return;
    ev.preventDefault();
    const href = a.getAttribute("href") ?? "";
    if (href.startsWith("#")) { div.querySelector(`[id="${CSS.escape(href.slice(1))}"]`)?.scrollIntoView(); return; }
    invoke("open_path", { path: /^[a-z]+:/i.test(href) && !href.startsWith("file:") ? href : resolveRel(base, href) }).catch((e) => flash(String(e)));
  });
  return div;
}

type Big = { el: HTMLElement; tools?: HTMLElement };
/** Build the preview element for a file. `big` = Quick Look sizes. */
async function previewOf(e: Entry, big: boolean): Promise<Big | null> {
  const x = ext(e), k = kindOf(e);
  const url = assetUrl(e.path);
  if (e.preview) { const i = new Image(); i.src = e.preview; i.draggable = false; return { el: i }; }
  if (e.dir || e.special || e.broken) return null;
  if (k === "img") { const i = new Image(); i.src = url; i.draggable = false; return { el: i }; }
  if (k === "vid") { const v = document.createElement("video"); v.src = url; v.controls = true; v.preload = "metadata"; return { el: v }; }
  if (k === "aud") { const a = document.createElement("audio"); a.src = url; a.controls = true; a.preload = "metadata"; return { el: a }; }
  if (x === "pdf") {
    const p = await invoke<string>("thumbnail", { path: e.path, size: big ? 1400 : 600 }).catch(() => "");
    if (!p) return null;
    const i = new Image(); i.src = assetUrl(p); return { el: i };
  }
  if (e.size > 64 * 1024 * 1024) return null;
  const t = await invoke<{ text: string; truncated: boolean; binary: boolean }>("read_text", { path: e.path, max: big ? 2_000_000 : 256_000 }).catch(() => null);
  if (!t || t.binary) return null;
  const note = t.truncated ? `<p class="hint">Showing the first ${fmtSize(t.text.length)}.</p>` : "";
  if (x === "md" || x === "markdown") {
    const wrap = document.createElement("div");
    const rendered = renderMarkdown(t.text, e.path), source = highlight(t.text, "md");
    source.hidden = true;
    const tools = document.createElement("div");
    tools.className = "pv-tools";
    tools.innerHTML = `<button data-src>View source</button>`;
    tools.querySelector("button")!.onclick = (ev) => {
      source.hidden = !source.hidden; rendered.hidden = !source.hidden;
      (ev.target as HTMLElement).textContent = source.hidden ? "View source" : "Rendered";
    };
    wrap.append(tools, rendered, source);
    wrap.insertAdjacentHTML("beforeend", note);
    return { el: wrap };
  }
  const wrap = document.createElement("div");
  wrap.append(highlight(t.text, x));
  wrap.insertAdjacentHTML("beforeend", note);
  return { el: wrap };
}

// ---------- info panel ----------
let token = 0;
hooks.info.push((el, picked, p) => {
  if (picked.length !== 1 || picked[0].trashId) return false;
  const e = picked[0], my = ++token;
  const pairs: [string, string][] = [["Type", typeName(e)]];
  if (!e.dir && !e.special) pairs.push(["Size", `${fmtSize(e.size)} (${e.size.toLocaleString()} bytes)`]);
  pairs.push(["Modified", fmtDate(e.mtime)], ["Location", parentOf(e.path)]);
  el.innerHTML = `<div class="preview"><div class="glyph">${glyph(e)}</div></div><h2>${esc(shown(e.name))}</h2>${dl(pairs)}<div class="more"></div>`;
  const more = el.querySelector<HTMLElement>(".more")!;
  if (e.dir) {
    invoke<Entry[]>("list_dir", { path: e.path }).then((list) => {
      if (my !== token) return;
      const hidden = list.filter((x) => x.hidden).length;
      more.innerHTML = dl([["Contents", `${list.length - hidden} items${hidden ? ` (+${hidden} hidden)` : ""}`]]) + `<button class="calc">Calculate total size</button>`;
      more.querySelector<HTMLElement>(".calc")!.onclick = (ev) => {
        (ev.target as HTMLButtonElement).textContent = "Calculating…";
        invoke<{ files: number; dirs: number; bytes: number }>("dir_stats", { path: e.path }).then((s) => {
          if (my !== token) return;
          more.innerHTML = dl([["Contents", `${s.files.toLocaleString()} files, ${s.dirs.toLocaleString()} folders`], ["Total size", `${fmtSize(s.bytes)} (${s.bytes.toLocaleString()} bytes)`]]);
        });
      };
    }).catch((err) => { if (my === token) more.textContent = String(err); });
    hooks.infoExtra.forEach((f) => f(el, [e]));
    return true;
  }
  previewOf(e, false).then((pv) => {
    if (my !== token || !pv) return;
    const box = el.querySelector<HTMLElement>(".preview")!;
    box.replaceChildren(pv.el);
    const big = document.createElement("button");
    big.className = "pv-big";
    big.textContent = "⤢ Open large";
    big.title = "Open this preview full size (Space)";
    big.onclick = () => showQL();
    let tools = box.querySelector(".pv-tools");
    if (!tools) { tools = document.createElement("div"); tools.className = "pv-tools"; box.prepend(tools); }
    tools.append(big);
  });
  hooks.infoExtra.forEach((f) => f(el, [e]));
  return true;
});

// ---------- Quick Look ----------
const ql = $("quicklook");
let qlToken = 0;
async function showQL() {
  const p = pane(), e = p.entry(p.focus) ?? p.selected()[0];
  if (!e) { closeQL(); return; }
  const my = ++qlToken;
  ql.hidden = false;
  ql.innerHTML = `<div class="ql-head"><span>${esc(shown(e.name))}</span><em>${esc(typeName(e))}${e.dir ? "" : ` · ${fmtSize(e.size)}`} · ←/→ browse · Esc close</em></div><div class="ql-body"></div>`;
  const body = ql.querySelector<HTMLElement>(".ql-body")!;
  const pv = e.dir ? null : await previewOf(e, true);
  if (my !== qlToken) return;
  if (pv) body.replaceChildren(pv.el);
  else body.innerHTML = `<div class="glyph" style="transform:scale(1.6)">${glyph(e)}</div>`;
  body.querySelector("video")?.play().catch(() => {});
}
function closeQL() { qlToken++; ql.hidden = true; ql.innerHTML = ""; pane().scroller.focus({ preventScroll: true }); }
window.addEventListener("keydown", (ev) => {
  if (ql.hidden) return;
  const k = ev.key;
  if (k === "Escape" || k === " ") { ev.preventDefault(); closeQL(); return; }
  if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(k)) {
    ev.preventDefault();
    const p = pane();
    const dir = k === "ArrowLeft" || k === "ArrowUp" ? -1 : 1;
    const i = (p.index.get(p.focus) ?? -1) + dir;
    if (i < 0 || i >= p.rows.length) return;
    p.selectPaths([p.rows[i].e.path]);
    showQL();
  }
});
ql.addEventListener("dblclick", closeQL);
hooks.keys.push((ev) => {
  if (ev.key === " " && !ev.ctrlKey && !ev.altKey) { showQL(); return true; }
  return false;
});
