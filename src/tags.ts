// Tags, colours and ratings (Dolphin-compatible xattrs): sidebar tag list, tag views, info-panel editor,
// drag-onto-tag. Colours are `color:<preset|#hex>` tags shown as folder tab / paper edge colours.
import { listen } from "@tauri-apps/api/event";
import { $, Entry, invoke, esc, TAG_COLORS } from "./util";
import { hooks, pane, allPanes, flash } from "./app";

let known: [string, number][] = [];
const label = (t: string) => (t.startsWith("color:") ? t.slice(6)[0].toUpperCase() + t.slice(7) : t);
const swatch = (t: string) => { const c = t.slice(6); return TAG_COLORS.includes(c) ? `var(--t-${c})` : c; };

export async function refreshTags() {
  known = await invoke<[string, number][]>("tag_counts").catch(() => []);
  const cur = pane()?.loc;
  $("tag-head").hidden = !known.length;
  // plain tags first, then colours
  const sorted = [...known].sort((a, b) => Number(a[0].startsWith("color:")) - Number(b[0].startsWith("color:")) || a[0].localeCompare(b[0]));
  $("tags").innerHTML = sorted.map(([t, n]) => {
    const loc = `tag:${t}`;
    return `<div class="tagrow${cur === loc ? " active" : ""}" data-tag="${esc(t)}" data-loc="${esc(loc)}"><i style="--tagc:${t.startsWith("color:") ? swatch(t) : "var(--accent)"}"></i>${esc(label(t))}<b>${n}</b></div>`;
  }).join("");
  let dl = document.getElementById("wc-tags") as HTMLDataListElement | null;
  if (!dl) { dl = document.createElement("datalist"); dl.id = "wc-tags"; document.body.append(dl); }
  dl.innerHTML = known.filter(([t]) => !t.startsWith("color:")).map(([t]) => `<option value="${esc(t)}">`).join("");
}

$("tags").addEventListener("click", (ev) => {
  const r = (ev.target as HTMLElement).closest<HTMLElement>(".tagrow"); if (r) pane().navigate(r.dataset.loc!);
});
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
    <div class="tag-editor">${plain.map((t) => `<span class="chip">${esc(t)}<b data-rm="${esc(t)}" title="Remove">✕</b></span>`).join("")}
      <input list="wc-tags" placeholder="Add tag…" spellcheck="false"></div>
    <div class="mini">Colour</div>
    <div class="swatches">${TAG_COLORS.map((c) => `<button data-c="${c}" title="${c}" class="${color === c ? "on" : ""}" style="background:var(--t-${c})"></button>`).join("")}
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
  input.addEventListener("change", () => { if (known.some(([t]) => t === input.value)) { editTags(paths, [input.value], []); input.value = ""; } });
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

refreshTags();
