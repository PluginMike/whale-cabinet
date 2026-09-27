// Sidebar: Places from ~/.local/share/user-places.xbel (shared with Dolphin and file dialogs) — add, remove,
// rename, drag to reorder — and Devices (udisks2 volumes + network mounts) with mount/unmount/eject.
import { listen } from "@tauri-apps/api/event";
import { $, invoke, esc, fmtSize } from "./util";
import { pane, allPanes, flash, HOME, newTab, renderPlaces, hooks } from "./app";
import { showMenu, openProps } from "./menu";

type Place = { href: string; title: string; path: string; icon: string; hidden: boolean; system: boolean };
type Device = { id: string; label: string; path: string; size: number; kind: string; can_eject: boolean; disk: string };
let places: Place[] = [];
let devices: Device[] = [];

// ---------- places ----------
export async function loadPlaces() {
  places = await invoke<Place[]>("places_list").catch(() => []);
  const shown = places.filter((p) => !p.hidden && p.path);
  if (!shown.some((p) => p.path === "trash:/")) shown.push({ href: "trash:/", title: "Trash", path: "trash:/", icon: "", hidden: false, system: true });
  renderPlaces(shown.map((p) => ({ name: p.title || p.path, path: p.path })));
  document.querySelectorAll<HTMLElement>("#places .drawer").forEach((d, i) => (d.dataset.href = shown[i].href));
  $("places").insertAdjacentHTML("beforeend", `<button class="sb-add" title="Drag a folder here, or right-click a folder → Add to Places">+ Drop a folder here</button>`);
}
listen("places-changed", loadPlaces);
hooks.addPlace = (p) => addPlace(p);

export async function addPlace(path: string) {
  await invoke("places_add", { path, title: path === "/" ? "Root" : path.split("/").pop(), index: null }).catch((e) => flash(String(e)));
  loadPlaces();
}

$("places").addEventListener("contextmenu", (ev) => {
  const d = (ev.target as HTMLElement).closest<HTMLElement>(".drawer[data-href]"); if (!d) return;
  ev.preventDefault();
  const href = d.dataset.href!, path = d.dataset.p!;
  const pl = places.find((p) => p.href === href);
  showMenu(ev.clientX, ev.clientY, [
    { label: "Open in New Tab", act: () => newTab(path, false) },
    "-",
    { label: "Rename Entry…", off: !pl, act: async () => {
      const label = d.querySelector<HTMLElement>(".label")!;
      const input = document.createElement("input");
      input.className = "rename"; input.value = pl!.title; label.replaceWith(input); input.focus(); input.select();
      const done = async (save: boolean) => { if (save && input.value.trim()) await invoke("places_rename", { href, title: input.value.trim() }); loadPlaces(); };
      input.addEventListener("keydown", (e) => { e.stopPropagation(); if (e.key === "Enter") done(true); if (e.key === "Escape") done(false); });
      input.addEventListener("blur", () => done(true));
    } },
    { label: "Remove from Places", off: !pl, danger: true, act: async () => { await invoke("places_remove", { href }); loadPlaces(); } },
    "-",
    { label: "Properties", off: path === "trash:/", act: () => openProps([path]) },
  ]);
});
$("places").addEventListener("click", (ev) => { if ((ev.target as HTMLElement).closest(".sb-add")) flash("Drag a folder onto this spot to add it to Places"); });

// drag a drawer to reorder
$("places").addEventListener("mousedown", (ev) => {
  const d = (ev.target as HTMLElement).closest<HTMLElement>(".drawer[data-href]");
  if (!d || ev.button !== 0) return;
  const y0 = ev.clientY; let moving = false;
  const move = (m: MouseEvent) => {
    if (!moving && Math.abs(m.clientY - y0) < 6) return;
    moving = true; d.style.opacity = ".5";
    document.querySelectorAll("#places .drawer").forEach((x) => x.classList.toggle("dragover", x !== d && x.contains(document.elementFromPoint(m.clientX, m.clientY))));
  };
  const up = async (m: MouseEvent) => {
    window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up);
    d.style.opacity = "";
    document.querySelectorAll("#places .dragover").forEach((x) => x.classList.remove("dragover"));
    if (!moving) return;
    const t = (document.elementFromPoint(m.clientX, m.clientY) as HTMLElement | null)?.closest<HTMLElement>(".drawer[data-href]");
    if (!t || t === d) return;
    const to = places.findIndex((p) => p.href === t.dataset.href);
    if (to >= 0) { await invoke("places_move", { href: d.dataset.href, to }); loadPlaces(); }
    // swallow the click that follows the drag
    window.addEventListener("click", (c) => c.stopPropagation(), { capture: true, once: true });
  };
  window.addEventListener("mousemove", move); window.addEventListener("mouseup", up);
});

// ---------- devices ----------
async function renderDevices(list: Device[]) {
  devices = list;
  $("dev-head").hidden = !list.length;
  const spaces = await Promise.all(list.map((d) => (d.path ? invoke<[number, number] | null>("disk_space", { path: d.path }) : Promise.resolve(null))));
  $("devices").innerHTML = list.map((d, i) => {
    const s = spaces[i];
    const bar = s ? `<span class="dev-bar" title="${fmtSize(s[0])} free of ${fmtSize(s[1])}"><i style="width:${Math.round((1 - s[0] / s[1]) * 100)}%"></i></span>` : "";
    const act = d.path && (d.can_eject || d.kind === "network") ? `<button data-act="eject" title="${d.can_eject ? "Eject" : "Unmount"}">⏏</button>` : "";
    return `<div class="drawer dev${d.path ? "" : " unmounted"}" data-dev="${i}" ${d.path ? `data-p="${esc(d.path)}"` : ""} title="${esc(d.id)}${d.path ? " — " + esc(d.path) : " (not mounted)"}"><span class="label">${esc(d.label)}</span><span class="handle"></span>${bar}${act}</div>`;
  }).join("");
  const p = pane(); if (p) document.querySelectorAll<HTMLElement>("#devices .drawer").forEach((d) => d.classList.toggle("active", d.dataset.p === p.loc));
}
async function eject(d: Device) {
  // step out of the volume first so nothing keeps it busy
  for (const p of allPanes()) if (d.path && (p.loc === d.path || p.loc.startsWith(d.path + "/"))) await p.navigate(HOME);
  try {
    if (d.can_eject) await invoke("device_eject", { dev: d }); else await invoke("device_unmount", { dev: d });
    flash(`${d.label} can be removed`);
  } catch (e) { flash(String(e)); }
  renderDevices(await invoke<Device[]>("devices_list"));
}
$("devices").addEventListener("click", async (ev) => {
  const el = (ev.target as HTMLElement).closest<HTMLElement>(".drawer[data-dev]"); if (!el) return;
  ev.stopPropagation();
  const d = devices[+el.dataset.dev!];
  if ((ev.target as HTMLElement).closest("[data-act=eject]")) { eject(d); return; }
  if (!d.path) {
    try { const at = await invoke<string>("device_mount", { id: d.id }); pane().navigate(at); } catch (e) { flash(String(e)); }
    renderDevices(await invoke<Device[]>("devices_list"));
  }
}, true);
$("devices").addEventListener("contextmenu", (ev) => {
  const el = (ev.target as HTMLElement).closest<HTMLElement>(".drawer[data-dev]"); if (!el) return;
  ev.preventDefault();
  const d = devices[+el.dataset.dev!];
  showMenu(ev.clientX, ev.clientY, [
    { label: d.path ? "Open" : "Mount and Open", act: () => el.click() },
    { label: "Open in New Tab", off: !d.path, act: () => newTab(d.path, false) },
    "-",
    { label: "Unmount", off: !d.path, act: async () => { try { await invoke("device_unmount", { dev: d }); } catch (e) { flash(String(e)); } renderDevices(await invoke<Device[]>("devices_list")); } },
    { label: "Safely Remove", off: !d.can_eject, act: () => eject(d) },
    "-",
    { label: "Properties", off: !d.path, act: () => openProps([d.path]) },
  ]);
});
listen<Device[]>("devices", ({ payload }) => renderDevices(payload));

const prev = hooks.onNavigate;
hooks.onNavigate = (p) => { prev?.(p); document.querySelectorAll<HTMLElement>("#devices .drawer").forEach((d) => d.classList.toggle("active", d.dataset.p === p.loc)); };

loadPlaces();
invoke<Device[]>("devices_list").then(renderDevices);
