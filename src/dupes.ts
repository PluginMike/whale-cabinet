// Duplicate finder (sidebar → Tools): scans a folder for identical files and lists them in sets, the ones
// wasting the most space first. Selection = what gets removed: "Keep newest / oldest / in this folder" select
// the extras, then Move to Trash (undoable) as usual.
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { $, Entry, invoke, esc, fmtSize, baseName, parentOf } from "./util";
import { hooks, pane, allPanes, flash, HOME } from "./app";
import { isFolder, Pane } from "./pane";
import { trashSel } from "./ops";

type Q = { id: number; root: string; min: number; hidden: boolean };
type Hits = { id: number; sets: { size: number; files: Entry[] }[]; scanned: number; done: boolean };
let nextId = 1 + Math.floor(Math.random() * 2 ** 30);
const results = new Map<number, { list: Entry[]; sets: number; done: boolean; scanned: number }>();
const labels = new Map<number, string>(); // group id → header
const parse = (loc: string) => JSON.parse(loc.slice(6)) as Q;
let gid = 0;

hooks.virtual!["dupes:"] = async (loc) => {
  const q = parse(loc);
  if (!results.has(q.id)) {
    results.set(q.id, { list: [], sets: 0, done: false, scanned: 0 });
    invoke("dupes_start", { id: q.id, root: q.root, minSize: q.min, hidden: q.hidden });
  }
  return results.get(q.id)!.list;
};
hooks.locTitle!["dupes:"] = (loc) => `Duplicates in ${baseName(parse(loc).root) || "/"}`;
hooks.group["dupes:"] = (e) => labels.get(e.group ?? -1) ?? "";
hooks.defaultSort["dupes:"] = "rank";

getCurrentWebviewWindow().listen<Hits>("dupes-hits", ({ payload: h }) => {
  const r = results.get(h.id); if (!r) return;
  if (h.scanned) r.scanned = h.scanned;
  for (const s of h.sets) {
    const g = ++gid, wasted = s.size * (s.files.length - 1);
    labels.set(g, `${s.files.length} copies · ${fmtSize(s.size)} each · ${fmtSize(wasted)} wasted`);
    for (const f of s.files) r.list.push({ ...f, group: g, score: wasted });
    r.sets++;
  }
  r.done = h.done;
  for (const p of allPanes()) if (p.loc.startsWith("dupes:") && parse(p.loc).id === h.id) { if (h.sets.length || h.done) p.setListing(r.list.slice()); else p.refreshSel(); }
  if (h.done) flash(r.sets ? `${r.sets} set${r.sets === 1 ? "" : "s"} of duplicates` : "No duplicates found");
});

// stop scans nobody is looking at
const prev = hooks.onNavigate;
hooks.onNavigate = (p) => {
  prev?.(p);
  const live = new Set(allPanes().filter((x) => x.loc.startsWith("dupes:")).map((x) => parse(x.loc).id));
  for (const id of [...results.keys()]) if (!live.has(id)) { invoke("search_cancel", { id }); results.delete(id); }
};

// after files are trashed/deleted, drop them (and sets left with one file)
getCurrentWebviewWindow().listen<{ state: string }>("op", async ({ payload }) => {
  if (payload.state !== "done") return;
  for (const p of allPanes().filter((x) => x.loc.startsWith("dupes:"))) {
    const r = results.get(parse(p.loc).id); if (!r) continue;
    const ok = await invoke<boolean[]>("paths_exist", { paths: r.list.map((e) => e.path) });
    const left = r.list.filter((_, i) => ok[i]);
    const per = new Map<number, number>();
    left.forEach((e) => per.set(e.group!, (per.get(e.group!) ?? 0) + 1));
    r.list = left.filter((e) => per.get(e.group!)! > 1);
    r.sets = new Set(r.list.map((e) => e.group)).size;
    p.setListing(r.list.slice());
  }
});

/** Select every file of each set except the one `keep` picks. */
function selectExtras(p: Pane, keep: (set: Entry[]) => Entry | undefined) {
  const sets = new Map<number, Entry[]>();
  for (const r of p.rows) if (!r.head && r.e.group) (sets.get(r.e.group) ?? sets.set(r.e.group, []).get(r.e.group)!).push(r.e);
  const pick: string[] = [];
  for (const set of sets.values()) { const k = keep(set); if (k) pick.push(...set.filter((e) => e !== k).map((e) => e.path)); }
  p.selectPaths(pick);
  flash(pick.length ? `${pick.length} extra cop${pick.length === 1 ? "y" : "ies"} selected` : "Nothing to select");
}

hooks.info.push((el, _picked, p) => {
  if (!p.loc.startsWith("dupes:")) return false;
  const q = parse(p.loc), r = results.get(q.id);
  const files = p.rows.filter((x) => !x.head);
  const wasted = [...new Map(files.map((x) => [x.e.group, x.e.score ?? 0])).values()].reduce((a, b) => a + b, 0);
  const focusDir = p.entry(p.focus) ? parentOf(p.focus) : "";
  const sel = p.selected();
  el.innerHTML = `<h2>Duplicates</h2><dl>
      <dt>In</dt><dd>${esc(q.root)}</dd><dt>Sets</dt><dd>${r?.sets ?? 0}${r && !r.done ? " (still looking…)" : ""}</dd>
      <dt>Wasted</dt><dd>${fmtSize(wasted)}</dd>${r && !r.done && r.scanned ? `<dt>Scanned</dt><dd>${r.scanned.toLocaleString()} files</dd>` : ""}
      ${sel.length ? `<dt>Selected</dt><dd>${sel.length} (${fmtSize(sel.reduce((a, e) => a + e.size, 0))})</dd>` : ""}</dl>
    <div class="mini">Select the extra copies, keeping…</div>
    <p class="dupe-btns"><button data-k="new">the newest</button> <button data-k="old">the oldest</button>
      <button data-k="dir" ${focusDir ? "" : "disabled"} title="${esc(focusDir)}">the one in ${esc(baseName(focusDir) || "the focused file's folder")}</button></p>
    <p><button id="dp-trash" ${sel.length ? "" : "disabled"}>Move ${sel.length || ""} selected to Trash</button></p>
    <p class="hint">Hard links aren't listed (removing one frees nothing). Trash is undoable with Ctrl+Z.</p>`;
  el.querySelector(".dupe-btns")!.addEventListener("click", (ev) => {
    const k = (ev.target as HTMLElement).dataset.k;
    if (k === "new") selectExtras(p, (s) => s.reduce((a, b) => (b.mtime > a.mtime ? b : a)));
    if (k === "old") selectExtras(p, (s) => s.reduce((a, b) => (b.mtime < a.mtime ? b : a)));
    if (k === "dir") selectExtras(p, (s) => s.find((e) => parentOf(e.path) === focusDir));
  });
  el.querySelector<HTMLButtonElement>("#dp-trash")!.onclick = () => trashSel(p);
  return true;
});

// ---------- Tools section ----------
$("tools").innerHTML = `<div class="recrow tool" id="tool-dupes"><i class="tool-ico"></i><span>Find duplicates…</span></div>`;
$("tool-dupes").addEventListener("click", () => startDialog());

function startDialog() {
  const p = pane();
  const here = isFolder(p.loc) ? p.loc : HOME;
  const m = document.createElement("div");
  m.className = "modal";
  m.innerHTML = `<div class="modal-box wide"><h3>Find duplicate files</h3>
    <label class="full">In <input id="du-root" spellcheck="false" value="${esc(here)}"></label>
    <label>Ignore files smaller than <select id="du-min"><option value="1">—</option><option value="1024" selected>1 KiB</option><option value="102400">100 KiB</option><option value="1048576">1 MiB</option><option value="10485760">10 MiB</option></select></label>
    <label><input type="checkbox" id="du-hidden"> Include hidden files and folders</label>
    <div class="btns"><button data-v="0">Cancel</button><button data-v="1" class="primary">Find</button></div></div>`;
  document.body.append(m);
  const q = (s: string) => m.querySelector<HTMLInputElement>(s)!;
  const close = () => { m.remove(); window.removeEventListener("keydown", key, true); };
  const go = async () => {
    const root = await invoke<string>("resolve_path", { input: q("#du-root").value, cwd: here }).catch((e) => { flash(String(e)); return ""; });
    if (!root) return;
    close();
    const s: Q = { id: nextId++, root, min: +q("#du-min").value, hidden: q("#du-hidden").checked };
    pane().navigate(`dupes:${JSON.stringify(s)}`);
    flash(`Looking for duplicates in ${root}…`);
  };
  const key = (e: KeyboardEvent) => { e.stopPropagation(); if (e.key === "Escape") close(); if (e.key === "Enter") { e.preventDefault(); go(); } };
  window.addEventListener("keydown", key, true);
  m.addEventListener("click", (e) => { const v = (e.target as HTMLElement).dataset.v; if (v === "0" || e.target === m) close(); if (v === "1") go(); });
  q("#du-root").focus();
}
