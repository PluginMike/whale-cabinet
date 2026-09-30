import { invoke } from "./util";
import { initTheme } from "./theme";

const q = new URLSearchParams(location.search);
const dialog = q.get("dialog");

if (dialog) {
  initTheme();
  import("./dialog").then((m) => m.run(dialog, q.get("arg") ?? ""));
} else {
  // isolated test runs keep "WCTEST" in window titles (see src-tauri isolated())
  if (import.meta.env.DEV) (window as any).WCTEST = await invoke<boolean>("selftest", {});
  const app = await import("./app");
  await import("./ops");
  await import("./preview");
  await import("./tags");
  await import("./menu");
  await import("./sidebar");
  await import("./search");
  await import("./term");
  await import("./nav");
  await import("./quickopen");
  await import("./git");
  await import("./recent");
  await import("./network");
  await initTheme(app.applySettings);
  // extra windows get their targets in the URL; the first one reads argv
  const targets: { loc: string; select: string | null }[] = q.has("targets") ? JSON.parse(q.get("targets")!) : await invoke("start_args");
  await app.start(targets.map((t) => ({ loc: t.loc, select: t.select ?? undefined })));
  if (import.meta.env.DEV && !q.has("targets") && (await invoke<boolean>("selftest", {}))) import("./selftest");
}
