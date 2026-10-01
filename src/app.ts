// App shell: tabs (each with one or two panes), top bar, sidebar, info panel, status bar, keyboard routing.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { Pane, Host, isFolder, View } from "./pane";
import { Settings } from "./theme";
import { $, Entry, invoke, esc, shown, baseName, parentOf, fmtSize, fmtDate, typeName, ext, edgeColor } from "./util";

type Tab = { panes: Pane[]; active: number };
export const tabs: Tab[] = [];
let cur = 0;
export const tab = () => tabs[cur];
export const pane = () => tab()?.panes[tab().active];
export let settings: Settings = {};
export let HOME = "/";

/** Extension points filled in by feature modules (ops, preview, tags, menu, …). */
export const hooks: {
  menu?: (p: Pane, ev: MouseEvent) => void;
  drag?: (p: Pane, ev: MouseEvent) => void;
  virtual?: Record<string, (loc: string, p: Pane) => Promise<Entry[]>>;
  thumb?: (e: Entry) => string | undefined;
  count?: (e: Entry) => string | undefined;
  info: ((el: HTMLElement, picked: Entry[], p: Pane) => boolean)[];
  /** Extra sections appended to a single item's info panel (tags, rating…). */
  infoExtra: ((el: HTMLElement, es: Entry[]) => void)[];
  tagDrop?: (tag: string, paths: string[]) => void;
  dropMenu?: (x: number, y: number, pick: (copy: boolean) => void) => void;
  addPlace?: (path: string) => void;
  locTitle?: Record<string, (loc: string) => string>;
  /** open with its app (plugins: download first) */
  open?: (e: Entry, p: Pane) => void;
  /** show it in the built-in viewer; false when the viewer can't show it */
  preview?: (e: Entry) => boolean;
  onNavigate?: (p: Pane) => void;
  keys: ((ev: KeyboardEvent, p: Pane) => boolean)[];
  /** git state letter for an entry (badges). */
  git?: (e: Entry) => string | undefined;
  /** plugin status badge for an entry */
  badge?: (e: Entry) => { text: string; title: string; state: string } | undefined;
  /** runs whenever the top bar/status is refreshed for the active pane */
  chrome: ((p: Pane) => void)[];
  /** extra folders to watch beyond what panes show (trash, .git dirs…) */
  extraWatch: (() => string[])[];
  /** extra context-menu entries for a selection (menu.ts Item[]), added before Properties */
  menuExtra: ((p: Pane, es: Entry[]) => any[])[];
  /** per location scheme: group headers, row height, opening sort */
  group: Record<string, (e: Entry) => string>;
  rowScale: Record<string, number>;
  defaultSort: Record<string, string>;
  defaultView: Record<string, View>;
  /** per location scheme: the whole context menu (menu.ts Item[]) for its items */
  locMenu: Record<string, (p: Pane, es: Entry[]) => any[]>;
  /** per location scheme: files dropped / pasted into one of its locations */
  dropInto: Record<string, (loc: string, paths: string[]) => void>;
} = { keys: [], info: [], infoExtra: [], virtual: {}, locTitle: {}, chrome: [], extraWatch: [], group: {}, rowScale: {}, defaultSort: {}, defaultView: {}, locMenu: {}, dropInto: {}, menuExtra: [] };

export const scheme = (loc: string) => loc.slice(0, loc.indexOf(":") + 1);

// ---------- view per folder (settings.folderViews: path → view/sort/zoom last used there) ----------
let viewSave = 0;
const MAX_VIEWS = 400;
export function rememberView(p: Pane) {
  if (!isFolder(p.loc)) return;
  const all: Record<string, any> = { ...(settings.folderViews ?? {}) };
  delete all[p.loc]; // re-insert last so the oldest drop off first
  all[p.loc] = { v: p.view, s: p.sortKey, a: p.asc, z: Math.round(p.zoom * 100) / 100 };
  const keys = Object.keys(all);
  for (const k of keys.slice(0, Math.max(0, keys.length - MAX_VIEWS))) delete all[k];
  settings.folderViews = all;
  clearTimeout(viewSave);
  viewSave = window.setTimeout(() => invoke("set_settings", { patch: { folderViews: all } }), 400);
}

export function flash(msg: string) {
  const el = $("st-sel");
  el.textContent = msg;
  el.classList.add("flash");
  setTimeout(() => el.classList.remove("flash"), 4000);
}

export const host: Host = {
  showHidden: false,
  singleClick: false,
  dblExpand: false,
  activate(p) {
    const t = tab(), i = t.panes.indexOf(p);
    if (i >= 0 && i !== t.active) { t.active = i; showTab(); }
  },
  changed(p) { if (!tab()) return; if (p === pane()) chrome(); renderTabbar(); },
  open(e, p) {
    if (e.dir) p.navigate(e.path);
    // plugin photos (e.preview) always open in the viewer: "Open with app" there downloads the original
    else if ((settings.openFiles === "viewer" || e.preview) && hooks.preview?.(e)) return;
    else if (hooks.open) hooks.open(e, p);
    else invoke("open_path", { path: e.path }).catch((err) => flash(String(err)));
  },
  openInTab(path) { newTab(path, false); },
  menu(p, ev) { hooks.menu?.(p, ev); },
  startDrag(p, ev) { hooks.drag?.(p, ev); },
  loadVirtual(loc, p) {
    const f = hooks.virtual?.[scheme(loc)];
    return f ? f(loc, p) : Promise.reject(`Unknown location ${loc}`);
  },
  thumb(e) { return hooks.thumb?.(e); },
  count(e) { return hooks.count?.(e); },
  git(e) { return hooks.git?.(e); },
  badge(e) { return hooks.badge?.(e); },
  defaultView(loc) { return hooks.defaultView[scheme(loc)]; },
  groupOf(loc) { return hooks.group[scheme(loc)]; },
  rowScale(loc) { return hooks.rowScale[scheme(loc)] ?? 1; },
  defaultSort(loc) { return hooks.defaultSort[scheme(loc)]; },
  folderView(loc) {
    const f = (settings.folderViews ?? {})[loc];
    const dz = +settings.zoom || 1;
    if (!f) return { view: (settings.view as View) || "cabinet", sortKey: "name", asc: true, zoom: dz };
    return { view: f.v ?? settings.view ?? "cabinet", sortKey: f.s ?? "name", asc: f.a ?? true, zoom: f.z ?? dz };
  },
  viewChanged(p) { rememberView(p); },
  syncWatch() { invoke("watch", { paths: [...new Set([...tabs.flatMap((t) => t.panes.flatMap((p) => p.watched())), ...hooks.extraWatch.flatMap((f) => f())])] }); },
  flash,
};

function makePane(loc: string, select?: string) {
  const p = new Pane(host);
  p.view = (settings.view as View) || "cabinet";
  p.el.dataset.view = p.view;
  p.setZoom(+settings.zoom || 1);
  const nav = p.navigate.bind(p);
  p.navigate = async (l, push, sel) => { const r = nav(l, push, sel); hooks.onNavigate?.(p); return r; };
  p.navigate(loc, true, select);
  return p;
}

export function newTab(loc: string, activate = true, select?: string) {
  tabs.push({ panes: [makePane(loc, select)], active: 0 });
  if (activate) cur = tabs.length - 1;
  showTab();
}
export function closeTab(i = cur) {
  if (tabs.length === 1) { getCurrentWindow().close(); return; }
  tabs.splice(i, 1);
  if (cur >= tabs.length) cur = tabs.length - 1;
  else if (i < cur) cur--;
  host.syncWatch();
  showTab();
}
export function toggleSplit() {
  const t = tab();
  if (t.panes.length === 2) { t.panes.splice(t.active === 0 ? 1 : 0, 1); t.active = 0; }
  else { t.panes.push(makePane(pane().loc)); t.active = 1; }
  host.syncWatch();
  showTab();
}

function showTab() {
  const views = $("views");
  const t = tab();
  if ([...views.children].some((c, i) => c !== t.panes[i]?.el) || views.children.length !== t.panes.length) views.replaceChildren(...t.panes.map((p) => p.el));
  views.classList.toggle("split", t.panes.length > 1);
  t.panes.forEach((p, i) => p.el.classList.toggle("active", i === t.active));
  pane().scroller.focus({ preventScroll: true });
  chrome();
  renderTabbar();
}

export function locTitle(loc: string) {
  const f = hooks.locTitle?.[scheme(loc)];
  return f ? f(loc) : baseName(loc) || "/";
}

// ---------- session: this window's tabs, saved by the backend when the app closes ----------
type TabState = { locs: string[]; active: number };
export type WinState = { tabs: TabState[]; cur: number };
// searches and duplicate scans are one-off; everything else can be reopened
const restorable = (loc: string) => !loc.startsWith("search:") && !loc.startsWith("dupes:");
let sentSession = "", sessionTimer = 0;
function reportSession() {
  clearTimeout(sessionTimer);
  sessionTimer = window.setTimeout(() => {
    const st: WinState = { tabs: tabs.map((t) => ({ locs: t.panes.map((p) => p.loc).filter(restorable), active: t.active })).filter((t) => t.locs.length), cur };
    const s = JSON.stringify(st);
    if (s !== sentSession) { sentSession = s; invoke("session_update", { state: st }); }
  }, 600);
}
/** Replace this window's tabs with a saved window state. */
export function openState(st: WinState) {
  tabs.length = 0;
  for (const t of st.tabs) {
    tabs.push({ panes: t.locs.map((l) => makePane(l)), active: Math.min(t.active, t.locs.length - 1) });
  }
  if (!tabs.length) tabs.push({ panes: [makePane(HOME)], active: 0 });
  cur = Math.min(st.cur, tabs.length - 1);
  host.syncWatch();
  showTab();
}
const tabCount = (ws: WinState[]) => ws.reduce((n, w) => n + w.tabs.length, 0);
function offerRestore(ws: WinState[]) {
  const trivial = ws.length === 1 && ws[0].tabs.length === 1 && ws[0].tabs[0].locs.length === 1 && ws[0].tabs[0].locs[0] === HOME;
  if (!ws.length || trivial || settings.restoreSession === "never") return;
  const restore = () => { openState(ws[0]); ws.slice(1).forEach((w) => invoke("open_session_window", { state: w })); };
  if (settings.restoreSession === "always") { restore(); return; }
  const t = document.createElement("div");
  t.className = "toast";
  const n = tabCount(ws);
  t.innerHTML = `<span>Restore your last session? ${ws.length > 1 ? `${ws.length} windows, ` : ""}${n} tab${n === 1 ? "" : "s"}</span>
    <button data-v="1" class="primary">Restore</button><button data-v="0">No thanks</button>
    <label title="Remember this answer (change it in Settings)"><input type="checkbox"> Always</label>`;
  document.body.append(t);
  const done = (yes: boolean) => {
    if (t.querySelector<HTMLInputElement>("input")!.checked) invoke("set_settings", { patch: { restoreSession: yes ? "always" : "never" } });
    t.remove();
    if (yes) restore();
  };
  t.addEventListener("click", (e) => { const v = (e.target as HTMLElement).dataset.v; if (v) done(v === "1"); });
  // gone by itself after a while, and as soon as you start browsing somewhere else
  setTimeout(() => t.remove(), 20000);
  closeOffer = () => t.remove();
}
let closeOffer = () => {};

function renderTabbar() {
  reportSession();
  const bar = $("tabbar");
  bar.hidden = tabs.length < 2;
  if (bar.hidden) return;
  bar.innerHTML = tabs.map((t, i) => `<div class="tabitem${i === cur ? " cur" : ""}" data-i="${i}"><span>${esc(locTitle(t.panes[t.active].loc))}</span><b data-close="${i}">✕</b></div>`).join("");
}
$("tabbar").addEventListener("mousedown", (ev) => {
  const el = (ev.target as HTMLElement).closest<HTMLElement>(".tabitem"); if (!el) return;
  const i = +el.dataset.i!;
  if (ev.button === 1 || (ev.target as HTMLElement).dataset.close) { ev.preventDefault(); closeTab(i); return; }
  cur = i; showTab();
});

// ---------- top bar / status / info ----------
function chrome() {
  const p = pane(); if (!p) return;
  renderCrumbs(p);
  ($("back") as HTMLButtonElement).disabled = !p.back.length;
  ($("fwd") as HTMLButtonElement).disabled = !p.fwd.length;
  ($("filter") as HTMLInputElement).value = p.filter;
  ($("sort") as HTMLSelectElement).value = p.sortKey;
  ($("viewmode") as HTMLSelectElement).value = p.view;
  $("sortdir").textContent = p.asc ? "↓" : "↑";
  $("splitbtn").classList.toggle("on", tab().panes.length > 1);
  document.querySelectorAll<HTMLElement>(".drawer").forEach((d) => d.classList.toggle("active", d.dataset.p === p.loc));
  status(p);
  info(p);
  hooks.chrome.forEach((f) => f(p));
  document.title = `${(window as any).WCTEST ? "WCTEST " : ""}${locTitle(p.loc)} — Whale Cabinet`;
}

function renderCrumbs(p: Pane) {
  const crumbs = $("crumbs");
  if (!isFolder(p.loc)) { crumbs.innerHTML = `<button data-p="${esc(p.loc)}">${esc(locTitle(p.loc))}</button>`; return; }
  let acc = "", html = `<button data-p="/">/</button>`;
  for (const part of p.loc.split("/").filter(Boolean)) { acc += "/" + part; html += `<span class="sep">›</span><button data-p="${esc(acc)}">${esc(shown(part))}</button>`; }
  crumbs.innerHTML = html;
  crumbs.scrollLeft = crumbs.scrollWidth;
}

function status(p: Pane) {
  const top = p.rows.filter((r) => r.depth === 0 && !r.head);
  const nd = top.filter((r) => r.e.dir).length;
  $("st-count").textContent = `${nd} folder${nd === 1 ? "" : "s"}, ${top.length - nd} file${top.length - nd === 1 ? "" : "s"}`;
  if (!$("st-sel").classList.contains("flash")) {
    const size = p.selected().reduce((s, e) => s + (e.dir ? 0 : e.size), 0);
    $("st-sel").textContent = p.sel.size ? `${p.sel.size} selected (${fmtSize(size)})` : "";
  }
  $("st-zoom").textContent = p.zoom !== 1 ? `${Math.round(p.zoom * 100)}%` : "";
  $("st-free").textContent = p.space ? `${fmtSize(p.space[0])} free of ${fmtSize(p.space[1])}` : "";
}

export const dl = (pairs: [string, string][]) => `<dl>${pairs.map(([k, v]) => `<dt>${k}</dt><dd>${esc(v)}</dd>`).join("")}</dl>`;
export function glyph(e?: Entry) {
  return !e || e.dir ? `<div class="gf" style="${e ? `--edge:${edgeColor(e)}` : ""}"></div>` : `<div class="gp" style="--edge:${edgeColor(e)}"><span class="badge">${esc(ext(e).slice(0, 5)) || "—"}</span></div>`;
}
export function info(p = pane()) {
  if ($("info").hidden) return;
  const el = $("info-body");
  const picked = p.selected();
  for (const h of hooks.info) if (h(el, picked, p)) return;
  if (picked.length === 1) {
    const e = picked[0];
    const pairs: [string, string][] = [["Type", typeName(e)]];
    if (!e.dir && !e.special) pairs.push(["Size", `${fmtSize(e.size)} (${e.size.toLocaleString()} bytes)`]);
    pairs.push(["Modified", fmtDate(e.mtime)], ["Location", parentOf(e.path)]);
    el.innerHTML = `<div class="glyph">${glyph(e)}</div><h2>${esc(shown(e.name))}</h2>${dl(pairs)}`;
  } else if (picked.length > 1) {
    const files = picked.filter((e) => !e.dir);
    el.innerHTML = `<div class="glyph">${glyph()}</div><h2>${picked.length} items selected</h2>` +
      dl([["Folders", String(picked.length - files.length)], ["Files", String(files.length)], ["Size", fmtSize(files.reduce((s, e) => s + e.size, 0))]]);
    hooks.infoExtra.forEach((f) => f(el, picked));
  } else {
    const n = p.rows.filter((r) => r.depth === 0 && !r.head).length;
    const pairs: [string, string][] = [["Items", String(n)], ["Location", p.loc]];
    if (p.space) pairs.push(["Free", `${fmtSize(p.space[0])} of ${fmtSize(p.space[1])}`]);
    el.innerHTML = `<div class="glyph">${glyph()}</div><h2>${esc(locTitle(p.loc))}</h2>${dl(pairs)}`;
  }
}

// ---------- info panel: drag its left edge to resize (width saved in settings) ----------
$("info-grip").addEventListener("mousedown", (ev) => {
  ev.preventDefault();
  const startX = ev.clientX, startW = $("info").getBoundingClientRect().width;
  let w = startW;
  document.body.classList.add("resizing");
  const move = (m: MouseEvent) => {
    w = Math.max(260, Math.min(innerWidth * 0.7, startW + (startX - m.clientX)));
    document.documentElement.style.setProperty("--info-w", `${Math.round(w)}px`);
  };
  const up = () => {
    window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up);
    document.body.classList.remove("resizing");
    invoke("set_settings", { patch: { infoWidth: Math.round(w) } });
  };
  window.addEventListener("mousemove", move); window.addEventListener("mouseup", up);
});
$("info-grip").addEventListener("dblclick", () => { document.documentElement.style.setProperty("--info-w", "420px"); invoke("set_settings", { patch: { infoWidth: 420 } }); });

// ---------- focus mode: just the files (F12, or double-click empty top-bar space; Esc/F12 to leave) ----------
export const focusMode = () => $("app").classList.contains("zen");
export function setFocusMode(on = !focusMode(), save = true) {
  $("app").classList.toggle("zen", on);
  $("app").classList.remove("peek");
  if (save) invoke("set_settings", { patch: { focusMode: on } });
  pane()?.scroller.focus({ preventScroll: true });
  if (!on && pane()) info();
}
$("topbar").addEventListener("dblclick", (ev) => { if (ev.target === $("topbar")) setFocusMode(true); });
// in focus mode the top bar peeks in while the pointer touches the top edge (or the path is being edited)
window.addEventListener("mousemove", (ev) => { if (focusMode() && ev.clientY <= 3) $("app").classList.add("peek"); }, { passive: true });
$("topbar").addEventListener("mouseleave", () => { if (document.activeElement !== pathedit) $("app").classList.remove("peek"); });

// ---------- location bar ----------
const crumbs = $("crumbs"), pathedit = $<HTMLInputElement>("pathedit"), filterEl = $<HTMLInputElement>("filter");
export function editPath() {
  if (focusMode()) $("app").classList.add("peek");
  crumbs.hidden = true; pathedit.hidden = false;
  pathedit.value = isFolder(pane().loc) ? pane().loc : "";
  pathedit.focus(); pathedit.select();
}
function endEdit() { pathedit.hidden = true; crumbs.hidden = false; if (!$("topbar").matches(":hover")) $("app").classList.remove("peek"); }
crumbs.addEventListener("click", (ev) => {
  const b = (ev.target as HTMLElement).closest("button");
  if (b) pane().navigate(b.dataset.p!); else editPath();
});
pathedit.addEventListener("keydown", async (ev) => {
  ev.stopPropagation();
  if (ev.key === "Escape") { endEdit(); pane().scroller.focus(); }
  if (ev.key !== "Enter") return;
  try {
    const p = await invoke<string>("resolve_path", { input: pathedit.value, cwd: isFolder(pane().loc) ? pane().loc : "/" });
    endEdit(); pane().scroller.focus(); pane().navigate(p);
  } catch (err) { flash(String(err)); pathedit.select(); }
});
pathedit.addEventListener("blur", endEdit);
filterEl.addEventListener("input", () => { const p = pane(); p.filter = filterEl.value; p.scroller.scrollTop = 0; p.rebuild(); });

export const goHome = () => pane().navigate(HOME);
$("back").onclick = () => pane().goBack();
$("fwd").onclick = () => pane().goFwd();
$("up").onclick = () => pane().goUp();
$("home").onclick = goHome;
$("hidden").onclick = () => toggleHidden();
$("splitbtn").onclick = () => toggleSplit();
$("menubtn").onclick = () => openSettings();
$("sort").onchange = () => { const p = pane(); p.sortKey = ($("sort") as HTMLSelectElement).value; p.rebuild(); rememberView(p); };
$("sortdir").onclick = () => { const p = pane(); p.asc = !p.asc; p.rebuild(); rememberView(p); };
$("viewmode").onchange = () => { pane().setView(($("viewmode") as HTMLSelectElement).value as View); rememberView(pane()); };

export function toggleHidden(v = !host.showHidden) {
  host.showHidden = v;
  $("hidden").classList.toggle("on", v);
  for (const t of tabs) for (const p of t.panes) p.rebuild();
}
export const openSettings = (tab = "") => invoke("open_dialog", { kind: "settings", arg: tab, title: "Whale Cabinet Settings", width: 560, height: 640 });
export const allPanes = () => tabs.flatMap((t) => t.panes);

// ---------- sidebar: places ----------
export type PlaceItem = { name: string; path: string };
export const drawerHtml = (p: PlaceItem, extra = "") =>
  `<div class="drawer" data-p="${esc(p.path)}" title="${esc(p.path)}"><span class="label">${esc(p.name)}</span><span class="handle"></span>${extra}</div>`;
export function renderPlaces(list: PlaceItem[]) {
  $("places").innerHTML = list.map((p) => drawerHtml(p)).join("");
  chrome();
}
$("sidebar").addEventListener("click", (ev) => {
  const d = (ev.target as HTMLElement).closest<HTMLElement>(".drawer[data-p]");
  if (d && !(ev.target as HTMLElement).closest("[data-act]")) pane().navigate(d.dataset.p!);
});
$("sidebar").addEventListener("auxclick", (ev) => {
  const d = (ev.target as HTMLElement).closest<HTMLElement>(".drawer[data-p]");
  if (d && ev.button === 1) newTab(d.dataset.p!, false);
});

// ---------- keyboard ----------
const typingIn = () => { const a = document.activeElement; return a instanceof HTMLInputElement || a instanceof HTMLTextAreaElement || (a as HTMLElement)?.isContentEditable; };
window.addEventListener("keydown", (ev) => {
  if (!$("menu").hidden || !$("quicklook").hidden) return; // overlays handle their own keys
  const k = ev.key, lk = k.toLowerCase(), p = pane(), c = ev.ctrlKey && !ev.altKey;
  const stop = () => ev.preventDefault();
  if (c && lk === "l") { stop(); editPath(); return; }
  if (c && lk === "h") { stop(); toggleHidden(); return; }
  if (c && lk === "i") { stop(); filterEl.focus(); return; }
  if (c && lk === "t") { stop(); newTab(isFolder(p.loc) ? p.loc : HOME); return; }
  if (c && lk === "w") { stop(); closeTab(); return; }
  if (c && k === "Tab") { stop(); cur = (cur + (ev.shiftKey ? tabs.length - 1 : 1)) % tabs.length; showTab(); return; }
  // Dolphin: Ctrl+1 icons, Ctrl+2 compact, Ctrl+3 details (our cabinet list)
  if (c && ["1", "2", "3"].includes(k)) { stop(); p.setView((["grid", "compact", "cabinet"] as View[])[+k - 1]); rememberView(p); return; }
  if (c && k === "PageDown") { stop(); cur = (cur + 1) % tabs.length; showTab(); return; }
  if (c && k === "PageUp") { stop(); cur = (cur + tabs.length - 1) % tabs.length; showTab(); return; }
  if (c && lk === "q") { stop(); getCurrentWindow().close(); return; }
  if (c && !ev.shiftKey && lk === "n") { stop(); invoke("open_window", { loc: isFolder(p.loc) ? p.loc : HOME }); return; }
  if (k === "F12") { stop(); setFocusMode(); return; }
  if (ev.altKey && k === ".") { stop(); toggleHidden(); return; }
  if (k === "F6") { stop(); editPath(); return; }
  if (k === "F9") { stop(); const sb = $("sidebar"); sb.hidden = !sb.hidden; $("app").classList.toggle("noside", sb.hidden); return; }
  if (c && (k === "+" || k === "=")) { stop(); p.setZoom(p.zoom * 1.1); rememberView(p); return; }
  if (c && k === "-") { stop(); p.setZoom(p.zoom / 1.1); rememberView(p); return; }
  if (c && k === "0") { stop(); p.setZoom(+settings.zoom || 1); rememberView(p); return; }
  if (ev.altKey && k === "ArrowLeft") { stop(); p.goBack(); return; }
  if (ev.altKey && k === "ArrowRight") { stop(); p.goFwd(); return; }
  if (ev.altKey && k === "ArrowUp") { stop(); p.goUp(); return; }
  if (ev.altKey && k === "Home") { stop(); goHome(); return; }
  if (k === "F3") { stop(); toggleSplit(); return; }
  if (k === "F5") { stop(); p.reload(); return; }
  if (k === "F11") { stop(); const i = $("info"); i.hidden = !i.hidden; $("app").classList.toggle("noinfo", i.hidden); info(); return; }
  if (typingIn()) {
    if (document.activeElement === filterEl && (k === "ArrowDown" || k === "Enter")) { stop(); p.scroller.focus(); p.key(new KeyboardEvent("keydown", { key: "Home" })); }
    if (document.activeElement === filterEl && k === "Escape") { filterEl.value = ""; p.filter = ""; p.rebuild(); p.scroller.focus(); }
    return;
  }
  if (p.key(ev)) return;
  for (const h of hooks.keys) if (h(ev, p)) { stop(); return; }
  if (k === "Backspace") { stop(); p.goUp(); return; }
  if (k === "Escape") { if (p.filter) { p.filter = ""; p.rebuild(); } else if (p.sel.size) { p.sel.clear(); p.refreshSel(); } else if (focusMode()) setFocusMode(false); return; }
  if (ev.ctrlKey && ev.shiftKey && lk === "a") { stop(); p.invertSel(); return; }
  if (c && lk === "a") { stop(); p.selectAll(); return; }
  // Type-to-filter: printable keys go to the filter box.
  if (k.length === 1 && !ev.ctrlKey && !ev.altKey && !ev.metaKey && k !== " ") filterEl.focus();
});
window.addEventListener("mouseup", (ev) => { if (ev.button === 3) pane().goBack(); if (ev.button === 4) pane().goFwd(); });

// ---------- titlebar ----------
const win = getCurrentWindow();
$("tb-min").onclick = () => win.minimize();
$("tb-max").onclick = () => win.toggleMaximize();
$("tb-close").onclick = () => win.close();

let settings0: Settings = {};
export function applySettings(s: Settings) {
  const first = !Object.keys(settings).length;
  settings = s;
  host.singleClick = !!s.singleClick;
  host.dblExpand = s.folderDblClick === "expand";
  // the Item size slider applies to every open view
  if (!first && +s.zoom !== +settings0.zoom) allPanes().forEach((p) => p.setZoom(+s.zoom || 1));
  settings0 = s;
  const iw = Math.max(260, Math.min(1400, +s.infoWidth || 420));
  document.documentElement.style.setProperty("--info-w", `${iw}px`);
  $("titlebar").hidden = !s.titlebar;
  if (first) { toggleHidden(!!s.showHidden); setFocusMode(!!s.focusMode, false); }
}

export async function start(initial: { loc: string; select?: string }[]) {
  HOME = await invoke<string>("resolve_path", { input: "~", cwd: "/" });
  const bare = !initial.length;
  if (bare) initial = [{ loc: HOME }];
  const started = performance.now();
  // Second launches and "Show in folder" (D-Bus) arrive here: open each in a tab. If we were just started
  // bare (e.g. by D-Bus activation), reuse that untouched home tab instead of stacking a second one.
  // window-scoped: the backend picks which window gets each request (the one used last)
  getCurrentWebviewWindow().listen<{ targets: { loc: string; select: string | null }[]; properties: string[] }>("open", ({ payload }) => {
    closeOffer();
    payload.targets.forEach((t, i) => {
      const fresh = bare && i === 0 && tabs.length === 1 && performance.now() - started < 3000 && pane().loc === HOME && !pane().back.length;
      if (fresh) pane().navigate(t.loc, false, t.select ?? undefined);
      else newTab(t.loc, true, t.select ?? undefined);
    });
    if (payload.properties.length) invoke("open_dialog", { kind: "props", arg: JSON.stringify({ paths: payload.properties }), title: "Properties", width: 500, height: 600 });
  });
  listen<string[]>("fs-change", ({ payload }) => allPanes().forEach((p) => p.onFsChange(payload)));
  for (const t of initial) newTab(t.loc, true, t.select);
  $("app").hidden = false;
  // a plain launch of the first window: offer to bring back what was open when the app last closed
  if (bare && !restored) invoke<WinState[] | null>("session_take").then((ws) => ws && offerRestore(ws));
}
let restored = false;
/** A window opened from a saved session. */
export async function startSession(st: WinState) {
  restored = true;
  await start([]);
  openState(st);
}
