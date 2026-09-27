// Applies the desktop theme (DMS palette + Hyprland shape + font) to the cabinet's CSS variables.
import { listen } from "@tauri-apps/api/event";
import { invoke } from "./util";

export type Theme = {
  source: "dms" | "builtin" | "custom"; dark: boolean;
  roles: Record<string, string>; tags: Record<string, string>;
  hypr: { running: boolean; rounding: number; gaps_in: number; gaps_out: number; border_size: number; border_colors: string[]; border_angle: number; blur: boolean };
  font_family: string; font_size: number; icon_theme: string; terminal: string[];
};
export type Settings = Record<string, any>;

function lum(hex: string) {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}
const contrast = (a: string, b: string) => { const [x, y] = [lum(a), lum(b)]; return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
/** Keep text readable: use the preferred colour if it contrasts enough, else near-black/near-white by luminance. */
export function pickText(bg: string, preferred?: string) {
  if (!/^#[0-9a-f]{6}/i.test(bg)) return preferred ?? "#000000";
  if (preferred && /^#[0-9a-f]{6}/i.test(preferred) && contrast(bg, preferred) >= 4.5) return preferred;
  return contrast(bg, "#111111") >= contrast(bg, "#f5f5f5") ? "#111111" : "#f5f5f5";
}

// cabinet variable → Material role(s), first found wins
const MAP: Record<string, string[]> = {
  interior: ["surface_container_lowest", "background", "surface"],
  body: ["surface_container", "surface_container_low", "surface"],
  front: ["surface_container_high", "surface_container", "surface"],
  holder: ["surface_container_highest", "surface_bright", "surface_container_high"],
  accent: ["primary"],
  tab: ["primary_container", "primary"],
  paper: ["surface_variant", "surface_container_highest"],
  divider: ["outline", "outline_variant"],
  danger: ["error"],
  bar: ["surface_container_low", "surface_container", "surface"],
  panel: ["surface_container_low", "surface_container", "surface"],
};
const TRANSLUCENT = ["interior", "body", "bar", "panel"];
const KIND_TAG: Record<string, string> = { img: "teal", vid: "purple", aud: "pink", arc: "orange", code: "blue", doc: "green", other: "grey" };
let setVars: string[] = [];

export function applyTheme(t: Theme, s: Settings) {
  const st = document.documentElement.style;
  for (const v of setVars) st.removeProperty(v);
  setVars = [];
  const set = (k: string, v: string) => { st.setProperty(k, v); setVars.push(k); };
  const r = t.roles;
  const pick = (roles: string[]) => roles.map((x) => r[x]).find(Boolean);
  const alpha = t.hypr.blur || !t.hypr.running ? Math.max(0.3, Math.min(1, +s.opacity || 1)) : 1;

  if (t.source !== "builtin" && r.primary) {
    const val: Record<string, string> = {};
    for (const [k, roles] of Object.entries(MAP)) { const c = pick(roles); if (c) val[k] = c; }
    val["on-accent"] = pickText(val.accent, r.on_primary);
    val["on-holder"] = pickText(val.holder, r.on_surface);
    val["on-paper"] = pickText(val.paper, r.on_surface_variant ?? r.on_surface);
    val["on-body"] = pickText(val.body, r.on_surface);
    val.text = pickText(val.interior, r.on_surface);
    val.muted = pickText(val.interior, r.on_surface_variant) === r.on_surface_variant ? r.on_surface_variant : `color-mix(in srgb, ${val.text} 65%, ${val.interior})`;
    val.folder = `color-mix(in srgb, ${val.front} 78%, ${val.tab})`;
    val["on-tab"] = pickText(val.tab, r.on_primary_container);
    for (const [k, v] of Object.entries(val)) set(`--${k}`, TRANSLUCENT.includes(k) && alpha < 1 ? `color-mix(in srgb, ${v} ${Math.round(alpha * 100)}%, transparent)` : v);
    for (const [name, c] of Object.entries(t.tags)) set(`--t-${name}`, c);
    for (const [k, tag] of Object.entries(KIND_TAG)) set(`--k-${k}`, t.tags[tag]);
    set("color-scheme", t.dark ? "dark" : "light");
  } else if (alpha < 1) {
    const cs = getComputedStyle(document.documentElement);
    for (const k of TRANSLUCENT) set(`--${k}`, `color-mix(in srgb, ${cs.getPropertyValue(`--${k}`).trim()} ${Math.round(alpha * 100)}%, transparent)`);
  }
  document.documentElement.style.colorScheme = t.source === "builtin" ? "light" : t.dark ? "dark" : "light";

  // Shape from Hyprland
  const h = t.hypr;
  if (h.running) {
    set("--radius", `${Math.min(h.rounding, 18)}px`);
    set("--gap", `${Math.max(2, Math.min(h.gaps_in, 16))}px`);
    set("--pad", `${Math.max(4, Math.min(h.gaps_out, 24))}px`);
    set("--ring-w", `${Math.max(1, Math.min(h.border_size, 4))}px`);
    if (h.border_colors.length) {
      set("--ring", h.border_colors[0]);
      set("--ring-img", h.border_colors.length > 1 ? `linear-gradient(${h.border_angle + 90}deg, ${h.border_colors.join(", ")})` : "none");
    }
  }
  if (t.font_family) {
    set("--font", `"${t.font_family}", system-ui, sans-serif`);
    set("--fs", `${Math.round(t.font_size * 4 / 3 * 10) / 10}px`); // pt → px
  }
  document.documentElement.dataset.theme = t.source;
}

/** Load settings + theme, apply, and keep both live. Calls `onSettings` whenever settings change. */
export async function initTheme(onSettings?: (s: Settings) => void) {
  let settings = await invoke<Settings>("get_settings");
  let theme = await invoke<Theme>("get_theme");
  applyTheme(theme, settings);
  onSettings?.(settings);
  listen<Theme>("theme", ({ payload }) => { theme = payload; applyTheme(theme, settings); });
  listen<Settings>("settings", ({ payload }) => { settings = payload; applyTheme(theme, settings); onSettings?.(settings); });
  return settings;
}
