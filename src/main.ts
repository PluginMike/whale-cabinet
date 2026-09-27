import { invoke } from "./util";
import { initTheme } from "./theme";

const q = new URLSearchParams(location.search);
const dialog = q.get("dialog");

if (dialog) {
  initTheme();
  import("./dialog").then((m) => m.run(dialog, q.get("arg") ?? ""));
} else {
  const app = await import("./app");
  await import("./ops");
  await import("./preview");
  await import("./tags");
  await initTheme(app.applySettings);
  await app.start([{ loc: await invoke<string>("start_path") }]);
  if (import.meta.env.DEV && (await invoke<boolean>("selftest", {}))) import("./selftest");
}
