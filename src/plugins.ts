// Plugins (backend: src-tauri/src/plugins.rs) mapped onto the app's hooks: right-click actions, status badges,
// sidebar sections, virtual locations (list / thumbnails / open = download first / drag out), palette search.
// A new plugin asks for consent before it ever runs.
import { listen } from "@tauri-apps/api/event";
import { $, Entry, invoke, esc, ext, parentOf, fmtSize, fmtDate, shown } from "./util";
import { hooks, pane, allPanes, flash, newTab, scheme, dl } from "./app";
import { Pane } from "./pane";
import { Item } from "./menu";
import { startDrag } from "./ops";
import { setupSection } from "./sidebar";
import { modes, openPalette } from "./palette";
import { actions } from "./actions";

type Action = { id: string; label: string; mime: string[]; ext: string[]; roots: boolean; own: boolean; multi: boolean };
type Plugin = { name: string; title: string; description: string; exec: string; settings: { key: string; label: string; secret: boolean }[]; actions: Action[]; badges: boolean; scheme: string; search: boolean; icon: string; consent: boolean | null };
type Init = { roots?: string[]; sidebar?: { title: string; loc: string }[] };
type Badge = { text: string; title: string; state: string };

let all: Plugin[] = [];
const running = new Map<string, { p: Plugin; init: Init }>(); // started, with their init answers

export const call = <T = any>(name: string, method: string, params: object = {}, timeout?: number) => invoke<T>("plugin_call", { name, method, params, timeout });
const url = (name: string, method: string, q: Record<string, string>) => `wcplugin://localhost/${name}/${method}?${new URLSearchParams(q)}`;
const under = (path: string, root: string) => path === root || path.startsWith(root.endsWith("/") ? root : root + "/");
/** The running plugin whose virtual locations these are ("immich:/albums" → immich). */
const owner = (loc: string) => [...running.values()].find((r) => r.p.scheme && scheme(loc) === r.p.scheme + ":");

// ---------- virtual locations ----------
const titles = new Map<string, string>();
/** Plugin entries → full Entries (thumbnail URLs through wcplugin://). */
function entries(name: string, list: any[]): Entry[] {
  return list.map((e) => ({
    link: false, broken: false, special: "", hidden: false, size: 0, mtime: 0, where: "", ...e, dir: !!e.dir,
    thumb: e.thumb ? url(name, "thumb", { path: e.path }) : undefined,
    preview: e.thumb && !e.dir ? url(name, "thumb", { path: e.path, size: "preview" }) : undefined,
  }));
}
/** Local copies of plugin items (downloaded into the plugin's cache), in order. */
export async function fetchFiles(paths: string[]) {
  const byPlugin = paths.map((p) => owner(p));
  if (byPlugin.some((o) => !o)) return paths; // local already
  if (paths.length > 1) flash(`Downloading ${paths.length} items…`);
  return Promise.all(paths.map(async (path, i) => (await call<{ file: string }>(byPlugin[i]!.p.name, "fetch", { path }, 900)).file));
}
async function openItems(es: Entry[]) {
  if (es.length === 1) flash(`Downloading ${shown(es[0].name)}…`);
  try { await invoke("open_default", { paths: await fetchFiles(es.map((e) => e.path)) }); } catch (err) { flash(String(err)); }
}

function register(p: Plugin) {
  const s = p.scheme + ":";
  if (!p.scheme || hooks.virtual![s]) return;
  hooks.virtual![s] = async (loc) => {
    const r = await call<{ entries: any[]; title?: string }>(p.name, "list", { loc }, 120);
    if (r.title) titles.set(loc, r.title);
    return entries(p.name, r.entries);
  };
  hooks.locTitle![s] = (loc) => titles.get(loc) ?? p.title;
  hooks.defaultView[s] = "grid";
  hooks.defaultSort[s] = "rank"; // the plugin's own order (score), e.g. newest first
  hooks.locMenu[s] = (pn, es) => {
    const own = p.actions.filter((a) => a.own && (a.multi || es.length === 1));
    if (!es.length) return [{ label: "Reload", kb: "F5", act: () => pn.reload() }];
    const files = es.filter((e) => !e.dir);
    return [
      { label: "Open", kb: "Enter", act: () => (es.length === 1 && es[0].dir ? pn.navigate(es[0].path) : openItems(files)) },
      ...(es.length === 1 && es[0].dir ? [{ label: "Open in New Tab", act: () => newTab(es[0].path, false) }] : []),
      "-",
      { label: "Copy", kb: "Ctrl+C", off: !files.length, act: () => copyItems(files) },
      { label: "Copy To…", off: !files.length, act: async () => { const paths = await fetchFiles(files.map((e) => e.path)).catch((err) => { flash(String(err)); return []; }); (await import("./shelf")).sendTo(paths, false); } },
      ...(own.length ? ["-" as const, ...own.map((a) => actionItem(p, a, es.map((e) => e.path)))] : []),
    ];
  };
}

async function copyItems(es: Entry[]) {
  try {
    const paths = await fetchFiles(es.map((e) => e.path));
    await invoke("clip_set", { paths, cut: false });
    flash(`Copied ${paths.length} item${paths.length > 1 ? "s" : ""}`);
  } catch (err) { flash(String(err)); }
}

// open / thumbnails / counts / drag for plugin items
const prevOpen = hooks.open;
hooks.open = (e, p) => (owner(e.path) ? openItems([e]) : prevOpen?.(e, p));
const prevThumb = hooks.thumb;
hooks.thumb = (e) => (owner(e.path) ? e.thumb : prevThumb?.(e));
const prevCount = hooks.count;
hooks.count = (e) => (owner(e.path) ? e.count : prevCount?.(e));
const prevDrag = hooks.drag;
hooks.drag = (p, ev) => {
  if (!owner(p.loc)) return prevDrag?.(p, ev);
  const files = p.selected().filter((e) => !e.dir).map((e) => e.path);
  // start downloading now, so they're there by the time the pointer leaves the window
  if (files.length) startDrag(files, ev, fetchFiles(files));
};
// read-only views: file-changing keys do nothing; Ctrl+C downloads and copies; Ctrl+F searches the plugin
hooks.keys.unshift((ev, p) => {
  const o = owner(p.loc); if (!o) return false;
  const k = ev.key, lk = k.toLowerCase(), c = ev.ctrlKey && !ev.altKey;
  if (c && lk === "c") { copyItems(p.selected().filter((e) => !e.dir)); return true; }
  if (c && lk === "f" && o.p.search) { openPalette(`plugin:${o.p.name}`); return true; }
  if (k === "Delete" || k === "F2" || (c && ["x", "v", "d"].includes(lk)) || (ev.ctrlKey && ev.shiftKey && lk === "n")) { flash(`${o.p.title} is read-only here`); return true; }
  return false;
});

hooks.info.unshift((el, picked, p) => {
  const o = owner(p.loc); if (!o) return false;
  if (picked.length !== 1) {
    const n = p.rows.filter((r) => !r.head).length;
    el.innerHTML = `<h2>${esc(hooks.locTitle![scheme(p.loc)](p.loc))}</h2>${dl([["Items", String(n)], ...(picked.length ? [["Selected", String(picked.length)]] as [string, string][] : [])])}<p class="hint">${esc(o.p.title)} (plugin)</p>`;
    return true;
  }
  const e = picked[0];
  const pairs: [string, string][] = [...(e.info ?? [])];
  if (!e.dir && e.size) pairs.push(["Size", fmtSize(e.size)]);
  if (e.mtime) pairs.push(["Date", fmtDate(e.mtime)]);
  el.innerHTML = `<div class="preview">${e.preview ? `<img src="${esc(e.preview)}" alt="" draggable="false">` : ""}</div><h2>${esc(shown(e.name))}</h2>${dl(pairs)}${e.dir ? "" : `<p><button class="pl-open">Open</button></p>`}`;
  el.querySelector<HTMLElement>(".pl-open")?.addEventListener("click", () => openItems([e]));
  return true;
});

// ---------- actions (right-click on local files) ----------
const mimeOk = (globs: string[], mime: string) => !globs.length || globs.some((g) => g === mime || (g.endsWith("/*") && mime.startsWith(g.slice(0, -1))));
function actionItem(p: Plugin, a: Action, paths: string[]): Item {
  return { label: a.label, act: async () => {
    try {
      const r = await call<{ message?: string; copy?: string } | null>(p.name, "action", { id: a.id, paths }, 300);
      if (r?.copy) await invoke("copy_text", { text: r.copy });
      if (r?.message) flash(r.message);
    } catch (err) { flash(String(err)); }
  } };
}
hooks.menuExtra.push((_p, es) => {
  if (!es.length || es.some((e) => e.trashId || owner(e.path) || !e.path.startsWith("/"))) return [];
  const paths = es.map((e) => e.path);
  const out: Item[] = [];
  for (const { p, init } of running.values()) {
    const fits = p.actions.filter((a) => !a.own && (a.multi || es.length === 1)
      && (!a.ext.length || es.every((e) => a.ext.includes(ext(e))))
      && (!a.roots || paths.every((x) => (init.roots ?? []).some((r) => under(x, r)))));
    if (!fits.length) continue;
    out.push({ label: p.title, sub: async () => {
      const mimes = fits.some((a) => a.mime.length) ? await invoke<string[]>("mime_types", { paths }) : [];
      const ok = fits.filter((a) => mimes.every((m) => mimeOk(a.mime, m)));
      return ok.length ? ok.map((a) => actionItem(p, a, paths)) : [{ label: "Nothing for this item", off: true }];
    } });
  }
  return out.length ? [...out, "-"] : [];
});

// ---------- badges (only what's on screen is asked for, in one batch) ----------
const badges = new Map<string, Badge | null>();
const wanted = new Map<string, Set<string>>(); // plugin → paths to ask about
let badgeTimer = 0;
hooks.badge = (e) => {
  if (!e.path.startsWith("/")) return;
  for (const { p, init } of running.values()) {
    if (!p.badges || !(init.roots ?? []).some((r) => under(e.path, r))) continue;
    const k = `${p.name}\0${e.path}`;
    if (badges.has(k)) { const b = badges.get(k); if (b) return b; continue; }
    (wanted.get(p.name) ?? wanted.set(p.name, new Set()).get(p.name)!).add(e.path);
    badgeTimer ||= window.setTimeout(askBadges, 30);
  }
};
async function askBadges() {
  badgeTimer = 0;
  const ask = [...wanted]; wanted.clear();
  await Promise.all(ask.map(async ([name, set]) => {
    const paths = [...set];
    paths.forEach((x) => badges.set(`${name}\0${x}`, null)); // asked: don't ask again until it changes
    const r = await call<Record<string, Badge | null>>(name, "badges", { paths }).catch(() => ({} as Record<string, Badge | null>));
    for (const [x, b] of Object.entries(r)) badges.set(`${name}\0${x}`, b);
  }));
  allPanes().forEach((p) => p.render());
}
function forgetBadges(match: (path: string) => boolean) {
  let any = false;
  for (const k of [...badges.keys()]) if (match(k.slice(k.indexOf("\0") + 1))) { badges.delete(k); any = true; }
  if (any) allPanes().forEach((p) => p.render());
}
listen<string[]>("fs-change", ({ payload }) => { const dirs = new Set(payload); forgetBadges((x) => dirs.has(parentOf(x)) || dirs.has(x)); });
// plugins tell us when something changed: {event: "badges", paths?: [...]} (no paths = everything)
listen<{ plugin: string; event: string; paths?: string[] }>("plugin-event", ({ payload: ev }) => {
  if (ev.event === "badges") { const ps = ev.paths && new Set(ev.paths); forgetBadges((x) => !ps || ps.has(x) || [...ps].some((d) => parentOf(x) === d)); }
  if (ev.event === "reload") allPanes().filter((p) => owner(p.loc)?.p.name === ev.plugin).forEach((p) => p.reload());
  if (ev.event === "message") flash(String((ev as any).text ?? ""));
});

// ---------- palette search ----------
function searchMode(p: Plugin) {
  let all = "";
  modes[`plugin:${p.name}`] = {
    title: p.title,
    placeholder: `Search ${p.title}…`,
    hint: "Enter open · Shift+Enter show all results · Ctrl+Enter results in a new tab",
    async query(q) {
      if (!q.trim()) return { items: [], note: "Type what you're looking for" };
      const r = await call<{ entries: any[]; loc?: string; note?: string }>(p.name, "search", { q }, 60);
      all = r.loc ?? "";
      return { items: entries(p.name, r.entries).map((e) => ({ path: e.path, dir: e.dir, label: e.name, sub: e.where || (e.mtime ? fmtDate(e.mtime) : ""), img: e.thumb, noIcon: !!e.thumb })), note: r.note };
    },
    pick(it, how) {
      if (how !== "open" && all) { how === "tab" ? newTab(all, true, it.path) : pane().navigate(all, true, it.path); return; }
      if (it.dir) pane().navigate(it.path);
      else openItems([{ name: it.label, path: it.path } as Entry]);
    },
  };
  if (!actions.some((a) => a.label === `${p.title}: search…`)) actions.push({ label: `${p.title}: search…`, run: () => openPalette(`plugin:${p.name}`), when: () => running.has(p.name) });
}

// ---------- sidebar ----------
function sidebar() {
  document.querySelectorAll(".sec.plugin-sec").forEach((s) => s.remove());
  for (const { p, init } of running.values()) {
    if (!init.sidebar?.length) continue;
    const sec = document.createElement("section");
    sec.className = "sec plugin-sec";
    sec.dataset.sec = `plugin-${p.name}`;
    sec.innerHTML = `<h3 class="section"></h3><div class="sec-body"><div>${init.sidebar.map((s) => `<div class="drawer small" data-p="${esc(s.loc)}" title="${esc(s.title)}"><span class="label">${esc(s.title)}</span><span class="handle"></span></div>`).join("")}</div></div>`;
    sec.querySelector("h3")!.textContent = p.title;
    $("tools-head").before(sec);
    setupSection(sec, p.icon ? `<path d="${esc(p.icon)}"/>` : '<rect x="4" y="4" width="16" height="16" rx="3"/><path d="M9 9h6v6H9z"/>');
  }
  const cur = pane()?.loc;
  document.querySelectorAll<HTMLElement>(".plugin-sec .drawer").forEach((d) => d.classList.toggle("active", d.dataset.p === cur));
}

// ---------- consent, starting ----------
function ask(p: Plugin) {
  if (document.querySelector(`.toast[data-plugin="${CSS.escape(p.name)}"]`)) return;
  const can = [p.actions.length && "right-click actions", p.badges && "status badges", p.scheme && `its own locations (${p.scheme}:)`, p.search && "search"].filter(Boolean).join(", ");
  const t = document.createElement("div");
  t.className = "toast plugin-ask";
  t.dataset.plugin = p.name;
  t.title = `Runs ~/.local/share/whale-cabinet/plugins/${p.name}/${p.exec} as you, with access to everything you can reach.`;
  t.innerHTML = `<span>Allow the <b>${esc(p.title)}</b> plugin?${p.description ? ` ${esc(p.description)}` : ""} It runs <code>${esc(p.exec)}</code> as you and adds ${esc(can || "nothing visible")}.</span>
    <button data-v="1" class="primary">Allow</button><button data-v="0">Don't allow</button>`;
  // several new plugins stack
  t.style.bottom = `${48 + document.querySelectorAll(".toast.plugin-ask").length * 64}px`;
  document.body.append(t);
  t.addEventListener("click", (e) => {
    const v = (e.target as HTMLElement).dataset.v; if (!v) return;
    t.remove();
    invoke("plugin_consent", { name: p.name, allow: v === "1" }).catch((err) => flash(String(err)));
  });
}

async function start(p: Plugin) {
  register(p);
  if (p.search) searchMode(p);
  try { running.set(p.name, { p, init: (await invoke<Init>("plugin_start", { name: p.name })) ?? {} }); }
  catch (err) { running.delete(p.name); flash(String(err)); }
}

export async function loadPlugins(only?: string) {
  all = await invoke<Plugin[]>("plugins_list").catch(() => []);
  for (const p of all) {
    if (only && p.name !== only) continue;
    running.delete(p.name);
    forgetBadges(() => true);
    document.querySelector(`.toast[data-plugin="${CSS.escape(p.name)}"]`)?.remove();
    if (p.consent === null) ask(p);
    else if (p.consent) await start(p);
  }
  sidebar();
  allPanes().filter((x) => (!only || owner(x.loc)?.p.name === only) && owner(x.loc)).forEach((x) => x.reload());
}
listen<string>("plugins-changed", ({ payload }) => loadPlugins(payload));

const prevNav = hooks.onNavigate;
hooks.onNavigate = (p: Pane) => { prevNav?.(p); document.querySelectorAll<HTMLElement>(".plugin-sec .drawer").forEach((d) => d.classList.toggle("active", d.dataset.p === p.loc)); };

loadPlugins();
