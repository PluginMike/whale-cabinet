// File picker for other apps (xdg-desktop-portal "Open file", see portal.rs): Recent files, zoxide's frequent
// folders, places and one search box. Type to filter the folder (and find zoxide folders), Enter picks.
import { register, frame, close } from "./dialog";
import { esc, fmtSize, fmtDate, invoke, baseName, parentOf, shown, assetUrl, kindOf, ext, collator, type Entry } from "./util";

type Filter = { name: string; globs: string[]; mimes: string[] };
type Arg = { title: string; multiple: boolean; directory: boolean; accept: string | null; folder: string | null; filters: Filter[]; current: string | null };
type Place = { title: string; path: string; hidden: boolean };

const toUri = (p: string) => "file://" + p.split("/").map(encodeURIComponent).join("/");
const globRe = (g: string) => new RegExp("^" + g.replace(/[.+^${}()|\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".") + "$", "i");
const mimeOk = (pats: string[], m: string) => pats.some((p) => p === m || (p.endsWith("/*") && m.startsWith(p.slice(0, -1))));

register("pick", async (raw) => {
  const a = JSON.parse(raw) as Arg;
  const home = (await invoke<string>("resolve_path", { input: "~", cwd: "/" }).catch(() => "")) || "/";
  const body = frame(a.title || (a.directory ? "Choose a folder" : "Open"), `
    <div class="pk">
      <aside><button class="pk-recent">Recent</button><h4>Frequent</h4><div class="pk-z"></div><h4>Places</h4><div class="pk-places"></div></aside>
      <section>
        <div class="pk-top"><span class="pk-where"></span><input class="pk-q" placeholder="Filter, or type a folder name / path…" spellcheck="false"></div>
        <ul class="pk-list" tabindex="0"></ul>
      </section>
    </div>
    <footer class="pk-foot">
      ${a.filters.length ? `<select class="pk-filter">${a.filters.map((f, i) => `<option value="${i}" ${f.name === a.current ? "selected" : ""}>${esc(f.name)}</option>`).join("")}<option value="-1">All files</option></select>` : ""}
      <span class="pk-n"></span><button class="pk-cancel">Cancel</button><button class="primary pk-go">${esc(a.accept || (a.directory ? "Choose" : "Open"))}</button>
    </footer>`);
  body.parentElement!.classList.add("pk-dlg");
  const $ = <T extends HTMLElement>(s: string) => body.querySelector<T>(s)!;
  const list = $<HTMLUListElement>(".pk-list"), q = $<HTMLInputElement>(".pk-q"), go = $<HTMLButtonElement>(".pk-go"), filterSel = body.querySelector<HTMLSelectElement>(".pk-filter");

  let loc = "", items: Entry[] = [], shownItems: Entry[] = [], mimes = new Map<string, string>(), hidden = false;
  const sel = new Set<string>();
  let cursor = -1;

  const filter = () => (filterSel && +filterSel.value >= 0 ? a.filters[+filterSel.value] : null);
  const fits = (e: Entry) => {
    if (e.dir) return true;
    if (a.directory) return false;
    const f = filter();
    if (!f) return true;
    return f.globs.some((g) => globRe(g).test(e.name)) || (!!f.mimes.length && mimeOk(f.mimes, mimes.get(e.path) ?? ""));
  };

  // ---------- drawing ----------
  const io = new IntersectionObserver((xs) => xs.forEach((x) => {
    if (!x.isIntersecting) return;
    const img = x.target as HTMLImageElement;
    io.unobserve(img);
    invoke<string>("thumbnail", { path: img.dataset.p, size: 128 }).then((p) => (img.src = assetUrl(p))).catch(() => img.remove());
  }), { root: list, rootMargin: "300px" });
  function draw() {
    const f = q.value.trim().toLowerCase();
    const typedPath = /^[~/]/.test(f);
    shownItems = items.filter((e) => fits(e) && (hidden || !e.hidden) && (typedPath || !f || e.name.toLowerCase().includes(f)));
    cursor = Math.min(cursor, shownItems.length - 1);
    list.innerHTML = shownItems.map((e, i) => {
      const k = e.dir ? "dir" : kindOf(e);
      return `<li data-i="${i}" class="${sel.has(e.path) ? "on" : ""} ${i === cursor ? "cur" : ""}">` +
        `<span class="pk-ico k-${k}">${k === "img" ? `<img data-p="${esc(e.path)}" alt="">` : e.dir ? "▸" : esc(ext(e).slice(0, 4) || "·")}</span>` +
        `<span class="pk-name">${esc(shown(e.name))}${loc ? "" : `<small>${esc(parentOf(e.path).replace(home, "~"))}</small>`}</span>` +
        `<span class="pk-size">${e.dir ? "" : fmtSize(e.size)}</span><span class="pk-date">${fmtDate(e.mtime)}</span></li>`;
    }).join("") || `<li class="pk-empty">${f ? "Nothing matches — Enter jumps to the first Frequent folder" : "Empty"}</li>`;
    list.querySelectorAll<HTMLImageElement>("img[data-p]").forEach((i) => { i.onload = () => i.classList.add("ok"); io.observe(i); });
    list.querySelector(".cur")?.scrollIntoView({ block: "nearest" });
    const files = [...sel];
    $(".pk-n").textContent = files.length > 1 ? `${files.length} selected` : "";
    go.disabled = !a.directory && !files.some((p) => !items.find((e) => e.path === p)?.dir);
  }

  async function frequent() {
    const zs = await invoke<[number, string][]>("zoxide_query", { q: /^[~/]/.test(q.value) ? "" : q.value.trim(), limit: 8 }).catch(() => []);
    $(".pk-z").innerHTML = zs.map(([, p]) => `<button data-p="${esc(p)}" title="${esc(p)}">${esc(shown(baseName(p)))}<small>${esc(parentOf(p).replace(home, "~"))}</small></button>`).join("") || `<p class="hint">No matches</p>`;
  }

  // ---------- navigation ----------
  async function show(where: string) {
    const recent = where === "";
    const next = recent ? await invoke<Entry[]>("recent_list", { limit: 300 }).catch(() => [])
      : await invoke<Entry[]>("list_dir", { path: where }).catch((e) => { $(".pk-n").textContent = String(e); return null; });
    if (!next) return;
    loc = where; items = next; sel.clear(); cursor = -1; q.value = "";
    if (!recent) items.sort((x, y) => +y.dir - +x.dir || collator.compare(x.name, y.name));
    mimes = new Map();
    if (a.filters.some((f) => f.mimes.length)) {
      const files = items.filter((e) => !e.dir);
      const ms = await invoke<string[]>("mime_types", { paths: files.map((e) => e.path) }).catch(() => []);
      files.forEach((e, i) => mimes.set(e.path, ms[i] ?? ""));
    }
    $(".pk-where").textContent = recent ? "Recent" : loc.replace(home, "~");
    $(".pk-where").title = loc;
    body.querySelectorAll(".pk aside .on").forEach((b) => b.classList.remove("on"));
    (recent ? $(".pk-recent") : body.querySelector(`aside [data-p="${CSS.escape(loc)}"]`))?.classList.add("on");
    draw(); frequent();
    q.focus();
  }

  const pick = (paths: string[]) => { if (paths.length) invoke("portal_done", { uris: paths.map(toUri) }); };
  function accept() {
    const picked = shownItems.filter((e) => sel.has(e.path));
    if (a.directory) { const d = picked.find((e) => e.dir)?.path ?? loc; return pick(d ? [d] : []); }
    if (picked.length === 1 && picked[0].dir) return show(picked[0].path);
    pick(picked.filter((e) => !e.dir).map((e) => e.path));
  }
  async function enter() {
    const t = q.value.trim();
    if (/^[~/]/.test(t)) { // a typed path: open the folder, or pick the file
      const p = await invoke<string>("resolve_path", { input: t, cwd: loc || home }).catch(() => "");
      const e = p && (await invoke<Entry[]>("list_dir", { path: parentOf(p) }).catch(() => [])).find((x) => x.path === p);
      if (e && e.dir) return show(p);
      if (e && !a.directory) return pick([p]);
      $(".pk-n").textContent = "No such file or folder";
      return;
    }
    if (sel.size) return accept();
    if (shownItems.length === 1 || (t && shownItems.length)) { sel.add(shownItems[0].path); return accept(); }
    const z = body.querySelector<HTMLElement>(".pk-z [data-p]");
    if (t && z) show(z.dataset.p!);
    else if (a.directory) accept();
  }

  // ---------- input ----------
  list.addEventListener("click", (ev) => {
    const li = (ev.target as HTMLElement).closest<HTMLElement>("li[data-i]"); if (!li) return;
    const i = +li.dataset.i!, e = shownItems[i];
    if (!(a.multiple && ev.ctrlKey)) sel.clear();
    sel.has(e.path) ? sel.delete(e.path) : sel.add(e.path);
    cursor = i; draw();
  });
  list.addEventListener("dblclick", (ev) => {
    const li = (ev.target as HTMLElement).closest<HTMLElement>("li[data-i]"); if (!li) return;
    const e = shownItems[+li.dataset.i!];
    if (e.dir) show(e.path); else if (!a.directory) pick([e.path]);
  });
  body.querySelector("aside")!.addEventListener("click", (ev) => {
    const b = (ev.target as HTMLElement).closest<HTMLElement>("button"); if (!b) return;
    show(b.classList.contains("pk-recent") ? "" : b.dataset.p!);
  });
  q.addEventListener("input", () => { sel.clear(); cursor = q.value ? 0 : -1; draw(); frequent(); });
  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") { ev.preventDefault(); enter(); }
    else if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      cursor = Math.max(0, Math.min(shownItems.length - 1, cursor + (ev.key === "ArrowDown" ? 1 : -1)));
      if (!(a.multiple && ev.shiftKey)) sel.clear();
      if (shownItems[cursor]) sel.add(shownItems[cursor].path);
      draw();
    }
    else if (ev.key === "Backspace" && !q.value && loc && loc !== "/") { ev.preventDefault(); show(parentOf(loc)); }
    else if (ev.key.toLowerCase() === "h" && ev.ctrlKey) { ev.preventDefault(); hidden = !hidden; draw(); }
    else if (ev.key.toLowerCase() === "a" && ev.ctrlKey && a.multiple && document.activeElement !== q) { ev.preventDefault(); shownItems.filter((e) => !e.dir).forEach((e) => sel.add(e.path)); draw(); }
  });
  filterSel?.addEventListener("change", draw);
  go.onclick = () => (sel.size ? accept() : enter());
  $(".pk-cancel").onclick = close;

  const places = await invoke<Place[]>("places_list").catch(() => []);
  $(".pk-places").innerHTML = places.filter((p) => !p.hidden && p.path).map((p) => `<button data-p="${esc(p.path)}">${esc(p.title)}</button>`).join("");
  show(a.folder ?? (a.directory ? home : ""));
});
