// Disk usage map (Tools → Disk usage…, a folder's right-click menu, or the action palette): a sunburst of what
// takes the space under a folder plus a sorted list; click to drill in, the centre (or Backspace) to go up,
// right-click to open, show or trash. One scan, kept in the backend, so drilling needs no rescan.
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { $, invoke, esc, shown, fmtSize, baseName, parentOf, TAG_COLORS } from "./util";
import { hooks, pane, flash, HOME } from "./app";
import { isFolder } from "./pane";
import { showMenu } from "./menu";
import { confirmBox } from "./ops";

type View = { name: string; path: string; size: number; files: number; dir: boolean; rest: boolean; kids: View[] };
const PALETTE = TAG_COLORS.filter((c) => c !== "grey");
let id = 0, root = "", cur = "", view: View | null = null, open = false;

const el = document.createElement("div");
el.id = "usage";
el.hidden = true;
el.innerHTML = `<div class="us-head"><button data-a="up" title="Up (Backspace)">↑</button><nav class="us-crumbs"></nav><span class="us-total"></span>
  <button data-a="rescan" title="Scan again">Rescan</button><button data-a="close" title="Close (Esc)">✕</button></div>
  <div class="us-body"><div class="us-chart"><svg viewBox="-100 -100 200 200"></svg></div><div class="us-list"></div></div><div class="us-tip" hidden></div>`;
document.body.append(el);
const svg = el.querySelector("svg")!, listEl = el.querySelector<HTMLElement>(".us-list")!, tip = el.querySelector<HTMLElement>(".us-tip")!;
const segs = new Map<string, View>(); // svg segment key → node

function arc(r1: number, r2: number, a0: number, a1: number) {
  a1 = Math.min(a1, a0 + Math.PI * 2 - 1e-4);
  const p = (r: number, a: number) => `${(r * Math.sin(a)).toFixed(3)} ${(-r * Math.cos(a)).toFixed(3)}`;
  const large = a1 - a0 > Math.PI ? 1 : 0;
  return `M${p(r2, a0)}A${r2} ${r2} 0 ${large} 1 ${p(r2, a1)}L${p(r1, a1)}A${r1} ${r1} 0 ${large} 0 ${p(r1, a0)}Z`;
}
const colour = (i: number, depth: number, n: View) => n.rest ? "var(--muted)" : `color-mix(in srgb, var(--t-${PALETTE[i % PALETTE.length]}) ${Math.max(35, 100 - depth * 18)}%, var(--interior))`;

function draw() {
  if (!view) return;
  segs.clear();
  const R0 = 26, RINGS = 3, W = (96 - R0) / RINGS;
  let html = `<circle r="${R0 - 1}" class="us-centre" data-k="up"></circle>
    <text class="us-c1" y="-2">${esc(shown(view.name))}</text><text class="us-c2" y="10">${fmtSize(view.size)}</text>`;
  let k = 0;
  const ring = (n: View, depth: number, a0: number, a1: number, hue: number) => {
    if (depth > RINGS || !n.size) return;
    let a = a0;
    n.kids.forEach((c, i) => {
      const span = ((a1 - a0) * c.size) / (n.size || 1);
      if (span > 0.004) {
        const key = `s${k++}`; segs.set(key, c);
        const h = depth === 1 ? i : hue;
        html += `<path d="${arc(R0 + (depth - 1) * W, R0 + depth * W - 0.6, a, a + span)}" fill="${colour(h, depth, c)}" class="${c.dir ? "d" : "f"}" data-k="${key}"></path>`;
        if (c.dir) ring(c, depth + 1, a, a + span, h);
      }
      a += span;
    });
  };
  ring(view, 1, 0, Math.PI * 2, 0);
  svg.innerHTML = html;
  const total = view.size || 1;
  listEl.innerHTML = view.kids.map((c, i) => `<div class="us-row${c.dir ? " dir" : ""}${c.rest ? " rest" : ""}" data-p="${esc(c.path)}">
      <i style="background:${colour(i, 1, c)}"></i><span class="us-name">${esc(shown(c.name))}</span>
      <span class="us-bar"><b style="width:${((c.size / total) * 100).toFixed(1)}%"></b></span><span class="us-size">${fmtSize(c.size)}</span><span class="us-pct">${Math.round((c.size / total) * 100)}%</span></div>`).join("");
  // breadcrumbs from the scanned folder down to here
  const parts = cur === root ? [] : cur.slice(root.length + 1).split("/");
  let acc = root;
  el.querySelector(".us-crumbs")!.innerHTML = `<button data-p="${esc(root)}">${esc(shown(baseName(root) || "/"))}</button>` +
    parts.map((x) => { acc += "/" + x; return `<span>›</span><button data-p="${esc(acc)}">${esc(shown(x))}</button>`; }).join("");
  el.querySelector(".us-total")!.textContent = `${fmtSize(view.size)} · ${view.files.toLocaleString()} files`;
  (el.querySelector("[data-a=up]") as HTMLButtonElement).disabled = cur === root;
}

async function show(path: string) {
  try {
    view = await invoke<View>("usage_view", { id, path, depth: 4 });
    cur = path;
    draw();
  } catch (e) { flash(String(e)); }
}
const find = (p: string) => segs.get(p) ?? view?.kids.find((k) => k.path === p);

function scanning(files: number, bytes: number) {
  svg.innerHTML = `<circle r="60" class="us-spin"></circle><text class="us-c1" y="-2">Scanning…</text><text class="us-c2" y="10">${files.toLocaleString()} files · ${fmtSize(bytes)}</text>`;
  listEl.innerHTML = "";
  el.querySelector(".us-total")!.textContent = "";
}

export function openUsage(dir: string) {
  if (!isFolder(dir)) dir = HOME;
  if (id) invoke("usage_drop", { id, paths: null });
  id = 1 + Math.floor(Math.random() * 2 ** 30);
  root = cur = dir; view = null; open = true;
  el.hidden = false;
  el.querySelector(".us-crumbs")!.innerHTML = `<button>${esc(shown(baseName(dir) || "/"))}</button>`;
  scanning(0, 0);
  invoke("usage_scan", { id, root: dir });
}
function close() {
  el.hidden = true; open = false; tip.hidden = true;
  invoke("search_cancel", { id }); invoke("usage_drop", { id, paths: null });
  id = 0;
  pane().scroller.focus();
}
const up = () => { if (cur !== root) show(parentOf(cur)); };

getCurrentWebviewWindow().listen<{ id: number; files: number; bytes: number; done: boolean; error: string | null }>("usage", ({ payload: u }) => {
  if (u.id !== id) return;
  if (u.error) { flash(u.error); close(); return; }
  if (u.done) show(root); else scanning(u.files, u.bytes);
});

el.addEventListener("click", (ev) => {
  const t = ev.target as HTMLElement;
  const a = t.closest<HTMLElement>("[data-a]")?.dataset.a;
  if (a === "close") return close();
  if (a === "up") return up();
  if (a === "rescan") return openUsage(root);
  const crumb = t.closest<HTMLElement>(".us-crumbs button[data-p]");
  if (crumb) return show(crumb.dataset.p!);
  const key = (t as unknown as SVGElement).dataset?.k;
  if (key === "up") return up();
  const n = key ? segs.get(key) : find(t.closest<HTMLElement>(".us-row")?.dataset.p ?? "");
  if (n?.dir && !n.rest) show(n.path);
  else if (n && !n.rest) { close(); pane().navigate(parentOf(n.path), true, n.path); }
});
el.addEventListener("contextmenu", (ev) => {
  const t = ev.target as HTMLElement;
  const key = (t as unknown as SVGElement).dataset?.k;
  const n = key && key !== "up" ? segs.get(key) : find(t.closest<HTMLElement>(".us-row")?.dataset.p ?? "");
  if (!n || n.rest) return;
  ev.preventDefault();
  showMenu(ev.clientX, ev.clientY, [
    ...(n.dir ? [{ label: "Look inside", act: () => show(n.path) }] : []),
    { label: n.dir ? "Open Folder" : "Show in Folder", act: () => { close(); n.dir ? pane().navigate(n.path) : pane().navigate(parentOf(n.path), true, n.path); } },
    "-",
    { label: `Move to Trash (${fmtSize(n.size)})`, danger: true, act: async () => {
      if (!(await confirmBox(`Move “${shown(n.name)}” (${fmtSize(n.size)}) to the trash?`, "Move to Trash", true))) return;
      await invoke("op_trash", { items: [n.path] });
      await invoke("usage_drop", { id, paths: [n.path] });
      show(cur);
    } },
  ]);
});
el.addEventListener("mousemove", (ev) => {
  const key = (ev.target as unknown as SVGElement).dataset?.k;
  const n = key && key !== "up" ? segs.get(key) : undefined;
  tip.hidden = !n;
  if (!n || !view) return;
  tip.innerHTML = `<b>${esc(shown(n.name))}</b><br>${fmtSize(n.size)} · ${Math.round((n.size / (view.size || 1)) * 100)}%${n.dir ? ` · ${n.files.toLocaleString()} files` : ""}`;
  tip.style.left = `${ev.clientX + 14}px`; tip.style.top = `${ev.clientY + 12}px`;
});
window.addEventListener("keydown", (ev) => {
  if (!open) return;
  ev.stopPropagation();
  if (ev.key === "Escape") { ev.preventDefault(); close(); }
  if (ev.key === "Backspace" || (ev.altKey && ev.key === "ArrowUp")) { ev.preventDefault(); up(); }
}, true);

// ---------- entry points ----------
$("tools").insertAdjacentHTML("beforeend", `<div class="recrow tool" id="tool-usage"><i class="tool-ico"></i><span>Disk usage…</span></div>`);
$("tool-usage").addEventListener("click", () => openUsage(pane().targetDir() || HOME));
hooks.menuExtra.push((_p, es) => (es.length === 1 && es[0].dir && !es[0].trashId ? [{ label: "Disk Usage…", act: () => openUsage(es[0].path) }] : []));
