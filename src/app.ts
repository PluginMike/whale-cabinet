// App shell: tabs (each with one or two panes), top bar, sidebar, info panel, status bar, keyboard routing.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
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
  info: ((el: HTMLElement, picked: Entry[], p: Pane) => boolean)[];
  tagDrop?: (tag: string, paths: string[]) => void;
  dropMenu?: (x: number, y: number, pick: (copy: boolean) => void) => void;
  locTitle?: Record<string, (loc: string) => string>;
  open?: (e: Entry, p: Pane) => void;
  onNavigate?: (p: Pane) => void;
  keys: ((ev: KeyboardEvent, p: Pane) => boolean)[];
} = { keys: [], info: [], virtual: {}, locTitle: {} };

const scheme = (loc: string) => loc.slice(0, loc.indexOf(":") + 1);

export function flash(msg: string) {
  const el = $("st-sel");
  el.textContent = msg;
  el.classList.add("flash");
  setTimeout(() => el.classList.remove("flash"), 4000);
}

export const host: Host = {
  showHidden: false,
  singleClick: false,
  activate(p) {
    const t = tab(), i = t.panes.indexOf(p);
    if (i >= 0 && i !== t.active) { t.active = i; showTab(); }
  },
  changed(p) { if (!tab()) return; if (p === pane()) chrome(); renderTabbar(); },
  open(e, p) {
    if (e.dir) p.navigate(e.path);
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
  syncWatch() { invoke("watch", { paths: [...new Set(tabs.flatMap((t) => t.panes.flatMap((p) => p.watched())))] }); },
  flash,
};

function makePane(loc: string, select?: string) {
  const p = new Pane(host);
  p.view = (settings.view as View) || "cabinet";
  p.zoom = +settings.zoom || 1;
  p.el.dataset.view = p.view;
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

function renderTabbar() {
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
  document.title = `${locTitle(p.loc)} — Whale Cabinet`;
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
  const top = p.rows.filter((r) => r.depth === 0);
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
  const el = $("info");
  if (el.hidden) return;
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
  } else {
    const n = p.rows.filter((r) => r.depth === 0).length;
    const pairs: [string, string][] = [["Items", String(n)], ["Location", p.loc]];
    if (p.space) pairs.push(["Free", `${fmtSize(p.space[0])} of ${fmtSize(p.space[1])}`]);
    el.innerHTML = `<div class="glyph">${glyph()}</div><h2>${esc(locTitle(p.loc))}</h2>${dl(pairs)}`;
  }
}

// ---------- location bar ----------
const crumbs = $("crumbs"), pathedit = $<HTMLInputElement>("pathedit"), filterEl = $<HTMLInputElement>("filter");
export function editPath() {
  crumbs.hidden = true; pathedit.hidden = false;
  pathedit.value = isFolder(pane().loc) ? pane().loc : "";
  pathedit.focus(); pathedit.select();
}
function endEdit() { pathedit.hidden = true; crumbs.hidden = false; }
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
$("sort").onchange = () => { const p = pane(); p.sortKey = ($("sort") as HTMLSelectElement).value; p.rebuild(); };
$("sortdir").onclick = () => { const p = pane(); p.asc = !p.asc; p.rebuild(); };
$("viewmode").onchange = () => pane().setView(($("viewmode") as HTMLSelectElement).value as View);

export function toggleHidden(v = !host.showHidden) {
  host.showHidden = v;
  $("hidden").classList.toggle("on", v);
  for (const t of tabs) for (const p of t.panes) p.rebuild();
}
export const openSettings = () => invoke("open_dialog", { kind: "settings", arg: "", title: "Whale Cabinet Settings", width: 460, height: 560 });
export const allPanes = () => tabs.flatMap((t) => t.panes);

// ---------- sidebar: places ----------
export type PlaceItem = { name: string; path: string };
export const drawerHtml = (p: PlaceItem, extra = "") =>
  `<div class="drawer" data-p="${esc(p.path)}" title="${esc(p.path)}"><span class="label">${esc(p.name)}</span><span class="handle"></span>${extra}</div>`;
export function renderPlaces(list: PlaceItem[]) {
  $("places").innerHTML = list.map((p) => drawerHtml(p)).join("") + drawerHtml({ name: "Trash", path: "trash:/" });
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
  if (c && ["1", "2", "3"].includes(k)) { stop(); p.setView((["cabinet", "compact", "grid"] as View[])[+k - 1]); return; }
  if (c && (k === "+" || k === "=")) { stop(); p.setZoom(p.zoom * 1.1); return; }
  if (c && k === "-") { stop(); p.setZoom(p.zoom / 1.1); return; }
  if (c && k === "0") { stop(); p.setZoom(1); return; }
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
  if (k === "Escape") { if (p.filter) { p.filter = ""; p.rebuild(); } else { p.sel.clear(); p.refreshSel(); } return; }
  if (c && lk === "a") { stop(); p.selectAll(); return; }
  if (ev.ctrlKey && ev.shiftKey && lk === "a") { stop(); p.invertSel(); return; }
  // Type-to-filter: printable keys go to the filter box.
  if (k.length === 1 && !ev.ctrlKey && !ev.altKey && !ev.metaKey && k !== " ") filterEl.focus();
});
window.addEventListener("mouseup", (ev) => { if (ev.button === 3) pane().goBack(); if (ev.button === 4) pane().goFwd(); });

// ---------- titlebar ----------
const win = getCurrentWindow();
$("tb-min").onclick = () => win.minimize();
$("tb-max").onclick = () => win.toggleMaximize();
$("tb-close").onclick = () => win.close();

export function applySettings(s: Settings) {
  const first = !Object.keys(settings).length;
  settings = s;
  host.singleClick = !!s.singleClick;
  $("titlebar").hidden = !s.titlebar;
  if (first) toggleHidden(!!s.showHidden);
}

export async function start(initial: { loc: string; select?: string }[]) {
  HOME = await invoke<string>("resolve_path", { input: "~", cwd: "/" });
  listen<string[]>("fs-change", ({ payload }) => allPanes().forEach((p) => p.onFsChange(payload)));
  for (const t of initial) newTab(t.loc, true, t.select);
  $("app").hidden = false;
  invoke<PlaceItem[]>("places").then(renderPlaces);
}
