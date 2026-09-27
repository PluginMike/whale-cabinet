// Dev-only UI smoke test (WC_SELFTEST=1 WC_DIR=<dir with big/ and play/>). Drives the real handlers with synthetic events.
import { invoke } from "@tauri-apps/api/core";

const wc = (window as any).wc;
const log = (m: string) => invoke("selftest", { msg: m });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const key = (key: string, o: KeyboardEventInit = {}) => window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, ...o }));
const until = async (f: () => boolean, ms = 5000) => { const t = performance.now(); while (!f()) { if (performance.now() - t > ms) return -1; await sleep(10); } return performance.now() - t; };
let fails = 0;
const check = (name: string, ok: boolean, extra = "") => { if (!ok) fails++; log(`${ok ? "PASS" : "FAIL"} ${name} ${extra}`); };

(async () => {
  const base: string = await invoke("selftest_dir");
  await sleep(500);

  const t0 = performance.now();
  wc.navigate(`${base}/big`);
  const t = await until(() => wc.rows.length === 10300);
  check("10k listing", t >= 0, `${Math.round(performance.now() - t0)}ms`);
  check("folders first + natural order", wc.rows[0].e.name === "folder 0" && wc.rows[1].e.name === "folder 1" && wc.rows[300].e.name === "file 0.txt" && wc.rows[302].e.name === "file 2.rs");
  wc.pane.scrollTop = 150000; await sleep(50);
  check("virtualized DOM", document.querySelectorAll(".row").length < 80, `${document.querySelectorAll(".row").length} nodes`);

  wc.pane.scrollTop = 0; await sleep(30);
  key("ArrowDown"); key("ArrowDown"); key("ArrowDown", { shiftKey: true }); key("ArrowDown", { shiftKey: true });
  check("arrow + shift select", wc.sel.size === 3, `sel=${wc.sel.size}`);
  key("a", { ctrlKey: true });
  check("ctrl+a", wc.sel.size === 10300);
  key("Escape");
  check("escape clears", wc.sel.size === 0);

  wc.filterEl.value = "file 99"; wc.filterEl.dispatchEvent(new Event("input"));
  check("filter", wc.rows.length === 111, `rows=${wc.rows.length}`);
  wc.filterEl.value = ""; wc.filterEl.dispatchEvent(new Event("input"));

  // rubber band from the whitespace of row 5 down to row 9
  const pr = wc.pane.getBoundingClientRect();
  const y = (i: number) => pr.top + 8 + i * 36 + 20;
  const x = pr.right - 400;
  document.querySelector(`.row[data-i="5"] .body`)!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, clientX: x, clientY: y(5), button: 0 }));
  window.dispatchEvent(new MouseEvent("mousemove", { clientX: x + 30, clientY: y(9) }));
  window.dispatchEvent(new MouseEvent("mouseup", { clientX: x + 30, clientY: y(9) }));
  check("rubber band", wc.sel.size === 5, `sel=${wc.sel.size}`);

  wc.navigate(`${base}/play`);
  await until(() => wc.rows.some((r: any) => r.e.name === "sub dir"));
  const names = wc.rows.map((r: any) => r.e.name);
  check("odd names listed", ["new\nline", "ünïcode 💾.pdf", "dangling", "pipe"].every((n) => names.includes(n)));
  check("b 9 before b 10", names.indexOf("b 9.txt") < names.indexOf("b 10.txt"));
  key("h", { ctrlKey: true }); await sleep(30);
  check("ctrl+h shows hidden", wc.rows.some((r: any) => r.e.name === ".secret"));
  const i = wc.rows.findIndex((r: any) => r.e.name === "sub dir");
  document.querySelector(`.row[data-i="${i}"] .chev`)!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
  check("pull out folder", (await until(() => wc.rows.some((r: any) => r.e.name === "x.png"))) >= 0);
  await sleep(250);
  log("SHOT expanded");
  await sleep(1500);

  const n = wc.rows.length;
  log("TOUCH");
  check("live refresh", (await until(() => wc.rows.length === n + 2, 3000)) >= 0, `rows ${n} -> ${wc.rows.length}`);

  key("ArrowUp", { altKey: true });
  check("up selects previous folder", (await until(() => wc.sel.size === 1 && [...wc.sel][0] === `${base}/play`)) >= 0);
  log(`DONE fails=${fails}`);
})();
