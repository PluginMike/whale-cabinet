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
        <div class="mini-h">When Whale Cabinet starts</div>
        ${radio("restoreSession", "ask", "Ask to restore the last windows and tabs")} ${radio("restoreSession", "always", "Always restore them")} ${radio("restoreSession", "never", "Start fresh")}
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
  body.append(await pluginSettings());
}

/** Settings → Plugins: allow or stop each one, and its own settings (secrets go to the keyring). */
async function pluginSettings() {
  type P = { name: string; title: string; description: string; consent: boolean | null; settings: { key: string; label: string; secret: boolean; placeholder: string }[] };
  const list = await invoke<P[]>("plugins_list").catch(() => [] as P[]);
  const box = document.createElement("fieldset");
  box.innerHTML = `<legend>Plugins</legend>` + (list.length ? "" : `<p class="hint">None installed.</p>`) +
    `<p class="hint">Plugins live in <code>~/.local/share/whale-cabinet/plugins/</code> and run as you. Changes restart the plugin.</p>`;
  for (const p of list) {
    const vals = p.consent ? await invoke<Record<string, string | boolean>>("plugin_settings", { name: p.name }).catch(() => ({} as Record<string, string | boolean>)) : {};
    const d = document.createElement("div");
    d.className = "plugin-set";
    d.innerHTML = `<label><input type="checkbox" data-consent ${p.consent ? "checked" : ""}> <b>${esc(p.title)}</b></label>
      ${p.description ? `<p class="hint">${esc(p.description)}</p>` : ""}
      ${p.settings.map((s) => s.secret
        ? `<label>${esc(s.label)} <input type="password" data-key="${esc(s.key)}" autocomplete="off" placeholder="${vals[s.key] ? "saved in your keyring (type to replace)" : esc(s.placeholder || "not set")}"></label>`
        : `<label>${esc(s.label)} <input data-key="${esc(s.key)}" spellcheck="false" placeholder="${esc(s.placeholder)}" value="${esc(String(vals[s.key] ?? ""))}"></label>`).join("")}`;
    d.querySelectorAll<HTMLInputElement>("[data-key]").forEach((i) => (i.disabled = !p.consent));
    d.addEventListener("change", async (e) => {
      const t = e.target as HTMLInputElement;
      try {
        if (t.dataset.consent !== undefined) { await invoke("plugin_consent", { name: p.name, allow: t.checked }); d.querySelectorAll<HTMLInputElement>("[data-key]").forEach((i) => (i.disabled = !t.checked)); }
        else if (t.dataset.key) {
          await invoke("plugin_set", { name: p.name, key: t.dataset.key, value: t.value.trim() });
          if (t.type === "password") { t.placeholder = t.value ? "saved in your keyring (type to replace)" : "not set"; t.value = ""; }
        }
        d.querySelector(".err")?.remove();
      } catch (err) { d.querySelector(".err")?.remove(); d.insertAdjacentHTML("beforeend", `<p class="err">${esc(String(err))}</p>`); }
    });
    box.append(d);
  }
  return box;
}
