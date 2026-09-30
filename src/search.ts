// Recursive search (Ctrl+F): a bar above the view; results stream into a virtual "search:" location.
import { listen } from "@tauri-apps/api/event";
import { $, Entry, invoke } from "./util";
import { hooks, pane, allPanes, flash, HOME } from "./app";
import { isFolder } from "./pane";

type Q = { id: number; root: string; q: string; content: boolean; hidden: boolean };
let nextId = 1 + Math.floor(Math.random() * 2 ** 30); // unique per window: hits go to every window
const results = new Map<number, Entry[]>();
const parse = (loc: string) => JSON.parse(loc.slice(7)) as Q;

const bar = document.createElement("div");
bar.id = "searchbar";
bar.hidden = true;
bar.innerHTML = `<input id="sq" placeholder="Search file names (* and ? work)…" spellcheck="false">
  <label><input type="checkbox" id="s-content"> Content</label>
  <label><input type="checkbox" id="s-hidden"> Hidden</label>
  <select id="s-where"><option value="here">From here</option><option value="home">Home</option><option value="root">Everywhere</option></select>
  <button id="s-close" title="Close (Esc)">✕</button>`;
$("center").insertBefore(bar, $("views"));
const input = bar.querySelector<HTMLInputElement>("#sq")!;
const val = (id: string) => bar.querySelector<HTMLInputElement>(id)!;

export function openSearch() {
  bar.hidden = false;
  const p = pane();
  if (p.loc.startsWith("search:")) { const q = parse(p.loc); input.value = q.q; val("#s-content").checked = q.content; }
  input.focus(); input.select();
}
function closeSearch() {
  bar.hidden = true;
  const p = pane();
  if (p.loc.startsWith("search:")) p.goBack();
  p.scroller.focus();
}
function run() {
  const q = input.value.trim(); if (!q) return;
  const p = pane();
  const here = isFolder(p.loc) ? p.loc : p.loc.startsWith("search:") ? parse(p.loc).root : HOME;
  const where = (bar.querySelector("#s-where") as HTMLSelectElement).value;
  const root = where === "home" ? HOME : where === "root" ? "/" : here;
  const s: Q = { id: nextId++, root, q, content: val("#s-content").checked, hidden: val("#s-hidden").checked };
  p.navigate(`search:${JSON.stringify(s)}`, !p.loc.startsWith("search:"));
  p.scroller.focus();
}
input.addEventListener("keydown", (e) => {
  e.stopPropagation();
  if (e.key === "Enter") run();
  if (e.key === "Escape") closeSearch();
  if (e.key === "ArrowDown") pane().scroller.focus();
});
bar.querySelector<HTMLElement>("#s-close")!.onclick = closeSearch;
bar.addEventListener("change", (e) => { if ((e.target as HTMLElement).id !== "sq" && input.value.trim()) run(); });

hooks.virtual!["search:"] = async (loc) => {
  const s = parse(loc);
  if (!results.has(s.id)) {
    results.set(s.id, []);
    invoke("search_start", { id: s.id, root: s.root, text: s.q, content: s.content, hidden: s.hidden });
    flash(`Searching ${s.root === "/" ? "everywhere" : s.root}…`);
  }
  return results.get(s.id)!;
};
hooks.locTitle!["search:"] = (loc) => { const s = parse(loc); return `Search: “${s.q}”${s.content ? " (content)" : ""}`; };

listen<{ id: number; entries: Entry[]; done: boolean }>("search-hits", ({ payload }) => {
  const list = results.get(payload.id); if (!list) return;
  list.push(...payload.entries);
  for (const p of allPanes()) if (p.loc.startsWith("search:") && parse(p.loc).id === payload.id) p.setListing(list.slice());
  if (payload.done) flash(`${list.length.toLocaleString()} found`);
});
// stop searches nobody is looking at any more
const prev = hooks.onNavigate;
hooks.onNavigate = (p) => {
  prev?.(p);
  const live = new Set(allPanes().filter((x) => x.loc.startsWith("search:")).map((x) => parse(x.loc).id));
  for (const id of [...results.keys()]) if (!live.has(id)) { invoke("search_cancel", { id }); results.delete(id); }
  if (!p.loc.startsWith("search:") && p === pane()) bar.hidden = true;
};
hooks.keys.push((ev) => {
  if (ev.ctrlKey && !ev.altKey && ev.key.toLowerCase() === "f") { openSearch(); return true; }
  return false;
});

