// Git status badges (modified / staged / untracked / conflicted, ignored dimmed) and a branch chip in the top bar.
// One `git status` per repository, re-run when something in it (or its .git folder) changes.
import { listen } from "@tauri-apps/api/event";
import { $, Entry, invoke, esc } from "./util";
import { hooks, pane, allPanes, host, HOME, flash } from "./app";
import { confirmBox } from "./ops";
import { highlight } from "./preview";
import { isFolder, Pane } from "./pane";

type Status = { root: string; branch: string; ahead: number; behind: number; files: Record<string, string>; dirs: Record<string, string> };
const repos = new Map<string, Status>(); // root → last status
const timers = new Map<string, number>();

const inRepo = (path: string, root: string) => path === root || path.startsWith(root === "/" ? "/" : root + "/");
function repoFor(path: string) {
  let best: Status | undefined;
  for (const st of repos.values()) if (inRepo(path, st.root) && (!best || st.root.length > best.root.length)) best = st;
  return best;
}

async function fetchFor(dir: string) {
  const st = await invoke<Status | null>("git_status", { dir }).catch(() => null);
  if (!st) { const old = repoFor(dir); if (old && dir.startsWith(old.root)) repos.delete(old.root); }
  else repos.set(st.root, st);
  allPanes().forEach((p) => p.render());
  if (pane()) chip(pane());
  host.syncWatch();
}
function refetch(root: string) {
  clearTimeout(timers.get(root));
  timers.set(root, window.setTimeout(() => { timers.delete(root); fetchFor(root); }, 400));
}

hooks.git = (e: Entry) => {
  const st = repoFor(e.path); if (!st || e.path === st.root) return;
  const rel = e.path.slice(st.root.length + 1);
  // a dotfiles repo at ~ would mark everything untracked: there, only real changes show
  const quiet = st.root === HOME, loud = (c?: string) => (quiet && (c === "U" || c === "I") ? undefined : c);
  const c = loud(st.files[rel] ?? (e.dir ? st.dirs[rel] : undefined)); if (c) return c;
  if (quiet) return;
  // inside an ignored or untracked folder
  for (let i = rel.lastIndexOf("/"); i > 0; i = rel.lastIndexOf("/", i - 1)) { const a = st.files[rel.slice(0, i)]; if (a === "I" || a === "U") return a; }
};

function chip(p: Pane) {
  const el = $("gitchip");
  const st = isFolder(p.loc) ? repoFor(p.loc) : undefined;
  el.hidden = !st;
  if (!st) return;
  const dirty = Object.values(st.files).filter((c) => c !== "I").length;
  el.innerHTML = `<b>⎇</b> ${esc(st.branch === "(detached)" ? "detached" : st.branch)}${st.ahead ? ` <span title="ahead">↑${st.ahead}</span>` : ""}${st.behind ? ` <span title="behind">↓${st.behind}</span>` : ""}`;
  el.title = `${st.root}\n${dirty ? `${dirty} changed path${dirty === 1 ? "" : "s"}` : "clean"}`;
  el.classList.toggle("dirty", dirty > 0);
}
hooks.chrome.push(chip);

const prev = hooks.onNavigate;
hooks.onNavigate = (p) => { prev?.(p); if (isFolder(p.loc)) fetchFor(p.loc); };
// changes inside a repo, or to its .git folder (commits, staging from a terminal)
listen<string[]>("fs-change", ({ payload }) => {
  for (const st of repos.values()) if (payload.some((d) => inRepo(d, st.root))) refetch(st.root);
});
hooks.extraWatch.push(() => [...new Set(allPanes().filter((p) => isFolder(p.loc)).map((p) => repoFor(p.loc)?.root).filter(Boolean).map((r) => `${r}/.git`))]);

// ---------- actions (right-click → Git) ----------

let difftool = false;
invoke<boolean>("has_difftool").then((v) => (difftool = v));
hooks.menuExtra.push((_p, es) => {
  const st = es.length ? repoFor(es[0].path) : undefined;
  if (!st || es.some((e) => e.trashId || !inRepo(e.path, st.root) || e.path === st.root)) return [];
  const codes = es.map((e) => hooks.git!(e));
  const paths = es.map((e) => e.path);
  const changed = codes.some((c) => c === "M" || c === "U" || c === "C");
  const staged = codes.some((c) => c === "S");
  const one = es.length === 1 && !es[0].dir ? es[0] : null;
  const act = async (action: string, ps = paths) => {
    try { await invoke("git_act", { root: st.root, action, paths: ps }); } catch (e) { flash(String(e)); }
    fetchFor(st.root);
  };
  return [{ label: "Git", sub: () => [
    { label: "Stage", off: !changed, act: () => act("stage") },
    { label: "Unstage", off: !staged, act: () => act("unstage") },
    { label: "Discard Changes…", danger: true, off: !codes.some((c) => c === "M" || c === "U"), act: async () => {
      const tracked = es.filter((_, i) => codes[i] === "M").map((e) => e.path), untracked = es.filter((_, i) => codes[i] === "U").map((e) => e.path);
      const what = [tracked.length && `undo the edits to ${tracked.length} file${tracked.length === 1 ? "" : "s"}`, untracked.length && `move ${untracked.length} new item${untracked.length === 1 ? "" : "s"} to the trash`].filter(Boolean).join(" and ");
      if (!(await confirmBox(`Discard changes: ${what}? Edits can't be brought back.`, "Discard", true))) return;
      if (tracked.length) await act("discard", tracked);
      if (untracked.length) invoke("op_trash", { items: untracked });
    } },
    "-",
    { label: "Show Changes", off: !one || !["M", "S", "U"].includes(codes[0] ?? ""), act: () => showDiff(st.root, one!.path, codes[0] === "U") },
    ...(difftool ? [{ label: "Compare With Last Commit", off: !one || !["M", "S"].includes(codes[0] ?? ""), act: () => invoke("git_difftool", { root: st.root, path: one!.path }).catch((e) => flash(String(e))) }] : []),
  ] }, "-"];
});


async function showDiff(root: string, path: string, untracked: boolean) {
  let text = "";
  try { text = await invoke<string>("git_diff", { root, path, untracked }); } catch (e) { flash(String(e)); return; }
  const m = document.createElement("div");
  m.className = "modal diffview";
  m.innerHTML = `<div class="modal-box"><div class="dv-head"><b>${esc(path.slice(root.length + 1))}</b><span>${untracked ? "new file" : "changes since the last commit"}</span><button data-v="0">Close</button></div><div class="dv-body"></div></div>`;
  m.querySelector(".dv-body")!.append(highlight(text || "(no changes)", "diff"));
  document.body.append(m);
  const close = () => { m.remove(); window.removeEventListener("keydown", key, true); pane().scroller.focus(); };
  const key = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); close(); } };
  window.addEventListener("keydown", key, true);
  m.addEventListener("click", (e) => { if (e.target === m || (e.target as HTMLElement).dataset.v === "0") close(); });
}

