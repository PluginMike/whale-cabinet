// File operations UI: clipboard, paste, rename, new, duplicate, trash/delete, undo, progress panel,
// conflicts (separate dialog window), Trash view, drag & drop (internal, out to other apps, in from them).
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { startDrag as nativeDrag } from "@crabnebula/tauri-plugin-drag";
import { $, Entry, invoke, esc, fmtSize, baseName, parentOf, joinPath, shown } from "./util";
import { hooks, pane, allPanes, flash, HOME, host, tab } from "./app";
import { Pane, isFolder } from "./pane";

type Undo = { kind: "move"; pairs: [string, string][] } | { kind: "trash"; paths: string[] } | { kind: "restore"; paths: string[]; since: number };
type Progress = { bytes_done: number; bytes_total: number; items_done: number; items_total: number; current: string };
type OpEvent = { id: number; title: string; state: "progress" | "conflict" | "done"; progress?: Progress; conflict?: any; errors: string[]; undo?: Undo; cancelled: boolean };

let lastUndo: Undo | null = null;
const selectAfter = new Map<number, Pane>(); // job id → pane whose results to select

// ---------- progress panel ----------
const jobs = new Map<number, HTMLElement>();
const panel = $("progress");
function jobCard(id: number, title: string) {
  let el = jobs.get(id);
  if (!el) {
    el = document.createElement("div");
    el.className = "job";
    el.innerHTML = `<div class="jt"><span></span><button>Cancel</button></div><div class="bar indet"><i></i></div><div class="sub"></div>`;
    el.querySelector("button")!.onclick = () => invoke("op_cancel", { id });
    jobs.set(id, el);
    // only show jobs that take a moment
    setTimeout(() => { if (jobs.has(id)) { panel.append(el!); panel.hidden = false; } }, 350);
  }
  el.querySelector(".jt span")!.textContent = title;
  return el;
}
function dropCard(id: number, delay = 0) {
  const el = jobs.get(id);
  setTimeout(() => { el?.remove(); jobs.delete(id); panel.hidden = !panel.children.length; }, delay);
}

// only this window's jobs: the backend sends each job's events to the window that started it
getCurrentWebviewWindow().listen<OpEvent>("op", ({ payload: ev }) => {
  const el = jobCard(ev.id, ev.title);
  if (ev.state === "progress" && ev.progress) {
    const p = ev.progress;
    const frac = p.bytes_total ? p.bytes_done / p.bytes_total : p.items_total ? p.items_done / p.items_total : 0;
    const bar = el.querySelector(".bar")!;
    bar.classList.toggle("indet", !p.bytes_total && !p.items_total);
    (bar.firstElementChild as HTMLElement).style.width = `${Math.round(frac * 100)}%`;
    el.querySelector(".sub")!.textContent = [p.items_total ? `${p.items_done}/${p.items_total}` : "", p.bytes_total ? `${fmtSize(p.bytes_done)} of ${fmtSize(p.bytes_total)}` : "", shown(baseName(p.current))].filter(Boolean).join(" · ");
  } else if (ev.state === "conflict") {
    el.querySelector(".sub")!.textContent = `Waiting: ${baseName(ev.conflict.dst)} already exists`;
    invoke("open_dialog", { kind: "conflict", arg: JSON.stringify({ id: ev.id, title: ev.title, c: ev.conflict }), title: "File already exists", width: 560, height: 360 });
  } else if (ev.state === "done") {
    if (ev.undo) lastUndo = ev.undo;
    const p = selectAfter.get(ev.id); selectAfter.delete(ev.id);
    if (p && ev.undo) {
      const made = ev.undo.kind === "trash" ? ev.undo.paths : ev.undo.kind === "move" ? ev.undo.pairs.map((x) => x[0]) : [];
      const here = made.filter((m) => parentOf(m) === p.loc);
      if (here.length) p.load(p.loc).then(() => p.selectPaths(here));
    }
    if (ev.errors.length) {
      el.classList.add("err");
      el.querySelector(".jt button")!.textContent = "Dismiss";
      (el.querySelector(".jt button") as HTMLButtonElement).onclick = () => dropCard(ev.id);
      el.querySelector(".sub")!.textContent = `${ev.errors.length} problem${ev.errors.length > 1 ? "s" : ""}: ${ev.errors.slice(0, 3).join(" · ")}`;
      (el.querySelector(".sub") as HTMLElement).title = ev.errors.join("\n");
      panel.append(el); panel.hidden = false;
      flash(ev.errors[0]);
    } else {
      dropCard(ev.id, ev.cancelled ? 0 : 600);
      if (ev.cancelled) flash("Cancelled");
    }
    allPanes().filter((x) => x.loc === "trash:/").forEach((x) => x.reload());
  }
});

// ---------- actions ----------
const targetDir = (p = pane()) => p.targetDir();
const selPaths = (p = pane()) => p.selected().filter((e) => !e.trashId).map((e) => e.path);

export async function copy(cut: boolean) {
  const paths = selPaths(); if (!paths.length) return;
  await invoke("clip_set", { paths, cut }).catch((e) => flash(String(e)));
  for (const p of allPanes()) { p.cutSet = cut ? new Set(paths) : undefined; p.render(); }
  flash(`${cut ? "Cut" : "Copied"} ${paths.length} item${paths.length > 1 ? "s" : ""}`);
}
export async function paste(into = targetDir()) {
  if (!into) return;
  const { paths, cut } = await invoke<{ paths: string[]; cut: boolean }>("clip_get");
  if (!paths.length) { flash("Nothing to paste"); return; }
  transfer(paths, into, cut);
  if (cut) { await invoke("clip_set", { paths: [], cut: false }); for (const p of allPanes()) { p.cutSet = undefined; p.render(); } }
}
export async function transfer(sources: string[], dest: string, mv: boolean, names?: string[]) {
  const id = await invoke<number>("op_transfer", { sources, dest, mv, names: names ?? null });
  const p = allPanes().find((x) => x.loc === dest); if (p) selectAfter.set(id, p);
}
export async function trashSel(p = pane()) {
  const paths = selPaths(p); if (!paths.length) return;
  if (p.loc === "trash:/") return purgeSel(p);
  invoke("op_trash", { items: paths });
}
export async function deleteSel(p = pane()) {
  if (p.loc === "trash:/") return purgeSel(p);
  const paths = selPaths(p); if (!paths.length) return;
  const what = paths.length === 1 ? `"${shown(baseName(paths[0]))}"` : `${paths.length} items`;
  if (await confirmBox(`Permanently delete ${what}? This can't be undone.`, "Delete", true)) invoke("op_delete", { items: paths });
}
export async function rename(p = pane()) {
  const e = p.entry(p.focus) ?? p.selected()[0];
  if (!e || e.trashId) return;
  if (p.sel.size > 1) { flash("Rename one item at a time"); return; }
  const name = await p.editName(e.path);
  if (!name) return;
  try {
    const [to, undo] = await invoke<[string, Undo]>("rename_item", { path: e.path, name });
    lastUndo = undo;
    await p.load(parentOf(to));
    p.selectPaths([to]);
  } catch (err) { flash(String(err)); }
}
export async function makeNew(folder: boolean, p = pane()) {
  const dir = targetDir(p); if (!dir) return;
  const name = await invoke<string>("unique_name", { dir, name: folder ? "New Folder" : "New File" });
  try {
    const path = await invoke<string>("make_item", { dir, name, folder });
    lastUndo = { kind: "trash", paths: [path] };
    await p.load(dir);
    p.selectPaths([path]);
    const n = await p.editName(path);
    if (n) { const [to, undo] = await invoke<[string, Undo]>("rename_item", { path, name: n }).catch((e) => { flash(String(e)); return [path, null] as const; }); if (undo) lastUndo = { kind: "trash", paths: [to] }; await p.load(dir); p.selectPaths([to]); }
  } catch (err) { flash(String(err)); }
}
export async function duplicate(p = pane()) {
  const paths = selPaths(p); if (!paths.length) return;
  const dir = parentOf(paths[0]);
  const names = await Promise.all(paths.map((x) => { const n = baseName(x), i = n.lastIndexOf("."); const copy = i > 0 ? `${n.slice(0, i)} (copy)${n.slice(i)}` : `${n} (copy)`; return invoke<string>("unique_name", { dir, name: copy }); }));
  transfer(paths, dir, false, names);
}
export function undo() {
  if (!lastUndo) { flash("Nothing to undo"); return; }
  invoke("op_undo", { undo: lastUndo });
  lastUndo = null;
}

// ---------- trash view ----------
async function purgeSel(p: Pane) {
  const ids = p.selected().map((e) => e.trashId!).filter(Boolean); if (!ids.length) return;
  if (await confirmBox(`Permanently delete ${ids.length} item${ids.length > 1 ? "s" : ""} from the trash?`, "Delete", true)) { await invoke("trash_purge", { ids }).catch((e) => flash(String(e))); p.reload(); }
}
export async function restoreSel(p = pane()) {
  const ids = p.selected().map((e) => e.trashId!).filter(Boolean); if (!ids.length) return;
  await invoke("trash_restore", { ids }).catch((e) => flash(String(e)));
  p.reload();
}
export async function emptyTrash() {
  if (await confirmBox("Empty the trash? Everything in it is deleted permanently.", "Empty Trash", true)) { await invoke("trash_purge", { ids: null }).catch((e) => flash(String(e))); allPanes().filter((x) => x.loc === "trash:/").forEach((x) => x.reload()); }
}
type TrashEntry = { id: string; name: string; orig_path: string; deleted: number; dir: boolean; size: number };
hooks.virtual!["trash:"] = async () => (await invoke<TrashEntry[]>("trash_list")).map((t) => ({
  name: t.name, path: `trash:${t.id}`, dir: t.dir, link: false, broken: false, special: "", hidden: false,
  size: t.dir ? 0 : t.size, mtime: t.deleted, deleted: t.deleted, trashId: t.id, origPath: t.orig_path,
}));
hooks.locTitle!["trash:"] = () => "Trash";
const trashFiles = () => `${HOME}/.local/share/Trash/files`;
listen<string[]>("fs-change", ({ payload }) => { if (payload.some((d) => d.startsWith(trashFiles()))) allPanes().filter((p) => p.loc === "trash:/").forEach((p) => p.reload()); });
const origSync = host.syncWatch;
host.syncWatch = () => {
  if (allPanes().some((p) => p.loc === "trash:/")) invoke("watch", { paths: [...new Set([...allPanes().flatMap((p) => p.watched()), trashFiles()])] });
  else origSync();
};
const origOpen = host.open;
host.open = (e, p) => { if (e.trashId) { flash(`Restore it first (it was ${e.origPath})`); return; } origOpen(e, p); };

hooks.info.push((el, picked, p) => {
  if (p.loc !== "trash:/") return false;
  if (picked.length === 1 && picked[0].trashId) {
    const e = picked[0];
    el.innerHTML = `<h2>${esc(shown(e.name))}</h2><dl><dt>Was in</dt><dd>${esc(parentOf(e.origPath!))}</dd><dt>Deleted</dt><dd>${esc(new Date(e.deleted!).toLocaleString())}</dd><dt>${e.dir ? "Items" : "Size"}</dt><dd>${e.dir ? "" : fmtSize(e.size)}</dd></dl><p><button id="tr-restore">Restore</button> <button id="tr-del">Delete permanently</button></p>`;
  } else {
    el.innerHTML = `<h2>Trash</h2><dl><dt>Items</dt><dd>${p.rows.length}</dd></dl><p>${picked.length ? `<button id="tr-restore">Restore ${picked.length}</button> ` : ""}<button id="tr-empty">Empty Trash</button></p><p class="hint">Ctrl+R restores the selection. Del deletes it permanently.</p>`;
  }
  el.querySelector<HTMLElement>("#tr-restore")?.addEventListener("click", () => restoreSel(p));
  el.querySelector<HTMLElement>("#tr-del")?.addEventListener("click", () => purgeSel(p));
  el.querySelector<HTMLElement>("#tr-empty")?.addEventListener("click", () => emptyTrash());
  return true;
});

// ---------- in-app confirm ----------
export function confirmBox(msg: string, ok: string, danger = false): Promise<boolean> {
  return new Promise((resolve) => {
    const m = document.createElement("div");
    m.className = "modal";
    m.innerHTML = `<div class="modal-box"><p>${esc(msg)}</p><div class="btns"><button data-v="0">Cancel</button><button data-v="1" class="${danger ? "danger" : "primary"}">${esc(ok)}</button></div></div>`;
    document.body.append(m);
    const done = (v: boolean) => { m.remove(); window.removeEventListener("keydown", key, true); pane().scroller.focus(); resolve(v); };
    const key = (e: KeyboardEvent) => { e.stopPropagation(); if (e.key === "Escape") done(false); if (e.key === "Enter") done(true); };
    window.addEventListener("keydown", key, true);
    m.addEventListener("click", (e) => { const v = (e.target as HTMLElement).dataset.v; if (v) done(v === "1"); });
    (m.querySelector("[data-v='1']") as HTMLElement).focus();
  });
}

// ---------- keys ----------
hooks.keys.push((ev, p) => {
  const k = ev.key, lk = k.toLowerCase(), c = ev.ctrlKey && !ev.altKey;
  if (c && !ev.shiftKey && lk === "c") { copy(false); return true; }
  if (c && !ev.shiftKey && lk === "x") { copy(true); return true; }
  if (c && !ev.shiftKey && lk === "v") { paste(); return true; }
  if (c && !ev.shiftKey && lk === "z") { undo(); return true; }
  if (c && !ev.shiftKey && lk === "d") { duplicate(); return true; }
  if (c && ev.shiftKey && lk === "n") { makeNew(true); return true; }
  if (ev.ctrlKey && ev.altKey && lk === "n") { makeNew(false); return true; }
  if (k === "F10") { makeNew(true); return true; }
  if (k === "F2") { rename(p); return true; }
  if (k === "Delete" && ev.shiftKey) { deleteSel(p); return true; }
  if (k === "Delete") { trashSel(p); return true; }
  if (c && lk === "r" && p.loc === "trash:/") { restoreSel(p); return true; }
  return false;
});

// ---------- drag & drop ----------
type Target = { el: HTMLElement; kind: "dir" | "trash" | "tag" | "place"; path: string };
function targetAt(x: number, y: number): Target | null {
  const el = document.elementFromPoint(x, y) as HTMLElement | null;
  if (!el) return null;
  const add = el.closest<HTMLElement>(".sb-add");
  if (add) return { el: add, kind: "place", path: "" };
  const tag = el.closest<HTMLElement>(".tagrow[data-tag]");
  if (tag) return { el: tag, kind: "tag", path: tag.dataset.tag! };
  const drawer = el.closest<HTMLElement>(".drawer[data-p]");
  if (drawer) return drawer.dataset.p === "trash:/" ? { el: drawer, kind: "trash", path: "" } : isFolder(drawer.dataset.p!) ? { el: drawer, kind: "dir", path: drawer.dataset.p! } : null;
  const view = allPanes().find((p) => p.el.contains(el));
  if (!view) return null;
  const item = el.closest<HTMLElement>(".item");
  if (item) { const e = view.rows[+item.dataset.i!]?.e; if (e?.dir && !e.trashId) return { el: item, kind: "dir", path: e.path }; }
  if (view.loc === "trash:/") return { el: view.scroller, kind: "trash", path: "" };
  return isFolder(view.loc) ? { el: view.scroller, kind: "dir", path: view.loc } : null;
}
let over: Target | null = null;
function hover(t: Target | null) {
  if (over?.el !== t?.el) { over?.el.classList.remove("dragover"); t?.el.classList.add("dragover"); }
  over = t;
}
async function dropOn(t: Target, paths: string[], copy: boolean) {
  if (t.kind === "trash") { invoke("op_trash", { items: paths }); return; }
  if (t.kind === "tag") { hooks.tagDrop?.(t.path, paths); return; }
  if (t.kind === "place") { const dirs = allPanes().flatMap((p) => p.selected()).filter((e) => e.dir && paths.includes(e.path)).map((e) => e.path); (dirs.length ? dirs : paths).forEach((d) => hooks.addPlace?.(d)); return; }
  if (paths.some((p) => t.path === p || t.path.startsWith(p + "/"))) { flash("Can't drop a folder into itself"); return; }
  if (!copy && paths.every((p) => parentOf(p) === t.path)) return; // already there
  transfer(paths, t.path, !copy);
}

hooks.drag = (p, ev) => {
  const paths = selPaths(p); if (!paths.length) return;
  const ghost = document.createElement("div");
  ghost.className = "drag-ghost";
  ghost.textContent = paths.length === 1 ? shown(baseName(paths[0])) : `${paths.length} items`;
  document.body.append(ghost);
  let gone = false;
  const place = (m: MouseEvent) => { ghost.style.transform = `translate(${m.clientX + 14}px, ${m.clientY + 10}px)`; };
  place(ev);
  const end = () => { gone = true; ghost.remove(); hover(null); window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
  const move = (m: MouseEvent) => {
    if (gone) return;
    place(m);
    // Left the window with the button held: hand over to a native drag so other apps can take the files.
    if (m.clientX <= 0 || m.clientY <= 0 || m.clientX >= innerWidth - 1 || m.clientY >= innerHeight - 1) {
      end();
      invoke<string>("drag_icon").then((icon) => nativeDrag({ item: paths, icon })).catch((e) => flash(String(e)));
      return;
    }
    const t = targetAt(m.clientX, m.clientY);
    hover(t && !(t.kind === "dir" && paths.includes(t.path)) ? t : null);
    ghost.dataset.op = m.ctrlKey ? "copy" : "move";
  };
  const up = (m: MouseEvent) => { const t = over; end(); if (t) dropOn(t, paths, m.ctrlKey); };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
};

// Files dropped in from other apps: ask Move / Copy like Dolphin does.
getCurrentWebview().onDragDropEvent((e) => {
  const d = e.payload;
  if (d.type === "leave") { hover(null); return; }
  const pos = "position" in d ? d.position : null;
  const t = pos ? targetAt(pos.x / devicePixelRatio, pos.y / devicePixelRatio) : null;
  if (d.type === "over" || d.type === "enter") { hover(t); return; }
  if (d.type === "drop") {
    hover(null);
    if (!t || !d.paths.length) return;
    if (t.kind !== "dir") { dropOn(t, d.paths, false); return; }
    hooks.dropMenu ? hooks.dropMenu(pos!.x / devicePixelRatio, pos!.y / devicePixelRatio, (copy) => dropOn(t, d.paths, copy)) : dropOn(t, d.paths, true);
  }
});

export { tab };
