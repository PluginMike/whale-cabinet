// Recent files (shared ~/.local/share/recently-used.xbel): a timeline view grouped by day with bigger
// thumbnails and the app each file was opened with, plus the latest few in the sidebar.
import { listen } from "@tauri-apps/api/event";
import { $, Entry, invoke, esc, shown, edgeColor } from "./util";
import { hooks, pane, allPanes, flash } from "./app";
import { confirmBox } from "./ops";

type RecentEntry = Entry & { used: number; app: string };
const LOC = "recent:/";

const day = (ms: number) => { const d = new Date(ms); d.setHours(0, 0, 0, 0); return d.getTime(); };
const monthFmt = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" });
export function bucket(ms: number, now = Date.now()) {
  const d = (day(now) - day(ms)) / 86_400_000;
  if (d <= 0) return "Today";
  if (d < 2) return "Yesterday";
  if (d < 7) return "This week";
  const a = new Date(ms), b = new Date(now);
  if (a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth()) return "Earlier this month";
  return monthFmt.format(a);
}
const ago = (ms: number) => {
  const m = Math.round((Date.now() - ms) / 60000);
  return m < 1 ? "now" : m < 60 ? `${m} min` : m < 1440 ? `${Math.round(m / 60)} h` : `${Math.round(m / 1440)} d`;
};

const load = () => invoke<RecentEntry[]>("recent_list", { limit: 500 });
hooks.virtual!["recent:"] = async () => (await load()).map((e) => ({ ...e, score: e.used }));
hooks.locTitle!["recent:"] = () => "Recent files";
hooks.group["recent:"] = (e) => bucket(e.used ?? 0);
hooks.rowScale["recent:"] = 1.35;
hooks.defaultSort["recent:"] = "rank";

export const forget = async (paths: string[] | null) => { await invoke("recent_remove", { paths }).catch((e) => flash(String(e))); };

hooks.info.push((el, picked, p) => {
  if (p.loc !== LOC || picked.length) return false;
  const n = p.rows.filter((r) => !r.head).length;
  el.innerHTML = `<h2>Recent files</h2><dl><dt>Files</dt><dd>${n}</dd></dl>
    <p class="hint">Shared with other apps through <code>~/.local/share/recently-used.xbel</code>. Files opened here are added to it.</p>
    <p><button id="rc-clear">Clear history</button></p>`;
  el.querySelector<HTMLElement>("#rc-clear")!.onclick = async () => { if (await confirmBox("Forget every recent file? The files themselves stay where they are.", "Clear history", true)) forget(null); };
  return true;
});

// ---------- sidebar: the latest few ----------
async function renderSidebar() {
  const list = (await load().catch(() => [] as RecentEntry[])).slice(0, 5);
  $("recent").innerHTML = `<div class="drawer small" data-p="${LOC}" title="Everything you opened lately"><span class="label">Recent files</span><span class="handle"></span></div>` +
    list.map((e) => `<div class="recrow" data-open="${esc(e.path)}" title="${esc(e.path)}${e.app ? ` — ${esc(e.app)}` : ""}"><i style="--edge:${edgeColor(e)}"></i><span>${esc(shown(e.name))}</span><b>${ago(e.used)}</b></div>`).join("");
  const cur = pane()?.loc;
  document.querySelectorAll<HTMLElement>("#recent .drawer").forEach((d) => d.classList.toggle("active", d.dataset.p === cur));
}
$("recent").addEventListener("click", (ev) => {
  const r = (ev.target as HTMLElement).closest<HTMLElement>(".recrow"); if (!r) return;
  ev.stopPropagation();
  invoke("open_default", { paths: [r.dataset.open] }).catch((e) => flash(String(e)));
});
listen("recent-changed", () => { renderSidebar(); allPanes().filter((p) => p.loc === LOC).forEach((p) => p.reload()); });
// other apps add to the list too: catch up whenever the window comes back
window.addEventListener("focus", () => { renderSidebar(); allPanes().filter((p) => p.loc === LOC).forEach((p) => p.reload()); });
renderSidebar();
