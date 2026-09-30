// Shelf: a tray under the view where you collect files from anywhere (any folder, any window, other apps), then
// move or copy the whole pile somewhere in one go. Also Copy To… / Move To…, which pick the destination with the
// jump box (zoxide frecency + every folder under home, fuzzy).
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { $, Entry, invoke, esc, shown, baseName, parentOf } from "./util";
import { hooks, pane, flash } from "./app";
import { modes, openPalette, termHits } from "./palette";
import { transfer, startDrag } from "./ops";
import { tilde } from "./nav";

// ---------- destination picker ----------
let onDest: ((dir: string) => void) | null = null;
modes.dest = {
  title: "To",
  placeholder: "Folder to send them to…",
  hint: "Enter pick · Esc cancel",
  async query(q) {
    const [z, f] = await Promise.all([
      invoke<[number, string][]>("zoxide_query", { q, limit: 15 }).catch(() => [] as [number, string][]),
      q.trim() ? invoke<{ items: { path: string; rel: string; idx: number[] }[] }>("fuzzy_find", { q, limit: 40, kind: "d" }).then((r) => r.items).catch(() => []) : Promise.resolve([]),
    ]);
    const seen = new Set<string>();
    const items = [];
    for (const [, path] of z) if (!seen.has(path)) { seen.add(path); const label = tilde(path); items.push({ path, dir: true, label, hl: termHits(label, q), sub: "frequent" }); }
    for (const h of f) if (!seen.has(h.path)) { seen.add(h.path); items.push({ path: h.path, dir: true, label: `~/${h.rel.replace(/\/$/, "")}`, hl: h.idx.map((i) => i + 2) }); }
    return { items };
  },
  pick(it) { const f = onDest; onDest = null; f?.(it.path); },
};
/** Ask for a destination folder, then send `paths` there. */
export function sendTo(paths: string[], move: boolean, after?: () => void) {
  if (!paths.length) return;
  onDest = (dir) => {
    if (paths.some((p) => dir === p || dir.startsWith(p + "/"))) { flash("Can't put a folder inside itself"); return; }
    transfer(paths, dir, move);
    after?.();
  };
  modes.dest.title = move ? "Move to" : "Copy to";
  openPalette("dest"); // a cancelled pick just leaves onDest to be replaced next time
}

// ---------- shelf ----------
let items: string[] = [];
const picked = new Set<string>();
let entries = new Map<string, Entry>();

const shelf = document.createElement("div");
shelf.id = "shelf";
shelf.innerHTML = `<div class="sh-head"><b>Shelf</b><span class="sh-count"></span>
  <span class="sh-btns"><button data-a="move-here" title="Move them into the folder you're in">Move here</button><button data-a="copy-here" title="Copy them into the folder you're in">Copy here</button>
  <button data-a="move-to">Move to…</button><button data-a="copy-to">Copy to…</button><button data-a="clear" title="Take them off the shelf (the files stay)">Clear</button></span></div>
  <div class="sh-items"></div><div class="sh-drop">Drop files here to collect them on the shelf</div>`;
$("center").insertBefore(shelf, $("termpanel"));
const list = shelf.querySelector<HTMLElement>(".sh-items")!;

/** The selected shelf items, or all of them. */
const chosen = () => (picked.size ? items.filter((p) => picked.has(p)) : items.slice());

function render(paths: string[]) {
  items = paths;
  for (const p of [...picked]) if (!items.includes(p)) picked.delete(p);
  shelf.classList.toggle("empty", !items.length);
  shelf.querySelector(".sh-count")!.textContent = items.length ? `${items.length} item${items.length === 1 ? "" : "s"}${picked.size ? ` · ${picked.size} selected` : ""}` : "";
  list.innerHTML = items.map((p) => {
    const e = entries.get(p);
    const thumb = e && !e.dir ? hooks.thumb?.(e) : undefined;
    const art = e?.dir ? `<i class="sh-fold"></i>` : thumb ? `<img src="${thumb}" alt="" draggable="false">` : `<i class="sh-paper"></i>`;
    return `<div class="sh-item${picked.has(p) ? " on" : ""}" data-p="${esc(p)}" title="${esc(p)}">${art}<span class="sh-name">${esc(shown(baseName(p)))}</span><span class="sh-where">${esc(tilde(parentOf(p)))}</span><b class="sh-x" title="Take off the shelf">✕</b></div>`;
  }).join("");
}
async function refresh(paths?: string[]) {
  const ps = paths ?? (await invoke<string[]>("shelf_get"));
  // stat what we haven't seen, for icons/thumbnails
  const missing = ps.filter((p) => !entries.has(p));
  if (missing.length) {
    const dirs = [...new Set(missing.map(parentOf))];
    for (const d of dirs) {
      const l = await invoke<Entry[]>("list_dir", { path: d }).catch(() => [] as Entry[]);
      for (const e of l) if (missing.includes(e.path)) entries.set(e.path, e);
    }
  }
  entries = new Map([...entries].filter(([k]) => ps.includes(k)));
  render(ps);
}

listen<string[]>("shelf", ({ payload }) => refresh(payload));
// after moves/trashes, drop what's gone
getCurrentWebviewWindow().listen<{ state: string }>("op", ({ payload }) => { if (payload.state === "done" && items.length) refresh(); });

shelf.addEventListener("click", (ev) => {
  const t = ev.target as HTMLElement;
  const a = t.closest<HTMLElement>("[data-a]")?.dataset.a;
  const it = t.closest<HTMLElement>(".sh-item");
  if (it && t.classList.contains("sh-x")) { invoke("shelf_remove", { paths: [it.dataset.p] }); return; }
  if (it) {
    const p = it.dataset.p!;
    if (!ev.ctrlKey && !ev.shiftKey) { const only = picked.size === 1 && picked.has(p); picked.clear(); if (!only) picked.add(p); }
    else picked.has(p) ? picked.delete(p) : picked.add(p);
    render(items);
    return;
  }
  if (!a) return;
  const ps = chosen(), here = pane().targetDir();
  const done = () => { picked.clear(); };
  if (a === "clear") { invoke("shelf_remove", { paths: picked.size ? ps : null }); done(); }
  else if (a === "move-here" || a === "copy-here") {
    if (!here) { flash("Open a folder first"); return; }
    transfer(ps, here, a === "move-here"); done();
  }
  else if (a === "move-to") sendTo(ps, true, done);
  else if (a === "copy-to") sendTo(ps, false, done);
});
shelf.addEventListener("dblclick", (ev) => {
  const it = (ev.target as HTMLElement).closest<HTMLElement>(".sh-item"); if (!it) return;
  const e = entries.get(it.dataset.p!);
  if (e?.dir) pane().navigate(e.path); else pane().navigate(parentOf(it.dataset.p!), true, it.dataset.p!);
});
// drag shelf items onto a folder / pane / another app
shelf.addEventListener("mousedown", (ev) => {
  const it = (ev.target as HTMLElement).closest<HTMLElement>(".sh-item");
  if (!it || ev.button !== 0 || (ev.target as HTMLElement).classList.contains("sh-x")) return;
  const x0 = ev.clientX, y0 = ev.clientY;
  const move = (m: MouseEvent) => {
    if (Math.hypot(m.clientX - x0, m.clientY - y0) < 6) return;
    cleanup();
    const p = it.dataset.p!;
    startDrag(picked.has(p) ? chosen() : [p], m);
  };
  const cleanup = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", cleanup); };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", cleanup);
});

export const addToShelf = (paths: string[]) => { if (paths.length) { invoke("shelf_add", { paths }); flash(`${paths.length} on the shelf`); } };

// ---------- menu + keys ----------
hooks.menuExtra.push((p, es) => {
  if (!es.length || es.some((e) => e.trashId) || p.loc === "trash:/") return [];
  const paths = es.map((e) => e.path);
  return [
    { label: "Copy To…", act: () => sendTo(paths, false) },
    { label: "Move To…", act: () => sendTo(paths, true) },
    { label: "Put on Shelf", kb: "Ctrl+Shift+S", act: () => addToShelf(paths) },
    "-",
  ];
});
hooks.keys.push((ev, p) => {
  if (ev.ctrlKey && ev.shiftKey && !ev.altKey && ev.key.toLowerCase() === "s") { addToShelf(p.selected().filter((e) => !e.trashId).map((e) => e.path)); return true; }
  return false;
});

refresh();
