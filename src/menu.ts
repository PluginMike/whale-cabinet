// Context menu (right-click, Menu key, Shift+F10) with Open With, New, clipboard, archives, colour/tags,
// trash and Properties; also the Move/Copy prompt for files dropped in from other apps.
import { Entry, invoke, esc, baseName, assetUrl, TAG_COLORS, tagLabel, tagColor } from "./util";
import { hooks, pane, flash, toggleHidden, host, newTab, scheme } from "./app";
import { Pane } from "./pane";
import * as ops from "./ops";
import { editTags, newTag, manageTags, plainTags, refreshTags } from "./tags";

export type Item = "-" | {
  label: string; icon?: string; dot?: string; kb?: string; danger?: boolean; off?: boolean;
  act?: () => void; sub?: () => Item[] | Promise<Item[]>;
};

const menu = document.getElementById("menu")!;
let hl = -1, subEl: HTMLElement | null = null, subTimer = 0;

function build(items: Item[], el: HTMLElement) {
  el.innerHTML = items.map((it, i) => it === "-" ? "<hr>" :
    `<div class="mi${it.sub ? " sub" : ""}${it.danger ? " danger" : ""}${it.off ? " off" : ""}" data-i="${i}">` +
    (it.icon ? `<img src="${esc(it.icon)}" alt="">` : it.dot ? `<span class="dot" style="background:${esc(it.dot)}"></span>` : `<span class="ico"></span>`) +
    `<span>${esc(it.label)}</span>${it.kb ? `<span class="kb">${esc(it.kb)}</span>` : ""}</div>`).join("");
  el.onmousemove = (ev) => {
    if (el !== menu) return;
    // the submenu lives inside #menu: its rows aren't main-menu rows
    if ((ev.target as HTMLElement).closest(".submenu")) { clearTimeout(subTimer); return; }
    const row = (ev.target as HTMLElement).closest<HTMLElement>(".mi"); if (!row) return;
    const i = +row.dataset.i!, it = items[i] as Exclude<Item, "-">;
    if (i === hl) return;
    setHl(i);
    // wait a moment before switching submenus, so moving diagonally towards an open one doesn't close it
    clearTimeout(subTimer);
    subTimer = window.setTimeout(() => { if (hl !== i) return; if (it.sub) openSub(row, it); else closeSub(); }, subEl ? 220 : 60);
  };
  el.onclick = (ev) => {
    const row = (ev.target as HTMLElement).closest<HTMLElement>(".mi"); if (!row) return;
    const it = items[+row.dataset.i!] as Exclude<Item, "-">;
    if (it.sub) { openSub(row, it); return; }
    closeMenu(); it.act?.();
  };
}
let items: Item[] = [];
function setHl(i: number) {
  hl = i;
  menu.querySelectorAll(":scope > .mi").forEach((m) => m.classList.toggle("hl", +(m as HTMLElement).dataset.i! === i));
}
async function openSub(row: HTMLElement, it: Exclude<Item, "-">) {
  closeSub();
  const el = document.createElement("div");
  el.className = "submenu";
  el.innerHTML = `<div class="mi off"><span class="ico"></span><span>Loading…</span></div>`;
  menu.append(el);
  subEl = el;
  const place = () => {
    const r = row.getBoundingClientRect(), w = el.offsetWidth, h = el.offsetHeight;
    el.style.left = `${r.right + w > innerWidth ? r.left - w : r.right}px`;
    el.style.top = `${Math.max(4, Math.min(r.top - 4, innerHeight - h - 4))}px`;
  };
  place();
  const list = await it.sub!();
  if (subEl !== el) return;
  build(list, el);
  place();
}
function closeSub() { clearTimeout(subTimer); subEl?.remove(); subEl = null; }
export function closeMenu() { menu.hidden = true; closeSub(); menu.innerHTML = ""; pane()?.scroller.focus({ preventScroll: true }); }

export function showMenu(x: number, y: number, list: Item[]) {
  items = list;
  build(list, menu);
  menu.hidden = false;
  hl = -1;
  const w = menu.offsetWidth, h = menu.offsetHeight;
  menu.style.left = `${Math.min(x, innerWidth - w - 4)}px`;
  menu.style.top = `${Math.max(4, Math.min(y, innerHeight - h - 4))}px`;
}
window.addEventListener("mousedown", (ev) => { if (!menu.hidden && !menu.contains(ev.target as Node)) closeMenu(); }, true);
window.addEventListener("blur", () => { if (!menu.hidden) closeMenu(); });
window.addEventListener("keydown", (ev) => {
  if (menu.hidden) return;
  ev.preventDefault(); ev.stopPropagation();
  const live = items.map((it, i) => (it !== "-" && !it.off ? i : -1)).filter((i) => i >= 0);
  const pos = live.indexOf(hl);
  if (ev.key === "Escape") closeMenu();
  else if (ev.key === "ArrowDown") setHl(live[(pos + 1) % live.length]);
  else if (ev.key === "ArrowUp") setHl(live[(pos - 1 + live.length) % live.length]);
  else if (ev.key === "ArrowRight" && hl >= 0) { const it = items[hl] as Exclude<Item, "-">; const row = menu.querySelector<HTMLElement>(`:scope > .mi[data-i="${hl}"]`); if (it.sub && row) openSub(row, it); }
  else if (ev.key === "ArrowLeft") closeSub();
  else if (ev.key === "Enter" && hl >= 0) { const it = items[hl] as Exclude<Item, "-">; if (it.sub) { const row = menu.querySelector<HTMLElement>(`:scope > .mi[data-i="${hl}"]`); if (row) openSub(row, it); } else { closeMenu(); it.act?.(); } }
}, true);

// ---------- building the file menu ----------
const ARCHIVE = /\.(zip|7z|rar|tar|tgz|txz|tar\.(gz|xz|zst|bz2)|gz|xz|bz2|zst)$/i;
export const openProps = (paths: string[]) =>
  invoke("open_dialog", { kind: "props", arg: JSON.stringify({ paths }), title: paths.length === 1 ? `Properties — ${baseName(paths[0])}` : `Properties — ${paths.length} items`, width: 500, height: 600 });
export const openWithDialog = (paths: string[], mime: string) =>
  invoke("open_dialog", { kind: "openwith", arg: JSON.stringify({ paths, mime }), title: "Open With", width: 460, height: 580 });
const terminalAt = (dir: string) => invoke("open_terminal", { dir }).catch((e) => flash(String(e)));

async function openWithItems(paths: string[]): Promise<Item[]> {
  const [mime, apps] = await invoke<[string, { id: string; name: string; icon: string | null; default: boolean }[]]>("apps_for", { path: paths[0] });
  const list: Item[] = apps.map((a) => ({ label: a.default ? `${a.name} (default)` : a.name, icon: a.icon ? assetUrl(a.icon) : undefined, act: () => invoke("launch_app", { id: a.id, paths }).catch((e) => flash(String(e))) }));
  if (list.length) list.push("-");
  list.push({ label: "Other Application…", act: () => openWithDialog(paths, mime) });
  return list;
}

function colorItems(es: Entry[]): Item[] {
  const paths = es.map((e) => e.path);
  const old = es.flatMap((e) => e.tags ?? []).filter((t) => t.startsWith("color:"));
  return [
    ...TAG_COLORS.map((c) => ({ label: tagLabel(`color:${c}`), dot: `var(--t-${c})`, act: () => editTags(paths, [`color:${c}`], []) })),
    "-",
    { label: "Custom…", act: () => { const i = document.createElement("input"); i.type = "color"; i.onchange = () => editTags(paths, [`color:${i.value}`], []); i.click(); } },
    { label: "No Colour", off: !old.length, act: () => editTags(paths, [], old) },
  ];
}
async function tagItems(es: Entry[]): Promise<Item[]> {
  const paths = es.map((e) => e.path);
  await refreshTags();
  const all = (t: string) => es.every((e) => e.tags?.includes(t));
  const list: Item[] = plainTags().map((t) => ({ label: `${all(t) ? "✓ " : ""}${t}`, dot: tagColor(t), act: () => (all(t) ? editTags(paths, [], [t]) : editTags(paths, [t], [])) }));
  if (list.length) list.push("-");
  list.push({ label: "New Tag…", act: () => newTag(paths) });
  list.push({ label: "Manage Tags…", act: () => manageTags() });
  return list;
}

async function compressItems(paths: string[]): Promise<Item[]> {
  const tools = await invoke<Record<string, boolean>>("archive_tools");
  const f = (fmt: string, need: string): Item => ({ label: fmt === "zip" ? "ZIP (.zip)" : fmt === "7z" ? "7-Zip (.7z)" : `.${fmt}`, off: !tools[need], act: () => invoke("op_compress", { items: paths, format: fmt }).catch((e) => flash(String(e))) });
  return [f("zip", "zip"), f("tar.gz", "tar"), f("tar.xz", "tar"), f("tar.zst", "tar"), f("7z", "7z")];
}

export function fileMenu(p: Pane): Item[] {
  const es = p.selected();
  const inTrash = p.loc === "trash:/";
  const dir = p.targetDir();
  const own = hooks.locMenu[scheme(p.loc)];
  if (own) return own(p, es);
  if (inTrash) {
    return [
      { label: "Restore", off: !es.length, kb: "Ctrl+R", act: () => ops.restoreSel(p) },
      { label: "Delete Permanently", danger: true, off: !es.length, kb: "Del", act: () => ops.deleteSel(p) },
      "-",
      { label: "Empty Trash", danger: true, act: () => ops.emptyTrash() },
    ];
  }
  if (!es.length) {
    return [
      { label: "New Folder…", kb: "Ctrl+Shift+N", off: !dir, act: () => ops.makeNew(true, p) },
      { label: "New File…", kb: "Ctrl+Alt+N", off: !dir, act: () => ops.makeNew(false, p) },
      "-",
      { label: "Paste", kb: "Ctrl+V", off: !dir, act: () => ops.paste(dir) },
      "-",
      { label: "Open Terminal Here", kb: "Shift+F4", off: !dir, act: () => terminalAt(dir) },
      { label: "Copy Location", off: !dir, act: () => invoke("copy_text", { text: dir }) },
      "-",
      { label: "Select All", kb: "Ctrl+A", act: () => p.selectAll() },
      { label: host.showHidden ? "Hide Hidden Files" : "Show Hidden Files", kb: "Ctrl+H", act: () => toggleHidden() },
      "-",
      { label: "Properties", kb: "Alt+Enter", off: !dir, act: () => openProps([dir]) },
    ];
  }
  const paths = es.map((e) => e.path);
  const one = es.length === 1 ? es[0] : null;
  const dirs = es.filter((e) => e.dir);
  const files = es.filter((e) => !e.dir);
  const archives = files.filter((e) => ARCHIVE.test(e.name));
  const list: Item[] = [
    { label: "Open", kb: "Enter", act: () => (one?.dir ? p.navigate(one.path) : invoke("open_default", { paths: files.map((e) => e.path) }).catch((e) => flash(String(e)))) },
  ];
  if ((files.length && !dirs.length) || one) list.push({ label: "Open With", sub: () => openWithItems(paths) });
  if (dirs.length) list.push({ label: dirs.length > 1 ? "Open in New Tabs" : "Open in New Tab", act: () => dirs.forEach((d) => newTab(d.path, false)) });
  if (one?.dir) list.push({ label: "Add to Places", act: () => hooks.addPlace?.(one.path) });
  list.push({ label: "Open Terminal Here", kb: "Shift+F4", act: () => terminalAt(one?.dir ? one.path : dir || "/") });
  list.push("-",
    { label: "Cut", kb: "Ctrl+X", act: () => ops.copy(true) },
    { label: "Copy", kb: "Ctrl+C", act: () => ops.copy(false) },
    { label: one?.dir ? "Paste Into Folder" : "Paste", kb: "Ctrl+V", off: !one?.dir && !dir, act: () => ops.paste(one?.dir ? one.path : dir) },
    { label: "Rename…", kb: "F2", off: es.length > 1, act: () => ops.rename(p) },
    { label: "Duplicate", kb: "Ctrl+D", act: () => ops.duplicate(p) },
    { label: paths.length > 1 ? "Copy Paths" : "Copy Path", act: () => invoke("copy_text", { text: paths.join("\n") }) },
    "-",
    { label: "Compress", sub: () => compressItems(paths) },
  );
  if (archives.length && archives.length === files.length && !dirs.length) list.push({ label: "Extract Here", act: () => invoke("op_extract", { items: archives.map((e) => e.path) }) });
  if (p.loc === "recent:/") list.push("-", { label: "Remove from Recent", act: () => invoke("recent_remove", { paths }).catch((e) => flash(String(e))) });
  list.push("-",
    { label: "Colour", sub: () => colorItems(es) },
    { label: "Tags", sub: () => tagItems(es) },
    "-",
    ...hooks.menuExtra.flatMap((f) => f(p, es)),
    { label: "Move to Trash", kb: "Del", act: () => ops.trashSel(p) },
    { label: "Delete", kb: "Shift+Del", danger: true, act: () => ops.deleteSel(p) },
    "-",
    { label: "Properties", kb: "Alt+Enter", act: () => openProps(paths) },
  );
  return list;
}

hooks.menu = (p, ev) => showMenu(ev.clientX, ev.clientY, fileMenu(p));
hooks.dropMenu = (x, y, pick) => showMenu(x, y, [
  { label: "Move Here", act: () => pick(false) },
  { label: "Copy Here", act: () => pick(true) },
  "-",
  { label: "Cancel", act: () => {} },
]);
hooks.open = (e) => invoke("open_default", { paths: [e.path] }).catch((err) => flash(String(err)));

function menuAtFocus(p: Pane) {
  const i = p.index.get(p.focus);
  const el = i === undefined ? null : p.items.querySelector<HTMLElement>(`.item[data-i="${i}"]`);
  const r = (el ?? p.scroller).getBoundingClientRect();
  showMenu(r.left + 40, el ? r.bottom : r.top + 40, fileMenu(p));
}
hooks.keys.push((ev, p) => {
  if (ev.key === "ContextMenu" || (ev.shiftKey && ev.key === "F10")) { menuAtFocus(p); return true; }
  if (ev.altKey && ev.key === "Enter") { const s = p.selected().map((e) => e.path); openProps(s.length ? s : p.targetDir() ? [p.targetDir()] : []); return true; }
  if (ev.shiftKey && ev.key === "F4") { terminalAt(p.targetDir() || "/"); return true; }
  return false;
});
