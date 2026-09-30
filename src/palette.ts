// Command-palette style overlay shared by the zoxide jump box (Ctrl+J, or `z foo` in the path bar) and the
// fuzzy quick-open (Ctrl+P). A mode supplies ranked items for a query; the overlay handles typing, keys and
// highlighting. Only the newest query's answer is ever shown.
import { esc, shown } from "./util";

export type PalItem = { path: string; dir: boolean; label: string; hl?: number[]; sub?: string };
export type PalMode = {
  title: string;
  placeholder: string;
  hint: string;
  query(q: string): Promise<{ items: PalItem[]; note?: string }>;
  /** how: "open" (Enter), "reveal" (Shift+Enter), "tab" (Ctrl+Enter) */
  pick(it: PalItem, how: "open" | "reveal" | "tab"): void;
};
export const modes: Record<string, PalMode> = {};

const root = document.createElement("div");
root.id = "palette";
root.hidden = true;
root.innerHTML = `<div class="pal-box" role="dialog"><div class="pal-head"><span class="pal-mode"></span><input spellcheck="false" autocomplete="off"></div><div class="pal-list" role="listbox"></div><div class="pal-foot"><span class="pal-note"></span><span class="pal-hint"></span></div></div>`;
document.body.append(root);
const input = root.querySelector("input")!, list = root.querySelector<HTMLElement>(".pal-list")!;
let mode: PalMode | null = null, items: PalItem[] = [], hl = 0, seq = 0, busy = false, again = false, onClose: (() => void) | null = null;

/** Wrap highlighted character positions (code points) in <b>. */
export function marked(label: string, idx: number[] = []) {
  const set = new Set(idx);
  return [...label].map((c, i) => (set.has(i) ? `<b>${esc(c)}</b>` : esc(c))).join("");
}
/** Positions of each whitespace-separated term's first case-insensitive occurrence, for substring matchers. */
export function termHits(label: string, q: string) {
  const chars = [...label.toLowerCase()], out: number[] = [];
  for (const t of q.toLowerCase().split(/\s+/).filter(Boolean)) {
    const tc = [...t];
    for (let i = chars.length - tc.length; i >= 0; i--) if (tc.every((c, j) => chars[i + j] === c)) { for (let j = 0; j < tc.length; j++) out.push(i + j); break; }
  }
  return out;
}

function draw() {
  list.innerHTML = items.map((it, i) =>
    `<div class="pal-item${i === hl ? " hl" : ""}${it.dir ? " dir" : ""}" data-i="${i}" role="option"><i class="pal-ico"></i><span class="pal-label"><bdi dir="ltr">${marked(shown(it.label), it.hl)}</bdi></span>${it.sub ? `<span class="pal-sub">${esc(it.sub)}</span>` : ""}</div>`).join("")
    || `<div class="pal-empty">${input.value.trim() || mode?.title !== "Jump" ? "No matches" : "Type to search"}</div>`;
  list.querySelector(".hl")?.scrollIntoView({ block: "nearest" });
}

async function refresh() {
  if (!mode) return;
  if (busy) { again = true; return; }
  busy = true;
  const my = ++seq, m = mode;
  try {
    const r = await m.query(input.value);
    if (my === seq && mode === m) { items = r.items; hl = 0; root.querySelector(".pal-note")!.textContent = r.note ?? ""; draw(); }
  } catch (e) { root.querySelector(".pal-note")!.textContent = String(e); }
  busy = false;
  if (again) { again = false; refresh(); }
}

export function openPalette(name: string, initial = "", closed?: () => void) {
  mode = modes[name];
  if (!mode) return;
  onClose = closed ?? null;
  root.querySelector(".pal-mode")!.textContent = mode.title;
  root.querySelector(".pal-hint")!.textContent = mode.hint;
  input.placeholder = mode.placeholder;
  input.value = initial;
  items = []; hl = 0; draw();
  root.hidden = false;
  requestAnimationFrame(() => root.classList.add("in"));
  input.focus();
  refresh();
}
export function closePalette() {
  if (root.hidden) return;
  root.hidden = true; root.classList.remove("in");
  mode = null; seq++;
  onClose?.();
}
export const paletteOpen = () => !root.hidden;

function pick(i: number, how: "open" | "reveal" | "tab") {
  const it = items[i], m = mode; if (!it || !m) return;
  closePalette();
  m.pick(it, how);
}

input.addEventListener("input", refresh);
input.addEventListener("keydown", (ev) => {
  ev.stopPropagation();
  const move = (d: number) => { ev.preventDefault(); if (items.length) { hl = Math.max(0, Math.min(items.length - 1, hl + d)); draw(); } };
  if (ev.key === "Escape") { ev.preventDefault(); closePalette(); }
  else if (ev.key === "ArrowDown") move(1);
  else if (ev.key === "ArrowUp") move(-1);
  else if (ev.key === "PageDown") move(8);
  else if (ev.key === "PageUp") move(-8);
  else if (ev.key === "Enter") { ev.preventDefault(); pick(hl, ev.ctrlKey ? "tab" : ev.shiftKey ? "reveal" : "open"); }
});
list.addEventListener("mousemove", (ev) => {
  const el = (ev.target as HTMLElement).closest<HTMLElement>(".pal-item");
  if (el && +el.dataset.i! !== hl) { hl = +el.dataset.i!; list.querySelectorAll(".pal-item").forEach((x, i) => x.classList.toggle("hl", i === hl)); }
});
list.addEventListener("click", (ev) => {
  const el = (ev.target as HTMLElement).closest<HTMLElement>(".pal-item");
  if (el) pick(+el.dataset.i!, ev.ctrlKey ? "tab" : ev.shiftKey ? "reveal" : "open");
});
root.addEventListener("mousedown", (ev) => { if (ev.target === root) { ev.preventDefault(); closePalette(); } });
