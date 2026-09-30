// Tags, colours and ratings (Dolphin-compatible xattrs): sidebar tag list, tag views, info-panel editor,
// drag-onto-tag. Colours are `color:<preset|#hex>` tags shown as folder tab / paper edge colours.
import { listen } from "@tauri-apps/api/event";
import { $, Entry, invoke, esc, TAG_COLORS, tagDefs, tagColor, tagLabel, cssColor } from "./util";
import { hooks, pane, allPanes, flash } from "./app";
import { showMenu, Item } from "./menu";

let known: [string, number][] = [];
const label = tagLabel;
const swatch = (t: string) => (t.startsWith("color:") ? cssColor(t.slice(6)) ?? "var(--muted)" : tagColor(t) ?? "var(--accent)");

// ---------- definitions (settings.tagDefs): colour per tag, your names for the nine colours ----------
function loadDefs(s: Record<string, any>) {
  tagDefs.colors = { ...(s.tagDefs?.colors ?? {}) };
  tagDefs.names = { ...(s.tagDefs?.names ?? {}) };
}
export function saveDefs() {
  invoke("set_settings", { patch: { tagDefs: { colors: tagDefs.colors, names: tagDefs.names } } });
}
listen<Record<string, any>>("settings", ({ payload }) => { loadDefs(payload); refreshTags(); allPanes().forEach((p) => p.render()); });

/** Every plain tag in use or defined, sorted. */
export const plainTags = () => [...new Set([...known.map(([t]) => t).filter((t) => !t.startsWith("color:")), ...Object.keys(tagDefs.colors)])].sort((a, b) => a.localeCompare(b));

export async function refreshTags() {
  known = await invoke<[string, number][]>("tag_counts").catch(() => []);
  const cur = pane()?.loc;
  const count = new Map(known);
  // plain tags (used or defined) first, then colours in use
  const rows: [string, number][] = [...plainTags().map((t) => [t, count.get(t) ?? 0] as [string, number]), ...known.filter(([t]) => t.startsWith("color:")).sort((a, b) => a[0].localeCompare(b[0]))];
  $("tag-head").hidden = false;
  $("tags").innerHTML = rows.map(([t, n]) => {
    const loc = `tag:${t}`;
    return `<div class="tagrow${cur === loc ? " active" : ""}" data-tag="${esc(t)}" data-loc="${esc(loc)}"><i style="--tagc:${swatch(t)}"></i>${esc(label(t))}<b>${n || ""}</b></div>`;
  }).join("") + `<div class="recrow tool" id="tag-manage"><i class="tool-ico"></i><span>Manage tags…</span></div>`;
  let dl = document.getElementById("wc-tags") as HTMLDataListElement | null;
  if (!dl) { dl = document.createElement("datalist"); dl.id = "wc-tags"; document.body.append(dl); }
  dl.innerHTML = plainTags().map((t) => `<option value="${esc(t)}">`).join("");
}

$("tags").addEventListener("click", (ev) => {
  if ((ev.target as HTMLElement).closest("#tag-manage")) { manageTags(); return; }
  const r = (ev.target as HTMLElement).closest<HTMLElement>(".tagrow"); if (r) pane().navigate(r.dataset.loc!);
});
$("tags").addEventListener("contextmenu", (ev) => {
  const r = (ev.target as HTMLElement).closest<HTMLElement>(".tagrow"); if (!r) return;
  ev.preventDefault();
  const t = r.dataset.tag!, isColor = t.startsWith("color:");
  showMenu(ev.clientX, ev.clientY, [
    { label: "Show Tagged Items", act: () => pane().navigate(r.dataset.loc!) },
    "-",
    { label: isColor ? "Rename Colour…" : "Rename Tag…", act: () => manageTags(t) },
    ...(isColor ? [] : [{ label: "Colour", sub: () => colourChoices((c) => { setColor(t, c); }) } as Item]),
    { label: isColor ? "Remove Colour From All Items" : "Delete Tag", danger: true, act: () => deleteTag(t) },
    "-",
    { label: "Manage Tags…", act: () => manageTags() },
  ]);
});

function setColor(t: string, c: string | null) {
  if (c) tagDefs.colors[t] = c; else delete tagDefs.colors[t];
  saveDefs();
}
/** Menu entries for the nine colours (with your names), a custom one and none. */
export function colourChoices(pick: (c: string | null) => void): Item[] {
  return [
    ...TAG_COLORS.map((c) => ({ label: tagLabel(`color:${c}`), dot: `var(--t-${c})`, act: () => pick(c) })),
    "-",
    { label: "Custom…", act: () => { const i = document.createElement("input"); i.type = "color"; i.onchange = () => pick(i.value); i.click(); } },
    { label: "No Colour", act: () => pick(null) },
  ];
}

/** Tag everything that has `from` with `to` instead (the index covers your home and what you've browsed). */
async function renameTag(from: string, to: string) {
  to = to.trim();
  if (!to || to === from || to.startsWith("color:")) return;
  const items = await invoke<Entry[]>("tag_items", { tag: from }).catch(() => [] as Entry[]);
  if (items.length) await editTags(items.map((e) => e.path), [to], [from]);
  if (from in tagDefs.colors) { tagDefs.colors[to] ??= tagDefs.colors[from]; delete tagDefs.colors[from]; saveDefs(); }
  flash(`Renamed “${from}” to “${to}”${items.length ? ` on ${items.length} item${items.length === 1 ? "" : "s"}` : ""}`);
}
async function deleteTag(t: string) {
  const items = await invoke<Entry[]>("tag_items", { tag: t }).catch(() => [] as Entry[]);
  const { confirmBox } = await import("./ops");
  if (items.length && !(await confirmBox(`Remove “${label(t)}” from ${items.length} item${items.length === 1 ? "" : "s"}? The files stay.`, "Remove", true))) return;
  if (items.length) await editTags(items.map((e) => e.path), [], [t]);
  if (t in tagDefs.colors) { delete tagDefs.colors[t]; saveDefs(); }
  if (pane().loc === `tag:${t}`) pane().goBack();
  refreshTags();
}

// ---------- New Tag… (right-click → Tags) ----------
export function newTag(paths: string[]) {
  const m = document.createElement("div");
  m.className = "modal";
  let color: string | null = null;
  m.innerHTML = `<div class="modal-box wide"><h3>New tag</h3>
    <label class="full">Name <input id="nt-name" spellcheck="false" list="wc-tags" placeholder="e.g. Work, Invoices, To read"></label>
    <div class="mini">Colour</div>
    <div class="swatches big">${TAG_COLORS.map((c) => `<button data-c="${c}" title="${esc(tagLabel(`color:${c}`))}" style="background:var(--t-${c})"></button>`).join("")}
      <input type="color" id="nt-custom" title="Custom colour" value="#888888"><button data-c="" class="none on" title="No colour"></button></div>
    <div class="btns"><button data-v="0">Cancel</button><button data-v="1" class="primary">${paths.length ? `Tag ${paths.length} item${paths.length === 1 ? "" : "s"}` : "Create"}</button></div></div>`;
  document.body.append(m);
  const name = m.querySelector<HTMLInputElement>("#nt-name")!;
  const mark = (el: Element | null) => m.querySelectorAll(".swatches > *").forEach((x) => x.classList.toggle("on", x === el));
  const close = () => { m.remove(); window.removeEventListener("keydown", key, true); pane().scroller.focus(); };
  const go = async () => {
    const t = name.value.trim();
    if (!t || t.startsWith("color:")) { name.focus(); return; }
    if (color) { tagDefs.colors[t] = color; saveDefs(); }
    close();
    if (paths.length) await editTags(paths, [t], []); else refreshTags();
  };
  const key = (e: KeyboardEvent) => { e.stopPropagation(); if (e.key === "Escape") close(); if (e.key === "Enter") { e.preventDefault(); go(); } };
  window.addEventListener("keydown", key, true);
  m.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    if (t === m || t.dataset.v === "0") close();
    else if (t.dataset.v === "1") go();
    else if (t.dataset.c !== undefined) { color = t.dataset.c || null; mark(t); }
  });
  m.querySelector<HTMLInputElement>("#nt-custom")!.addEventListener("input", (e) => { color = (e.target as HTMLInputElement).value; mark(e.target as Element); });
  name.focus();
}

// ---------- Manage tags ----------
export async function manageTags(focus?: string) {
  await refreshTags();
  document.querySelector(".modal.tagmgr")?.remove();
  const count = new Map(known);
  const m = document.createElement("div");
  m.className = "modal tagmgr";
  const colourCell = (c?: string) => `<span class="tm-sw" style="background:${cssColor(c) ?? "transparent"}"></span>`;
  m.innerHTML = `<div class="modal-box wide"><h3>Tags</h3>
    <div class="tm-list">${plainTags().map((t) => `<div class="tm-row" data-t="${esc(t)}">
        <button class="tm-col" title="Colour">${colourCell(tagDefs.colors[t])}</button>
        <input class="tm-name" value="${esc(t)}" spellcheck="false"><span class="tm-n">${count.get(t) ?? 0}</span>
        <button class="tm-del" title="Delete tag">✕</button></div>`).join("") || `<p class="hint">No tags yet.</p>`}</div>
    <p><button id="tm-new">+ New tag…</button></p>
    <div class="mini">Colour names</div>
    <div class="tm-colors">${TAG_COLORS.map((c) => `<label><span class="tm-sw" style="background:var(--t-${c})"></span><input data-c="${c}" value="${esc(tagDefs.names[c] ?? "")}" placeholder="${c[0].toUpperCase() + c.slice(1)}" spellcheck="false"></label>`).join("")}</div>
    <p class="hint">Tags are stored on the files (user.xdg.tags, shared with Dolphin); colours and colour names are Whale Cabinet's own. Renaming or deleting changes every indexed item with that tag.</p>
    <div class="btns"><button data-v="0" class="primary">Done</button></div></div>`;
  document.body.append(m);
  const close = () => { m.remove(); window.removeEventListener("keydown", key, true); pane().scroller.focus(); };
  const key = (e: KeyboardEvent) => { e.stopPropagation(); if (e.key === "Escape") close(); if (e.key === "Enter") (e.target as HTMLElement).blur?.(); };
  window.addEventListener("keydown", key, true);
  m.addEventListener("click", async (e) => {
    const t = e.target as HTMLElement, row = t.closest<HTMLElement>(".tm-row");
    if (t === m || t.dataset.v === "0") close();
    else if (t.id === "tm-new") { close(); newTag([]); }
    else if (row && t.closest(".tm-del")) { close(); await deleteTag(row.dataset.t!); manageTags(); }
    else if (row && t.closest(".tm-col")) {
      const r = t.closest(".tm-col")!.getBoundingClientRect();
      showMenu(r.left, r.bottom, colourChoices((c) => { setColor(row.dataset.t!, c); row.querySelector(".tm-col")!.innerHTML = colourCell(c ?? undefined); }));
    }
  });
  m.addEventListener("change", async (e) => {
    const t = e.target as HTMLInputElement;
    if (t.classList.contains("tm-name")) { const row = t.closest<HTMLElement>(".tm-row")!; await renameTag(row.dataset.t!, t.value); row.dataset.t = t.value.trim() || row.dataset.t; }
    if (t.dataset.c) { if (t.value.trim()) tagDefs.names[t.dataset.c] = t.value.trim(); else delete tagDefs.names[t.dataset.c]; saveDefs(); }
  });
  const f = focus?.startsWith("color:") ? m.querySelector<HTMLInputElement>(`.tm-colors input[data-c="${focus.slice(6)}"]`) : focus ? m.querySelector<HTMLInputElement>(`.tm-row[data-t="${CSS.escape(focus)}"] .tm-name`) : null;
  f?.focus(); f?.select();
}

hooks.virtual!["tag:"] = (loc) => invoke<Entry[]>("tag_items", { tag: loc.slice(4) });
hooks.locTitle!["tag:"] = (loc) => `Tag: ${label(loc.slice(4))}`;
const prevNav = hooks.onNavigate;
hooks.onNavigate = (p) => { prevNav?.(p); refreshTags(); };
listen("tags-changed", refreshTags);

export async function editTags(paths: string[], add: string[], remove: string[]) {
  if (!paths.length) return;
  try {
    await invoke("tags_edit", { paths, add, remove });
  } catch (e) { flash(String(e)); }
  // xattr changes also arrive via inotify, but don't wait for that
  for (const p of allPanes()) if (p.selected().some((e) => paths.includes(e.path)) || p.loc.startsWith("tag:")) p.reload();
  refreshTags();
}
hooks.tagDrop = (tag, paths) => editTags(paths, [tag], []);

// ---------- info-panel editor (one or many items) ----------
hooks.infoExtra.push((el, es) => {
  if (!es.length || es.some((e) => e.trashId)) return;
  const paths = es.map((e) => e.path);
  const all = (t: string) => es.every((e) => e.tags?.includes(t));
  const common = [...new Set(es.flatMap((e) => e.tags ?? []))].filter(all);
  const plain = common.filter((t) => !t.startsWith("color:"));
  const color = common.find((t) => t.startsWith("color:"))?.slice(6);
  const box = document.createElement("div");
  box.className = "tagbox";
  box.innerHTML = `
    <div class="mini">Tags</div>
    <div class="tag-editor">${plain.map((t) => `<span class="chip"${tagColor(t) ? ` style="--cc:${tagColor(t)}"` : ""}>${esc(t)}<b data-rm="${esc(t)}" title="Remove">✕</b></span>`).join("")}
      <input list="wc-tags" placeholder="Add tag…" spellcheck="false"></div>
    <div class="mini">Colour</div>
    <div class="swatches">${TAG_COLORS.map((c) => `<button data-c="${c}" title="${esc(tagLabel(`color:${c}`))}" class="${color === c ? "on" : ""}" style="background:var(--t-${c})"></button>`).join("")}
      <input type="color" title="Custom colour" value="${color?.startsWith("#") ? color : "#888888"}" style="width:26px;height:22px;padding:0;border:0">
      ${color ? `<button data-c="" title="No colour" style="background:transparent;border:1px dashed var(--divider)!important"></button>` : ""}</div>
    ${es.length === 1 ? `<div class="mini">Rating</div><div class="stars" title="Click a star; click it again to clear"></div>` : ""}`;
  el.append(box);
  const input = box.querySelector<HTMLInputElement>(".tag-editor input")!;
  input.addEventListener("keydown", (ev) => {
    ev.stopPropagation();
    if (ev.key === "Enter" && input.value.trim()) { editTags(paths, [input.value.trim()], []); input.value = ""; }
    if (ev.key === "Escape") { input.blur(); pane().scroller.focus(); }
  });
  input.addEventListener("change", () => { if (plainTags().includes(input.value)) { editTags(paths, [input.value], []); input.value = ""; } });
  box.addEventListener("click", (ev) => {
    const t = ev.target as HTMLElement;
    if (t.dataset.rm) editTags(paths, [], [t.dataset.rm]);
    if (t.dataset.c !== undefined && t.tagName === "BUTTON") {
      const old = es.flatMap((e) => e.tags ?? []).filter((x) => x.startsWith("color:"));
      editTags(paths, t.dataset.c ? [`color:${t.dataset.c}`] : [], t.dataset.c ? [] : old);
    }
  });
  box.querySelector<HTMLInputElement>("input[type=color]")!.addEventListener("change", (ev) => editTags(paths, [`color:${(ev.target as HTMLInputElement).value}`], []));
  const stars = box.querySelector<HTMLElement>(".stars");
  if (stars) {
    const path = paths[0];
    const draw = (r: number) => { stars.innerHTML = [1, 2, 3, 4, 5].map((i) => `<span data-r="${i * 2}" class="${r >= i * 2 ? "on" : ""}">${r >= i * 2 ? "★" : r === i * 2 - 1 ? "⯪" : "☆"}</span>`).join(""); };
    let rating = 0;
    invoke<{ tags: string[]; rating: number }>("tag_meta", { path }).then((m) => { rating = m.rating; draw(rating); });
    stars.addEventListener("click", (ev) => {
      const r = +((ev.target as HTMLElement).dataset.r ?? 0); if (!r) return;
      rating = rating === r ? 0 : r; // Dolphin scale: 0–10, 2 per star
      draw(rating);
      invoke("set_rating", { path, rating }).catch((e) => flash(String(e)));
    });
  }
});

invoke<Record<string, any>>("get_settings").then((s) => { loadDefs(s); refreshTags(); allPanes().forEach((p) => p.render()); });
