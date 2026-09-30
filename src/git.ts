// Git status badges (modified / staged / untracked / conflicted, ignored dimmed) and a branch chip in the top bar.
// One `git status` per repository, re-run when something in it (or its .git folder) changes.
import { listen } from "@tauri-apps/api/event";
import { $, Entry, invoke, esc } from "./util";
import { hooks, pane, allPanes, host } from "./app";
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
  const c = st.files[rel] ?? (e.dir ? st.dirs[rel] : undefined); if (c) return c;
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
