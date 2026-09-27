// Dev-only UI smoke test (run via scripts/smoke.sh). Drives the real handlers with synthetic events and
// prints PASS/FAIL lines; SHOT/ACT markers let the driver take screenshots or poke the desktop.
import { invoke } from "./util";
import * as app from "./app";

const log = (m: string) => invoke("selftest", { msg: m });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const key = (key: string, o: KeyboardEventInit = {}) => window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...o }));
const until = async (f: () => boolean, ms = 5000) => { const t = performance.now(); while (!f()) { if (performance.now() - t > ms) return -1; await sleep(10); } return performance.now() - t; };
const names = () => app.pane().rows.map((r) => r.e.name);
let fails = 0;
const check = (name: string, ok: boolean, extra = "") => { if (!ok) fails++; log(`${ok ? "PASS" : "FAIL"} ${name} ${extra}`); };
const cssVar = (v: string) => getComputedStyle(document.documentElement).getPropertyValue(v).trim();
export const suites: Record<string, (base: string) => Promise<void>> = {};

suites.browse = async (base) => {
  const p = () => app.pane();
  const t0 = performance.now();
  p().navigate(`${base}/big`);
  const t = await until(() => p().rows.length === 10300);
  check("10k listing", t >= 0, `${Math.round(performance.now() - t0)}ms`);
  const r = p().rows;
  check("folders first + natural order", r[0].e.name === "folder 0" && r[1].e.name === "folder 1" && r[300].e.name === "file 0.txt" && r[302].e.name === "file 2.rs");
  p().scroller.scrollTop = 150000; await sleep(50);
  check("virtualized DOM", document.querySelectorAll(".item").length < 80, `${document.querySelectorAll(".item").length} nodes`);
  p().scroller.scrollTop = 0; await sleep(30);
  key("ArrowDown"); key("ArrowDown"); key("ArrowDown", { shiftKey: true }); key("ArrowDown", { shiftKey: true });
  check("arrow + shift select", p().sel.size === 3, `sel=${p().sel.size}`);
  key("a", { ctrlKey: true });
  check("ctrl+a", p().sel.size === 10300);
  key("Escape");
  check("escape clears", p().sel.size === 0);
  const f = document.getElementById("filter") as HTMLInputElement;
  f.value = "file 99"; f.dispatchEvent(new Event("input"));
  check("filter", p().rows.length === 111, `rows=${p().rows.length}`);
  f.value = ""; f.dispatchEvent(new Event("input"));
  // rubber band from the whitespace of row 5 down to row 9
  const pr = p().scroller.getBoundingClientRect();
  const y = (i: number) => pr.top + 8 + i * 36 + 20, x = pr.right - 300;
  document.querySelector(`.item[data-i="5"] .body`)!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, clientX: x, clientY: y(5), button: 0 }));
  window.dispatchEvent(new MouseEvent("mousemove", { clientX: x + 30, clientY: y(9) }));
  window.dispatchEvent(new MouseEvent("mouseup", { clientX: x + 30, clientY: y(9) }));
  check("rubber band", p().sel.size === 5, `sel=${p().sel.size}`);

  p().navigate(`${base}/play`);
  await until(() => names().includes("sub dir"));
  const n = names();
  check("odd names listed", ["new\nline", "ünïcode 💾.pdf", "dangling", "pipe"].every((x) => n.includes(x)));
  check("b 9 before b 10", n.indexOf("b 9.txt") < n.indexOf("b 10.txt"));
  key("h", { ctrlKey: true }); await sleep(30);
  check("ctrl+h shows hidden", names().includes(".secret"));
  const i = p().rows.findIndex((r) => r.e.name === "sub dir");
  document.querySelector(`.item[data-i="${i}"] .chev`)!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
  check("pull out folder", (await until(() => names().includes("x.png"))) >= 0);
  await sleep(250);
  log("SHOT expanded");
  await sleep(800);
  const count = p().rows.length;
  log("ACT touch");
  check("live refresh", (await until(() => p().rows.length === count + 2, 3000)) >= 0, `rows ${count} -> ${p().rows.length}`);
  key("ArrowUp", { altKey: true });
  check("up selects previous folder", (await until(() => p().sel.size === 1 && [...p().sel][0] === `${base}/play`)) >= 0);
};

suites.theme = async () => {
  const theme = await invoke<any>("get_theme");
  log(`theme source=${theme.source} dark=${theme.dark} primary=${theme.roles.primary} rounding=${theme.hypr.rounding}`);
  if (theme.source === "builtin") {
    check("builtin fallback palette", cssVar("--accent") === "#d8772e", cssVar("--accent"));
    log("SHOT builtin");
    return;
  }
  check("accent = DMS primary", cssVar("--accent") === theme.roles.primary, `${cssVar("--accent")} vs ${theme.roles.primary}`);
  check("radius from Hyprland", !theme.hypr.running || cssVar("--radius") === `${Math.min(theme.hypr.rounding, 18)}px`, cssVar("--radius"));
  check("font from gsettings", cssVar("--font").includes(theme.font_family), cssVar("--font"));
  log("SHOT themed");
  const before = cssVar("--interior") + cssVar("--accent");
  const t0 = performance.now();
  log("ACT theme-toggle");
  const dt = await until(() => cssVar("--interior") + cssVar("--accent") !== before, 8000);
  check("live re-theme on dark/light switch", dt >= 0, `${Math.round(performance.now() - t0)}ms after request`);
  await sleep(400);
  log("SHOT toggled");
  const before2 = cssVar("--interior") + cssVar("--accent");
  log("ACT theme-toggle");
  await until(() => cssVar("--interior") + cssVar("--accent") !== before2, 8000);
  const b3 = cssVar("--accent");
  const t1 = performance.now();
  log("ACT wallpaper-next");
  const dt2 = await until(() => cssVar("--accent") !== b3, 15000);
  check("live re-theme on wallpaper change", dt2 >= 0, `${Math.round(performance.now() - t1)}ms after request`);
  await sleep(400);
  log("SHOT wallpaper");
};

(async () => {
  const base: string = await invoke("selftest_dir");
  const which = (await invoke<string>("selftest_suites")).split(",").filter(Boolean);
  await sleep(600);
  for (const s of which.length ? which : Object.keys(suites)) {
    log(`suite ${s}`);
    try { await suites[s](base); } catch (e) { check(`${s} crashed`, false, String(e)); }
  }
  log(`DONE fails=${fails}`);
})();
