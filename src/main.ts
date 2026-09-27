import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type Entry = {
  name: string; path: string; dir: boolean; link: boolean; broken: boolean;
  special: string; hidden: boolean; size: number; mtime: number;
};
type Row = { e: Entry; depth: number; parent: string };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const pane = $("pane"), rowsEl = $("rows"), spacer = $("spacer"), band = $("band");
const crumbs = $("crumbs"), pathedit = $<HTMLInputElement>("pathedit"), filterEl = $<HTMLInputElement>("filter");
const sortEl = $<HTMLSelectElement>("sort"), sortDirBtn = $("sortdir"), hiddenBtn = $("hidden");

const ROW = 36;
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

// ---------- state ----------
let cwd = "";
const back: string[] = [], fwd: string[] = [];
const cache = new Map<string, Entry[]>();   // folder -> listing
const expanded = new Set<string>();
let rows: Row[] = [];
let index = new Map<string, number>();      // path -> row index
const sel = new Set<string>();
let anchor = "", focus = "";
let sortKey = "name", asc = true, showHidden = false, filter = "";
let pop: { parent: string; until: number } | null = null;
const seqs = new Map<string, number>(); // latest listing request per folder
let space: [number, number] | null = null;

// ---------- helpers ----------
const shown = (s: string) => s.replace(/[\n\r\t]/g, "↵");
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
const ext = (e: Entry) => { const i = e.name.lastIndexOf("."); return !e.dir && i > 0 ? e.name.slice(i + 1).toLowerCase() : ""; };
const parentOf = (p: string) => (p === "/" ? "/" : p.slice(0, p.lastIndexOf("/")) || "/");
const baseName = (p: string) => (p === "/" ? "/" : p.slice(p.lastIndexOf("/") + 1));
function fmtSize(n: number) {
  if (n < 1024) return `${n} B`;
  const u = ["KiB", "MiB", "GiB", "TiB", "PiB"]; let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return `${n.toFixed(n < 10 ? 1 : 0)} ${u[i]}`;
}
const dateFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
const fmtDate = (ms: number) => dateFmt.format(ms);

const KINDS: Record<string, string> = {};
for (const [k, list] of Object.entries({
  img: "png jpg jpeg gif webp svg bmp tiff tif avif heic ico jxl raw cr2 nef",
  vid: "mp4 mkv webm avi mov wmv flv m4v mpg mpeg",
  aud: "mp3 flac ogg opus wav m4a aac wma",
  arc: "zip tar gz xz bz2 zst 7z rar tgz iso deb rpm appimage",
  code: "ts js mjs py rs c h cpp hpp go java kt sh fish zsh lua json toml yaml yml xml html css scss sql rb php cs qml nix",
  doc: "pdf md txt odt ods odp doc docx xls xlsx ppt pptx rtf epub csv",
})) for (const x of list.split(" ")) KINDS[x] = k;
const kindVar = (e: Entry) => `var(--k-${KINDS[ext(e)] ?? "other"})`;

function typeName(e: Entry) {
  if (e.broken) return "Broken link";
  if (e.dir) return e.link ? "Folder (link)" : "Folder";
  if (e.special) return { fifo: "Named pipe", socket: "Socket", char: "Character device", block: "Block device" }[e.special] ?? e.special;
  const x = ext(e);
  return (x ? `${x.toUpperCase()} file` : "File") + (e.link ? " (link)" : "");
}

// ---------- sorting / filtering / flattening ----------
function cmp(a: Entry, b: Entry) {
  if (a.dir !== b.dir) return a.dir ? -1 : 1;
  let r = 0;
  if (sortKey === "size") r = a.size - b.size;
  else if (sortKey === "mtime") r = a.mtime - b.mtime;
  else if (sortKey === "type") r = collator.compare(ext(a), ext(b));
  if (r === 0) r = collator.compare(a.name, b.name);
  return asc ? r : -r;
}

function rebuild() {
  const f = filter.toLowerCase();
  const out: Row[] = [];
  const walk = (dir: string, depth: number): boolean => {
    let any = false;
    const list = (cache.get(dir) ?? []).filter((e) => showHidden || !e.hidden).sort(cmp);
    for (const e of list) {
      const at = out.length;
      out.push({ e, depth, parent: dir });
      const kids = e.dir && expanded.has(e.path) && walk(e.path, depth + 1);
      // With a filter, keep a row if it matches or an expanded descendant does.
      if (f && !kids && !e.name.toLowerCase().includes(f)) out.length = at;
      else any = true;
    }
    return any;
  };
  walk(cwd, 0);
  rows = out;
  index = new Map(rows.map((r, i) => [r.e.path, i]));
  if (cache.has(cwd)) for (const p of sel) if (!index.has(p)) sel.delete(p); // keep pre-selection until the listing lands
  spacer.style.height = `${rows.length * ROW + 16}px`;
  $("empty").hidden = rows.length > 0;
  $("empty").textContent = cache.has(cwd) ? (filter ? "Nothing matches the filter" : "Empty drawer") : "Opening…";
  render();
  status();
  info();
}

// ---------- rendering (virtualized) ----------
function render() {
  const first = Math.max(0, Math.floor(pane.scrollTop / ROW) - 6);
  const last = Math.min(rows.length, Math.ceil((pane.scrollTop + pane.clientHeight) / ROW) + 6);
  const now = performance.now();
  if (pop && now > pop.until) pop = null;
  let html = "";
  let popN = 0;
  for (let i = first; i < last; i++) {
    const { e, depth, parent } = rows[i];
    const cls = [e.dir ? "folder" : "file"];
    if (sel.has(e.path)) cls.push("sel");
    if (focus === e.path) cls.push("focus");
    if (e.link) cls.push("link");
    if (e.broken) cls.push("broken");
    if (e.hidden) cls.push("hiddenf");
    if (e.dir && expanded.has(e.path)) cls.push("open");
    let style = `transform:translateY(${i * ROW}px);--depth:${depth}`;
    if (pop && parent === pop.parent) { cls.push("pop"); style += `;--d:${Math.min(popN++, 20) * 22}ms`; }
    const date = `<span class="meta date">${fmtDate(e.mtime)}</span>`;
    if (e.dir) {
      style += `;--stagger:${(i % 5) * 58 + 24}px`;
      html += `<div class="row ${cls.join(" ")}" data-i="${i}" style="${style}"><div class="tab"></div>` +
        `<div class="body"><span class="chev">›</span><span class="name grab">${esc(shown(e.name))}</span>` +
        `<span class="meta size"></span>${date}</div></div>`;
    } else {
      const x = ext(e) || (e.special ? e.special : "");
      html += `<div class="row ${cls.join(" ")}" data-i="${i}" style="${style};--edge:${kindVar(e)}">` +
        `<div class="paper"><span class="badge">${esc(x.slice(0, 5)) || "—"}</span><span class="name grab">${esc(shown(e.name))}</span>` +
        `<span class="meta size">${e.special || e.broken ? "" : fmtSize(e.size)}</span>${date}</div></div>`;
    }
  }
  rowsEl.innerHTML = html;
}
pane.addEventListener("scroll", () => { render(); if (bandState) moveBand(); }, { passive: true });
new ResizeObserver(() => render()).observe(pane);

function renderCrumbs() {
  const parts = cwd.split("/").filter(Boolean);
  let acc = "";
  let html = `<button data-p="/">/</button>`;
  for (const p of parts) { acc += "/" + p; html += `<span class="sep">›</span><button data-p="${esc(acc)}">${esc(p)}</button>`; }
  crumbs.innerHTML = html;
  crumbs.scrollLeft = crumbs.scrollWidth;
  ($("back") as HTMLButtonElement).disabled = !back.length;
  ($("fwd") as HTMLButtonElement).disabled = !fwd.length;
  document.querySelectorAll<HTMLElement>(".drawer").forEach((d) => d.classList.toggle("active", d.dataset.p === cwd));
}

function status() {
  const top = rows.filter((r) => r.depth === 0);
  const nd = top.filter((r) => r.e.dir).length;
  $("st-count").textContent = `${nd} folder${nd === 1 ? "" : "s"}, ${top.length - nd} file${top.length - nd === 1 ? "" : "s"}`;
  let size = 0;
  for (const p of sel) { const r = rows[index.get(p)!]; if (r && !r.e.dir) size += r.e.size; }
  $("st-sel").textContent = sel.size ? `${sel.size} selected (${fmtSize(size)})` : "";
  $("st-free").textContent = space ? `${fmtSize(space[0])} free of ${fmtSize(space[1])}` : "";
}

function info() {
  const el = $("info");
  const picked = [...sel].map((p) => rows[index.get(p)!]?.e).filter(Boolean);
  const dl = (pairs: [string, string][]) => `<dl>${pairs.map(([k, v]) => `<dt>${k}</dt><dd>${esc(v)}</dd>`).join("")}</dl>`;
  if (picked.length === 1) {
    const e = picked[0];
    const glyph = e.dir ? `<div class="gf"></div>` : `<div class="gp" style="--edge:${kindVar(e)}"><span class="badge">${esc(ext(e).slice(0, 5)) || "—"}</span></div>`;
    const pairs: [string, string][] = [["Type", typeName(e)]];
    if (!e.dir && !e.special) pairs.push(["Size", `${fmtSize(e.size)} (${e.size.toLocaleString()} bytes)`]);
    if (e.dir && cache.has(e.path)) pairs.push(["Contents", `${cache.get(e.path)!.length} items`]);
    pairs.push(["Modified", fmtDate(e.mtime)], ["Location", parentOf(e.path)]);
    el.innerHTML = `<div class="glyph">${glyph}</div><h2>${esc(e.name)}</h2>${dl(pairs)}`;
  } else if (picked.length > 1) {
    const files = picked.filter((e) => !e.dir);
    el.innerHTML = `<div class="glyph"><div class="gf"></div></div><h2>${picked.length} items selected</h2>` +
      dl([["Folders", String(picked.length - files.length)], ["Files", String(files.length)], ["Size", fmtSize(files.reduce((s, e) => s + e.size, 0))]]);
  } else {
    const n = cache.get(cwd)?.filter((e) => showHidden || !e.hidden).length ?? 0;
    const pairs: [string, string][] = [["Items", String(n)], ["Location", cwd]];
    if (space) pairs.push(["Free", `${fmtSize(space[0])} of ${fmtSize(space[1])}`]);
    el.innerHTML = `<div class="glyph"><div class="gf"></div></div><h2>${esc(baseName(cwd))}</h2>${dl(pairs)}`;
  }
}

// ---------- loading / navigation ----------
async function load(dir: string) {
  const seq = (seqs.get(dir) ?? 0) + 1;
  seqs.set(dir, seq);
  try {
    const list = await invoke<Entry[]>("list_dir", { path: dir });
    if (seqs.get(dir) !== seq || (dir !== cwd && !expanded.has(dir))) return; // stale or no longer shown
    cache.set(dir, list);
  } catch (err) {
    flash(String(err));
    // cwd vanished or is unreadable: fall back to its parent.
    if (dir === cwd) { if (dir !== "/") navigate(parentOf(dir), false); return; }
    expanded.delete(dir);
    cache.delete(dir);
  }
  rebuild();
}

function syncWatch() { invoke("watch", { paths: [cwd, ...expanded] }); }

async function navigate(dir: string, push = true, select?: string) {
  if (dir === cwd) return;
  if (push && cwd) { back.push(cwd); fwd.length = 0; }
  cwd = dir;
  cache.clear(); expanded.clear(); sel.clear();
  filter = filterEl.value = "";
  focus = anchor = select ?? "";
  if (select) sel.add(select);
  pane.scrollTop = 0;
  renderCrumbs();
  syncWatch();
  rebuild();
  invoke<[number, number] | null>("disk_space", { path: dir }).then((s) => { space = s; status(); });
  await load(dir);
  if (select) scrollTo(select);
}

const goBack = () => { const p = back.pop(); if (p) { fwd.push(cwd); navigate(p, false); } };
const goFwd = () => { const p = fwd.pop(); if (p) { back.push(cwd); navigate(p, false); } };
const goUp = () => { if (cwd !== "/") navigate(parentOf(cwd), true, cwd); };
const goHome = () => invoke<string>("resolve_path", { input: "~", cwd }).then((p) => navigate(p));

async function toggleExpand(p: string, open = !expanded.has(p)) {
  if (open === expanded.has(p)) return;
  if (!open) {
    for (const x of [...expanded]) if (x === p || x.startsWith(p + "/")) { expanded.delete(x); cache.delete(x); }
  } else {
    expanded.add(p);
    pop = { parent: p, until: performance.now() + 700 };
    await load(p);
  }
  syncWatch();
  rebuild();
}

function openEntry(e: Entry) {
  if (e.dir) navigate(e.path);
  else invoke("open_path", { path: e.path }).catch((err) => flash(String(err)));
}

// Live refresh: coalesce bursts of inotify events per folder.
const pending = new Set<string>();
let pendingTimer = 0;
listen<string[]>("fs-change", ({ payload }) => {
  for (const d of payload) if (d === cwd || expanded.has(d)) pending.add(d);
  if (pending.size && !pendingTimer) pendingTimer = window.setTimeout(() => {
    pendingTimer = 0;
    const dirs = [...pending]; pending.clear();
    dirs.forEach(load);
  }, 120);
});

function flash(msg: string) { $("st-sel").textContent = msg; }

// ---------- selection ----------
function scrollTo(p: string) {
  const i = index.get(p); if (i === undefined) return;
  const top = i * ROW, h = pane.clientHeight;
  if (top < pane.scrollTop) pane.scrollTop = top;
  else if (top + ROW + 16 > pane.scrollTop + h) pane.scrollTop = top + ROW + 16 - h;
}
function selectRange(a: string, b: string, add: boolean) {
  const i = index.get(a) ?? 0, j = index.get(b) ?? 0;
  if (!add) sel.clear();
  for (let k = Math.min(i, j); k <= Math.max(i, j); k++) sel.add(rows[k].e.path);
}
function clickRow(i: number, ev: MouseEvent | KeyboardEvent) {
  const p = rows[i].e.path;
  if (ev.shiftKey && anchor && index.has(anchor)) selectRange(anchor, p, ev.ctrlKey);
  else if (ev.ctrlKey) { sel.has(p) ? sel.delete(p) : sel.add(p); anchor = p; }
  else { sel.clear(); sel.add(p); anchor = p; }
  focus = p;
  refreshSel();
}
function refreshSel() { render(); status(); info(); }

// ---------- mouse: click, double-click, rubber band ----------
let bandState: { x0: number; y0: number; x: number; y: number; base: Set<string>; moved: boolean; row: number; ev: MouseEvent } | null = null;
const rowAt = (t: EventTarget | null) => { const r = (t as HTMLElement).closest?.(".row") as HTMLElement | null; return r ? +r.dataset.i! : -1; };

pane.addEventListener("mousedown", (ev) => {
  if (ev.button !== 0) return;
  pane.focus();
  const t = ev.target as HTMLElement, i = rowAt(t);
  if (i >= 0 && t.classList.contains("chev")) { toggleExpand(rows[i].e.path); return; }
  if (i >= 0 && t.closest(".grab, .badge")) { clickRow(i, ev); return; }
  // Anywhere else (row whitespace or empty pane) starts a rubber band; a click without a drag acts as a click.
  const r = pane.getBoundingClientRect();
  const x = ev.clientX - r.left, y = ev.clientY - r.top + pane.scrollTop;
  bandState = { x0: x, y0: y, x, y, base: ev.ctrlKey ? new Set(sel) : new Set(), moved: false, row: i, ev };
});
window.addEventListener("mousemove", (ev) => {
  if (!bandState) return;
  const r = pane.getBoundingClientRect();
  bandState.x = ev.clientX - r.left; bandState.y = ev.clientY - r.top + pane.scrollTop;
  if (!bandState.moved && Math.hypot(bandState.x - bandState.x0, bandState.y - bandState.y0) < 5) return;
  bandState.moved = true;
  // NOTE: autoscroll only while the mouse moves; add a rAF loop if holding still at the edge matters.
  if (ev.clientY > r.bottom - 24) pane.scrollTop += 20; else if (ev.clientY < r.top + 24) pane.scrollTop -= 20;
  moveBand();
});
function moveBand() {
  const b = bandState!; if (!b.moved) return;
  const top = Math.min(b.y0, b.y), bot = Math.max(b.y0, b.y);
  Object.assign(band.style, { left: `${Math.min(b.x0, b.x)}px`, top: `${top}px`, width: `${Math.abs(b.x - b.x0)}px`, height: `${bot - top}px` });
  band.hidden = false;
  sel.clear(); b.base.forEach((p) => sel.add(p));
  const off = 8; // #rows top offset
  const i0 = Math.max(0, Math.floor((top - off) / ROW)), i1 = Math.min(rows.length - 1, Math.floor((bot - off) / ROW));
  for (let k = i0; k <= i1; k++) sel.add(rows[k].e.path);
  if (i1 >= i0 && i1 >= 0) focus = rows[Math.max(i0, Math.min(i1, rows.length - 1))].e.path;
  refreshSel();
}
window.addEventListener("mouseup", () => {
  const b = bandState; bandState = null; band.hidden = true;
  if (!b || b.moved) return;
  if (b.row >= 0) clickRow(b.row, b.ev);
  else if (!b.ev.ctrlKey) { sel.clear(); refreshSel(); }
});
pane.addEventListener("dblclick", (ev) => {
  const i = rowAt(ev.target); if (i < 0 || (ev.target as HTMLElement).classList.contains("chev")) return;
  openEntry(rows[i].e);
});
window.addEventListener("mouseup", (ev) => { if (ev.button === 3) goBack(); if (ev.button === 4) goFwd(); });

// ---------- keyboard ----------
function moveFocus(to: number, ev: KeyboardEvent) {
  if (!rows.length) return;
  to = Math.max(0, Math.min(rows.length - 1, to));
  const p = rows[to].e.path;
  if (ev.shiftKey) { if (!anchor || !index.has(anchor)) anchor = focus || p; selectRange(anchor, p, ev.ctrlKey); }
  else if (!ev.ctrlKey) { sel.clear(); sel.add(p); anchor = p; }
  focus = p;
  scrollTo(p);
  refreshSel();
}

window.addEventListener("keydown", (ev) => {
  const k = ev.key, typing = document.activeElement === filterEl || document.activeElement === pathedit;
  const fi = index.get(focus) ?? -1;
  const page = Math.max(1, Math.floor(pane.clientHeight / ROW) - 1);
  if (ev.ctrlKey && k.toLowerCase() === "l") { ev.preventDefault(); editPath(); return; }
  if (ev.ctrlKey && k.toLowerCase() === "h") { ev.preventDefault(); toggleHidden(); return; }
  if (ev.ctrlKey && k.toLowerCase() === "i") { ev.preventDefault(); filterEl.focus(); return; }
  if (ev.altKey && k === "ArrowLeft") { ev.preventDefault(); goBack(); return; }
  if (ev.altKey && k === "ArrowRight") { ev.preventDefault(); goFwd(); return; }
  if (ev.altKey && k === "ArrowUp") { ev.preventDefault(); goUp(); return; }
  if (ev.altKey && k === "Home") { ev.preventDefault(); goHome(); return; }
  if (k === "F5") { ev.preventDefault(); load(cwd); return; }
  if (typing) {
    if (document.activeElement === filterEl && (k === "ArrowDown" || k === "Enter")) { ev.preventDefault(); pane.focus(); moveFocus(0, ev); }
    if (document.activeElement === filterEl && k === "Escape") { filterEl.value = ""; filter = ""; rebuild(); pane.focus(); }
    return;
  }
  switch (k) {
    case "ArrowDown": ev.preventDefault(); moveFocus(fi + 1, ev); return;
    case "ArrowUp": ev.preventDefault(); moveFocus(fi < 0 ? 0 : fi - 1, ev); return;
    case "PageDown": ev.preventDefault(); moveFocus(fi + page, ev); return;
    case "PageUp": ev.preventDefault(); moveFocus(fi - page, ev); return;
    case "Home": ev.preventDefault(); moveFocus(0, ev); return;
    case "End": ev.preventDefault(); moveFocus(rows.length - 1, ev); return;
    case "ArrowRight": if (fi >= 0 && rows[fi].e.dir) { ev.preventDefault(); toggleExpand(focus, true); } return;
    case "ArrowLeft":
      if (fi < 0) return; ev.preventDefault();
      if (expanded.has(focus)) toggleExpand(focus, false);
      else if (rows[fi].depth > 0) moveFocus(index.get(rows[fi].parent)!, ev);
      return;
    case "Enter": {
      if (fi < 0) return;
      const picked = sel.size ? [...sel].map((p) => rows[index.get(p)!].e) : [rows[fi].e];
      const dir = picked.find((e) => e.dir);
      if (dir) openEntry(dir); else picked.forEach(openEntry);
      return;
    }
    case "Backspace": ev.preventDefault(); goUp(); return;
    case "Escape": if (filter) { filterEl.value = filter = ""; rebuild(); } else { sel.clear(); refreshSel(); } return;
    case " ": if (ev.ctrlKey && focus) { ev.preventDefault(); sel.has(focus) ? sel.delete(focus) : sel.add(focus); refreshSel(); } return;
  }
  if (ev.ctrlKey && k.toLowerCase() === "a") { ev.preventDefault(); rows.forEach((r) => sel.add(r.e.path)); refreshSel(); return; }
  // Type-to-filter: printable keys go to the filter box.
  if (k.length === 1 && !ev.ctrlKey && !ev.altKey && !ev.metaKey && k !== " ") { filterEl.focus(); }
});

filterEl.addEventListener("input", () => { filter = filterEl.value; pane.scrollTop = 0; rebuild(); });

// ---------- location bar ----------
function editPath() {
  crumbs.hidden = true; pathedit.hidden = false;
  pathedit.value = cwd; pathedit.focus(); pathedit.select();
}
function endEdit() { pathedit.hidden = true; crumbs.hidden = false; }
crumbs.addEventListener("click", (ev) => {
  const b = (ev.target as HTMLElement).closest("button");
  if (b) navigate(b.dataset.p!); else editPath();
});
pathedit.addEventListener("keydown", async (ev) => {
  if (ev.key === "Escape") { endEdit(); pane.focus(); }
  if (ev.key !== "Enter") return;
  try {
    const p = await invoke<string>("resolve_path", { input: pathedit.value, cwd });
    endEdit(); pane.focus(); navigate(p);
  } catch (err) { flash(String(err)); pathedit.select(); }
});
pathedit.addEventListener("blur", endEdit);

// ---------- top bar ----------
$("back").onclick = goBack; $("fwd").onclick = goFwd; $("up").onclick = goUp; $("home").onclick = goHome;
function toggleHidden() { showHidden = !showHidden; hiddenBtn.classList.toggle("on", showHidden); rebuild(); }
hiddenBtn.onclick = toggleHidden;
sortEl.onchange = () => { sortKey = sortEl.value; rebuild(); };
sortDirBtn.onclick = () => { asc = !asc; sortDirBtn.textContent = asc ? "↓" : "↑"; rebuild(); };

// ---------- sidebar ----------
invoke<{ name: string; path: string }[]>("places").then((places) => {
  $("places").innerHTML = places.map((p) =>
    `<div class="drawer" data-p="${esc(p.path)}" title="${esc(p.path)}"><span class="label">${esc(p.name)}</span><span class="handle"></span></div>`).join("");
  renderCrumbs();
});
$("places").addEventListener("click", (ev) => {
  const d = (ev.target as HTMLElement).closest<HTMLElement>(".drawer"); if (d) navigate(d.dataset.p!);
});

invoke<string>("start_path").then((p) => navigate(p));
pane.focus();

if (import.meta.env.DEV) {
  (window as any).wc = { get rows() { return rows; }, get cwd() { return cwd; }, sel, expanded, navigate, pane, filterEl };
  invoke<boolean>("selftest", {}).then((on) => on && import("./selftest"));
}
