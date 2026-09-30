// zoxide navigation: every folder you open is fed to `zoxide add`; Ctrl+J (or `z foo` in the path bar) opens a
// jump box ranked by zoxide's frecency; the sidebar's Frequent drawer lists your top folders.
import { $, invoke, esc, baseName, parentOf } from "./util";
import { hooks, pane, newTab, HOME, flash } from "./app";
import { isFolder } from "./pane";
import { modes, openPalette, termHits } from "./palette";

export const tilde = (p: string) => (p === HOME ? "~" : p.startsWith(HOME + "/") ? "~" + p.slice(HOME.length) : p);

modes.jump = {
  title: "Jump",
  placeholder: "Folder you've been to… (zoxide)",
  hint: "Enter go · Ctrl+Enter new tab · Shift+Enter show in parent",
  async query(q) {
    const hits = await invoke<[number, string][]>("zoxide_query", { q, limit: 40 });
    const items = hits.map(([score, path]) => { const label = tilde(path); return { path, dir: true, label, hl: termHits(label, q), sub: score >= 10 ? String(Math.round(score)) : score.toFixed(1) }; });
    return { items, note: items.length ? "" : q.trim() ? "zoxide knows no folder matching that yet" : "" };
  },
  pick(it, how) {
    if (how === "tab") newTab(it.path);
    else if (how === "reveal") pane().navigate(parentOf(it.path), true, it.path);
    else pane().navigate(it.path);
  },
};

// ---------- Frequent drawer ----------
let lastFreq = 0, freqTimer = 0;
export async function refreshFrequent() {
  lastFreq = performance.now();
  const top = await invoke<[number, string][]>("zoxide_query", { q: "", limit: 8 }).catch(() => [] as [number, string][]);
  $("freq-head").hidden = !top.length;
  $("frequent").innerHTML = top.map(([s, p]) =>
    `<div class="drawer small" data-p="${esc(p)}" title="${esc(tilde(p))} — frecency ${Math.round(s)}"><span class="label">${esc(baseName(p) || "/")}</span><span class="handle"></span></div>`).join("");
  const cur = pane()?.loc;
  document.querySelectorAll<HTMLElement>("#frequent .drawer").forEach((d) => d.classList.toggle("active", d.dataset.p === cur));
}

const prev = hooks.onNavigate;
hooks.onNavigate = (p) => {
  prev?.(p);
  if (isFolder(p.loc)) invoke("zoxide_add", { path: p.loc });
  // re-rank at most every few seconds; the list barely moves between visits
  clearTimeout(freqTimer);
  freqTimer = window.setTimeout(refreshFrequent, performance.now() - lastFreq > 5000 ? 300 : 5000);
};

hooks.keys.push((ev) => {
  if (ev.ctrlKey && !ev.altKey && !ev.shiftKey && ev.key.toLowerCase() === "j") { openPalette("jump"); return true; }
  return false;
});

// `z foo` typed into the location bar opens the jump box with that query
const pathedit = $<HTMLInputElement>("pathedit");
pathedit.addEventListener("input", () => {
  const m = /^z\s+(.*)$/.exec(pathedit.value);
  if (!m) return;
  pathedit.blur();
  openPalette("jump", m[1]);
});

refreshFrequent().catch((e) => flash(String(e)));
