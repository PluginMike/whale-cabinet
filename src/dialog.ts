// Small separate windows (class whale-cabinet-dialog): settings, properties, open-with, conflicts.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { $, esc, invoke } from "./util";
import { Settings } from "./theme";

export const win = getCurrentWindow();
export const close = () => win.close();

/** Frame a dialog: compact draggable titlebar + body. */
export function frame(title: string, body: string) {
  const d = $("dialog");
  d.hidden = false;
  d.innerHTML = `<div class="dlg-title" data-tauri-drag-region><span data-tauri-drag-region>${esc(title)}</span><button class="dlg-x" title="Close (Esc)">✕</button></div><div class="dlg-body">${body}</div>`;
  d.querySelector<HTMLElement>(".dlg-x")!.onclick = close;
  window.addEventListener("keydown", (e) => { if (e.key === "Escape") close(); });
  return d.querySelector<HTMLElement>(".dlg-body")!;
}

const handlers: Record<string, (arg: string) => void | Promise<void>> = {
  settings: settingsDialog,
};
export function register(kind: string, f: (arg: string) => void | Promise<void>) { handlers[kind] = f; }

export async function run(kind: string, arg: string) {
  document.body.classList.add("is-dialog");
  // feature dialogs live with their features
  await Promise.all([import("./dialogs")]);
  await handlers[kind]?.(arg);
}

async function settingsDialog() {
  const s = await invoke<Settings>("get_settings");
  const radio = (name: string, v: string, label: string) => `<label><input type="radio" name="${name}" value="${v}" ${s[name] === v ? "checked" : ""}> ${label}</label>`;
  const body = frame("Settings", `
    <form id="sf">
      <fieldset><legend>Theme</legend>
        ${radio("theme", "dms", "Follow DMS")} ${radio("theme", "builtin", "Built-in cabinet")} ${radio("theme", "custom", "Custom")}
        <div class="row2"><label>Custom colour <input type="color" name="customColor" value="${esc(s.customColor)}"></label>
        <label><input type="checkbox" name="customDark" ${s.customDark ? "checked" : ""}> Dark</label></div>
      </fieldset>
      <fieldset><legend>Window</legend>
        <label>Background opacity <input type="range" name="opacity" min="0.5" max="1" step="0.01" value="${s.opacity}"> <output>${Math.round(s.opacity * 100)}%</output></label>
        <label><input type="checkbox" name="titlebar" ${s.titlebar ? "checked" : ""}> Compact titlebar (for floating mode)</label>
      </fieldset>
      <fieldset><legend>Browsing</legend>
        <label>Default view <select name="view">${["cabinet", "compact", "grid"].map((v) => `<option ${s.view === v ? "selected" : ""} value="${v}">${v === "grid" ? "Icons" : v[0].toUpperCase() + v.slice(1)}</option>`).join("")}</select></label>
        <label><input type="checkbox" name="showHidden" ${s.showHidden ? "checked" : ""}> Show hidden files by default</label>
        <label>Item size <input type="range" name="zoom" min="0.6" max="2" step="0.05" value="${s.zoom}"> <output>${Math.round(s.zoom * 100)}%</output></label>
        <div class="mini-h">Double-clicking a folder</div>
        ${radio("folderDblClick", "open", "Opens it (Windows style)")} ${radio("folderDblClick", "expand", "Pulls it out inline (cabinet style)")}
        <label><input type="checkbox" name="singleClick" ${s.singleClick ? "checked" : ""}> Single click opens items</label>
        <label>Terminal <input name="terminal" placeholder="auto (kitty, foot, ghostty, alacritty…)" value="${esc(s.terminal ?? "")}"></label>
      </fieldset>
      <p class="hint">Opacity below 100% only shows when Hyprland blur is on. Hyprland rules for this window: <code>class:^(whale-cabinet)$</code>, dialogs: <code>class:^(whale-cabinet-dialog)$</code>.</p>
    </form>`);
  const form = body.querySelector<HTMLFormElement>("#sf")!;
  form.addEventListener("input", (e) => {
    const t = e.target as HTMLInputElement;
    const v = t.type === "checkbox" ? t.checked : t.type === "range" ? +t.value : t.value;
    if (t.type === "range") t.nextElementSibling!.textContent = `${Math.round(+t.value * 100)}%`;
    invoke("set_settings", { patch: { [t.name]: v } });
  });
}
