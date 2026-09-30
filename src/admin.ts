// Administrator actions in the context menu: Open as Administrator (the default app via pkexec) and
// Edit as Administrator (edit a copy with your normal app; each save is written back as root).
// Permission errors elsewhere offer "Retry as administrator" (ops.ts).
import { listen } from "@tauri-apps/api/event";
import { invoke, baseName, kindOf } from "./util";
import { hooks, flash } from "./app";

hooks.menuExtra.push((p, es) => {
  const one = es.length === 1 ? es[0] : null;
  if (!one || one.dir || one.trashId || p.loc === "trash:/") return [];
  const texty = ["code", "doc", "other"].includes(kindOf(one));
  return [{
    label: "Administrator", sub: () => [
      { label: "Open as Administrator", act: () => invoke("open_as_admin", { path: one.path }).catch((e) => flash(String(e))) },
      { label: "Edit as Administrator", off: !texty, act: () => invoke("edit_as_admin", { path: one.path }).then(() => flash(`Editing a copy of ${baseName(one.path)} — each save asks for your password`)).catch((e) => flash(String(e))) },
    ],
  }, "-"];
});

listen<[string, string | null]>("admin-saved", ({ payload: [path, err] }) => {
  flash(err ? `Not saved to ${path}: ${err}` : `Saved ${baseName(path)} as administrator`);
});
