// Action palette (Ctrl+Shift+P): every command, searchable, with its shortcut. Commands that have a key are
// run by replaying that key, so they behave exactly like pressing it.
import { invoke } from "./util";
import { hooks, pane, openSettings, setFocusMode, newTab, HOME } from "./app";
import { modes, openPalette, termHits } from "./palette";

type Action = { label: string; kb?: string; run: () => void; when?: () => boolean };
const press = (key: string, o: KeyboardEventInit = {}) => () => {
  pane().scroller.focus();
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...o }));
};
const ctrl = (key: string, o: KeyboardEventInit = {}) => press(key, { ctrlKey: true, ...o });
const sel = () => pane().sel.size > 0;
const click = (sel: string) => () => document.querySelector<HTMLElement>(sel)?.click();

/** Other modules can add their own. */
export const actions: Action[] = [
  { label: "Quick open (find a file or folder)", kb: "Ctrl+P", run: ctrl("p") },
  { label: "Jump to a frequent folder", kb: "Ctrl+J", run: ctrl("j") },
  { label: "Search here", kb: "Ctrl+F", run: ctrl("f") },
  { label: "Filter this view", kb: "Ctrl+I", run: ctrl("i") },
  { label: "Edit location", kb: "Ctrl+L", run: ctrl("l") },
  { label: "Go home", kb: "Alt+Home", run: () => pane().navigate(HOME) },
  { label: "Go up", kb: "Alt+↑", run: () => pane().goUp() },
  { label: "Back", kb: "Alt+←", run: () => pane().goBack() },
  { label: "Forward", kb: "Alt+→", run: () => pane().goFwd() },
  { label: "Reload", kb: "F5", run: press("F5") },
  { label: "New tab", kb: "Ctrl+T", run: ctrl("t") },
  { label: "Close tab", kb: "Ctrl+W", run: ctrl("w") },
  { label: "New window", kb: "Ctrl+N", run: ctrl("n") },
  { label: "Split view", kb: "F3", run: press("F3") },
  { label: "Terminal panel", kb: "F4", run: press("F4") },
  { label: "Open terminal here", kb: "Shift+F4", run: press("F4", { shiftKey: true }) },
  { label: "Focus mode", kb: "F12", run: () => setFocusMode() },
  { label: "Show / hide sidebar", kb: "F9", run: press("F9") },
  { label: "Show / hide info panel", kb: "F11", run: press("F11") },
  { label: "Show / hide hidden files", kb: "Ctrl+H", run: ctrl("h") },
  { label: "View: icons", kb: "Ctrl+1", run: ctrl("1") },
  { label: "View: compact", kb: "Ctrl+2", run: ctrl("2") },
  { label: "View: cabinet", kb: "Ctrl+3", run: ctrl("3") },
  { label: "Zoom in", kb: "Ctrl+=", run: ctrl("=") },
  { label: "Zoom out", kb: "Ctrl+-", run: ctrl("-") },
  { label: "Reset zoom", kb: "Ctrl+0", run: ctrl("0") },
  { label: "Select all", kb: "Ctrl+A", run: ctrl("a") },
  { label: "Invert selection", kb: "Ctrl+Shift+A", run: ctrl("A", { shiftKey: true }) },
  { label: "New folder", kb: "Ctrl+Shift+N", run: ctrl("N", { shiftKey: true }) },
  { label: "New file", kb: "Ctrl+Alt+N", run: ctrl("n", { altKey: true }) },
  { label: "Copy", kb: "Ctrl+C", run: ctrl("c"), when: sel },
  { label: "Cut", kb: "Ctrl+X", run: ctrl("x"), when: sel },
  { label: "Paste", kb: "Ctrl+V", run: ctrl("v") },
  { label: "Rename", kb: "F2", run: press("F2"), when: sel },
  { label: "Duplicate", kb: "Ctrl+D", run: ctrl("d"), when: sel },
  { label: "Move to trash", kb: "Del", run: press("Delete"), when: sel },
  { label: "Delete permanently", kb: "Shift+Del", run: press("Delete", { shiftKey: true }), when: sel },
  { label: "Undo", kb: "Ctrl+Z", run: ctrl("z") },
  { label: "Quick Look", kb: "Space", run: press(" "), when: sel },
  { label: "Properties", kb: "Alt+Enter", run: press("Enter", { altKey: true }) },
  { label: "Context menu", kb: "Menu", run: press("ContextMenu") },
  { label: "Copy selection to…", run: () => import("./shelf").then((m) => m.sendTo(pane().selected().map((e) => e.path), false)), when: sel },
  { label: "Move selection to…", run: () => import("./shelf").then((m) => m.sendTo(pane().selected().map((e) => e.path), true)), when: sel },
  { label: "Put selection on the shelf", kb: "Ctrl+Shift+S", run: ctrl("S", { shiftKey: true }), when: sel },
  { label: "Recent files", run: () => pane().navigate("recent:/") },
  { label: "Trash", run: () => pane().navigate("trash:/") },
  { label: "Find duplicates…", run: click("#tool-dupes") },
  { label: "Disk usage of this folder…", run: () => import("./usage").then((m) => m.openUsage(pane().targetDir() || HOME)) },
  { label: "Connect to server…", run: () => import("./network").then((m) => m.connectDialog()) },
  { label: "Manage tags…", run: () => import("./tags").then((m) => m.manageTags()) },
  { label: "New tag for the selection…", run: () => import("./tags").then((m) => m.newTag(pane().selected().map((e) => e.path))), when: sel },
  { label: "Open current folder in a new tab", run: () => newTab(pane().loc) },
  { label: "Copy location", run: () => invoke("copy_text", { text: pane().targetDir() || pane().loc }) },
  { label: "Settings", run: () => openSettings() },
];

/** Letters of `q` in order in `label` (fuzzy); lower score = better: earlier and tighter matches. */
function score(label: string, q: string) {
  const l = label.toLowerCase(), s = q.toLowerCase().replace(/\s+/g, "");
  let pos = -1, first = -1, gaps = 0;
  for (const c of s) {
    const i = l.indexOf(c, pos + 1);
    if (i < 0) return -1;
    if (first < 0) first = i;
    if (pos >= 0) gaps += i - pos - 1;
    pos = i;
  }
  return first + gaps * 2 + (l.includes(q.toLowerCase()) ? -50 : 0);
}
function fuzzyHits(label: string, q: string) {
  const plain = termHits(label, q);
  if (plain.length) return plain;
  const out: number[] = [], l = [...label.toLowerCase()];
  let pos = -1;
  for (const c of q.toLowerCase().replace(/\s+/g, "")) { const i = l.indexOf(c, pos + 1); if (i < 0) break; out.push(i); pos = i; }
  return out;
}

const byLabel = new Map<string, Action>();
modes.actions = {
  title: "Do",
  placeholder: "Type a command…",
  hint: "Enter run · Esc close",
  async query(q) {
    const live = actions.filter((a) => !a.when || a.when());
    const ranked = (q.trim() ? live.map((a) => [a, score(a.label, q.trim())] as const).filter(([, s]) => s >= 0).sort((a, b) => a[1] - b[1]).map(([a]) => a) : live);
    byLabel.clear();
    return { items: ranked.map((a) => { byLabel.set(a.label, a); return { path: a.label, dir: false, label: a.label, hl: q.trim() ? fuzzyHits(a.label, q.trim()) : [], sub: a.kb, noIcon: true }; }) };
  },
  pick(it) { const a = byLabel.get(it.path); if (a) setTimeout(a.run, 30); },
};

hooks.keys.push((ev) => {
  if (ev.ctrlKey && ev.shiftKey && !ev.altKey && ev.key.toLowerCase() === "p") { openPalette("actions"); return true; }
  return false;
});
