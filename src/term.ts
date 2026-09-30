// Embedded terminal panel (F4): xterm.js over a PTY running $SHELL; follows the active folder when the shell is idle.
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { listen } from "@tauri-apps/api/event";
import { $, invoke } from "./util";
import { hooks, pane } from "./app";
import { isFolder } from "./pane";
import type { Theme } from "./theme";

const panel = $("termpanel");
const ID = 1 + Math.floor(Math.random() * 2 ** 30); // unique per window: PTY events go to every window
let term: Terminal | null = null, fit: FitAddon | null = null, alive = false, palette: string[] = [];

/** Resolve a CSS variable (which may be a color-mix) to a concrete colour string xterm understands. */
function cssColor(v: string) {
  const probe = document.createElement("i");
  probe.style.color = `var(${v})`;
  document.body.append(probe);
  const c = getComputedStyle(probe).color;
  probe.remove();
  return c;
}
function xtermTheme() {
  const names = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
  const t: Record<string, string> = { background: cssColor("--body"), foreground: cssColor("--on-body"), cursor: cssColor("--accent"), selectionBackground: cssColor("--accent"), selectionForeground: cssColor("--on-accent") };
  palette.forEach((c, i) => { t[i < 8 ? names[i] : `bright${names[i - 8][0].toUpperCase()}${names[i - 8].slice(1)}`] = c; });
  return t;
}
const dir = () => (isFolder(pane().loc) ? pane().loc : "");

async function start() {
  palette = (await invoke<Theme>("get_theme")).terminal ?? [];
  if (!term) {
    term = new Terminal({ fontFamily: "ui-monospace, 'JetBrainsMono Nerd Font', 'JetBrains Mono', 'Fira Code', monospace", fontSize: 13, cursorBlink: true, scrollback: 5000, allowTransparency: true, theme: xtermTheme() });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(panel);
    term.onData((data) => invoke("term_write", { id: ID, data }));
    term.onResize(({ cols, rows }) => { if (alive) invoke("term_resize", { id: ID, cols, rows }); });
    new ResizeObserver(() => { if (!panel.hidden) fit!.fit(); }).observe(panel);
  }
  fit!.fit();
  await invoke("term_open", { id: ID, cwd: dir() || "/", cols: term.cols, rows: term.rows });
  alive = true;
}

export async function toggleTerminal() {
  panel.hidden = !panel.hidden;
  if (panel.hidden) { pane().scroller.focus(); return; }
  if (!alive) await start();
  else { fit!.fit(); if (dir()) invoke("term_cd", { id: ID, dir: dir() }); }
  term!.focus();
}

listen<{ id: number; data: string }>("term-data", ({ payload }) => { if (payload.id === ID) term?.write(payload.data); });
listen<number>("term-exit", ({ payload }) => {
  if (payload !== ID) return;
  alive = false;
  term?.write("\r\n[shell exited — press F4 twice for a new one]\r\n");
});
listen<Theme>("theme", ({ payload }) => { palette = payload.terminal ?? []; if (term) term.options.theme = xtermTheme(); });

// follow the active folder
const prev = hooks.onNavigate;
hooks.onNavigate = (p) => {
  prev?.(p);
  if (alive && !panel.hidden && p === pane() && isFolder(p.loc)) invoke("term_cd", { id: ID, dir: p.loc });
};

// F4 works even while the terminal has focus
window.addEventListener("keydown", (ev) => {
  if (ev.key === "F4" && !ev.shiftKey && !ev.ctrlKey && !ev.altKey) { ev.preventDefault(); ev.stopPropagation(); toggleTerminal(); }
}, true);
