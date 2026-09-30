// Quick open (Ctrl+P): fuzzy-find anything under home (fd index, nucleo ranking). Enter opens it,
// Shift+Enter shows it in its folder, Ctrl+Enter opens it (or its folder) in a new tab.
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { invoke, parentOf } from "./util";
import { hooks, pane, newTab, flash } from "./app";
import { modes, openPalette, paletteOpen } from "./palette";

type Hit = { path: string; rel: string; dir: boolean; score: number; idx: number[] };
let open = false;

modes.files = {
  title: "Open",
  placeholder: "Find a file or folder in your home…",
  hint: "Tab files/folders · Enter open · Shift+Enter show in folder · Ctrl+Enter new tab",
  kinds: [["", "All"], ["f", "Files"], ["d", "Folders"]],
  async query(q, kind) {
    const r = await invoke<{ items: Hit[]; total: number; indexing: boolean }>("fuzzy_find", { q, limit: 80, kind: kind || null });
    const items = r.items.map((h) => ({ path: h.path, dir: h.dir, label: `~/${h.rel.replace(/\/$/, "")}`, hl: h.idx.map((i) => i + 2) }));
    const note = r.indexing && !r.total ? "Indexing your home folder…" : `${r.total.toLocaleString()} items indexed${r.indexing ? " · refreshing" : ""}`;
    return { items, note };
  },
  pick(it, how) {
    if (how === "tab") newTab(it.dir ? it.path : parentOf(it.path), true, it.dir ? undefined : it.path);
    else if (how === "reveal") pane().navigate(parentOf(it.path), true, it.path);
    else if (it.dir) pane().navigate(it.path);
    else invoke("open_default", { paths: [it.path] }).catch((e) => flash(String(e)));
  },
};

export function quickOpen(initial = "") {
  open = true;
  openPalette("files", initial, () => { open = false; });
}
// the index finished (re)building: refresh what's shown
getCurrentWebviewWindow().listen("fuzzy-ready", () => {
  if (open && paletteOpen()) document.querySelector<HTMLInputElement>("#palette input")?.dispatchEvent(new Event("input"));
});
hooks.keys.push((ev) => {
  if (ev.ctrlKey && !ev.altKey && !ev.shiftKey && ev.key.toLowerCase() === "p") { quickOpen(); return true; }
  return false;
});
