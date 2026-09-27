// Feature dialogs (each opens in its own whale-cabinet-dialog window).
import { register, frame, close, win } from "./dialog";
import { esc, fmtSize, fmtDate, invoke, baseName, parentOf, shown, assetUrl, KINDS, ext } from "./util";

// ---------- file conflict ----------
register("conflict", (arg) => {
  const { id, title, c } = JSON.parse(arg);
  let answered = false;
  const isImg = (p: string) => KINDS[ext({ name: p, dir: false })] === "img";
  const card = (label: string, path: string, dir: boolean, size: number, mtime: number) =>
    `<div><b>${label}</b><br>${esc(shown(baseName(path)))}<br>${dir ? "Folder" : fmtSize(size)} · ${fmtDate(mtime)}${!dir && isImg(path) ? `<img src="${assetUrl(path)}">` : ""}</div>`;
  const body = frame("File already exists", `
    <p>${esc(title)}: <b>${esc(shown(baseName(c.dst)))}</b> already exists in <code>${esc(parentOf(c.dst))}</code>.</p>
    <div class="conflict">${card("Existing", c.dst, c.dst_dir, c.dst_size, c.dst_mtime)}${card("Incoming", c.src, c.src_dir, c.src_size, c.src_mtime)}</div>
    <label>New name <input id="nn" value="${esc(c.suggestion)}" style="flex:1"></label>
    <label><input type="checkbox" id="all"> Apply to all remaining conflicts</label>
    <div class="btns"><button data-c="cancel">Cancel</button><button data-c="skip">Skip</button><button data-c="rename">Rename</button><button data-c="overwrite" class="danger">${c.src_dir !== c.dst_dir ? "Replace (old one to trash)" : "Overwrite"}</button></div>`);
  const send = (choice: string) => {
    answered = true;
    const all = (body.querySelector("#all") as HTMLInputElement).checked;
    const name = (body.querySelector("#nn") as HTMLInputElement).value;
    invoke("op_resolve", { id, choice: choice === "rename" ? { choice, name } : { choice }, all }).finally(close);
  };
  body.addEventListener("click", (e) => { const ch = (e.target as HTMLElement).dataset.c; if (ch) send(ch); });
  (body.querySelector("#nn") as HTMLInputElement).addEventListener("keydown", (e) => { if (e.key === "Enter") send("rename"); });
  win.onCloseRequested(() => { if (!answered) invoke("op_resolve", { id, choice: { choice: "cancel" }, all: false }); });
});

// ---------- Properties (Dolphin-style: General / Permissions / Details / Checksums) ----------
type Props = {
  name: string; path: string; parent: string; mime: string; is_dir: boolean; size: number; link_target: string | null;
  created: number | null; modified: number; accessed: number; owner: string; group: string; uid: number; gid: number; mode: number; can_chmod: boolean;
};
register("props", async (arg) => {
  const { paths } = JSON.parse(arg) as { paths: string[] };
  if (paths.length > 1) {
    const all = await Promise.all(paths.map((p) => invoke<Props>("file_props", { path: p }).catch(() => null)));
    const ok = all.filter(Boolean) as Props[];
    const body = frame(`${paths.length} items`, `<div class="kv"><span>Items</span><span>${ok.length} (${ok.filter((p) => p.is_dir).length} folders)</span><span>Location</span><span>${esc(parentOf(paths[0]))}</span><span>Size</span><span id="tot">${fmtSize(ok.reduce((s, p) => s + p.size, 0))} in files</span></div>
      <div class="btns"><button id="calc">Calculate total size</button><button class="primary" id="ok">Close</button></div>`);
    body.querySelector<HTMLElement>("#ok")!.onclick = close;
    body.querySelector<HTMLElement>("#calc")!.onclick = async () => {
      let bytes = 0;
      for (const p of ok) bytes += p.is_dir ? (await invoke<{ bytes: number }>("dir_stats", { path: p.path })).bytes : p.size;
      body.querySelector("#tot")!.textContent = `${fmtSize(bytes)} (${bytes.toLocaleString()} bytes)`;
    };
    return;
  }
  const path = paths[0];
  let p = await invoke<Props>("file_props", { path });
  const icon = await invoke<string | null>("mime_icon", { path }).catch(() => null);
  const date = (ms: number | null) => (ms ? new Date(ms).toLocaleString() : "—");
  const body = frame(p.name, `
    <div class="tabs-h"><button data-t="gen" class="on">General</button><button data-t="perm">Permissions</button>${p.is_dir ? "" : `<button data-t="det">Details</button><button data-t="sum">Checksums</button>`}</div>
    <section data-p="gen">
      <div style="display:flex;gap:12px;align-items:center;margin-bottom:12px">
        ${icon ? `<img src="${assetUrl(icon)}" style="width:48px;height:48px">` : ""}
        <input id="nm" value="${esc(p.name)}" style="flex:1;height:30px;padding:0 8px" ${p.parent ? "" : "disabled"}>
      </div>
      <div class="kv">
        <span>Type</span><span>${esc(p.is_dir ? "Folder" : p.mime)}</span>
        <span>Location</span><span>${esc(p.parent)}</span>
        ${p.link_target !== null ? `<span>Points to</span><span>${esc(p.link_target)}</span>` : ""}
        <span>Size</span><span id="sz">${p.is_dir ? `<button id="calc">Calculate</button>` : `${fmtSize(p.size)} (${p.size.toLocaleString()} bytes)`}</span>
        ${p.is_dir ? `<span>Contents</span><span id="ct">…</span>` : ""}
        <span>Created</span><span>${date(p.created)}</span>
        <span>Modified</span><span>${date(p.modified)}</span>
        <span>Accessed</span><span>${date(p.accessed)}</span>
      </div>
      <p class="hint" id="msg"></p>
    </section>
    <section data-p="perm" hidden>
      <div class="kv"><span>Owner</span><span>${esc(p.owner)} (${p.uid})</span><span>Group</span><span>${esc(p.group)} (${p.gid})</span></div>
      <table class="perm"><tr><th></th><th>Read</th><th>Write</th><th>${p.is_dir ? "Enter" : "Execute"}</th></tr>
        ${["Owner", "Group", "Others"].map((who, r) => `<tr><td>${who}</td>${[4, 2, 1].map((bit) => `<td><input type="checkbox" data-b="${bit << ((2 - r) * 3)}" ${p.mode & (bit << ((2 - r) * 3)) ? "checked" : ""} ${p.can_chmod ? "" : "disabled"}></td>`).join("")}</tr>`).join("")}
      </table>
      <p class="hint">Mode <code id="oct">${(p.mode & 0o7777).toString(8).padStart(4, "0")}</code>${p.can_chmod ? "" : " — only the owner can change this."}</p>
      ${p.is_dir ? `<label><input type="checkbox" id="rec" ${p.can_chmod ? "" : "disabled"}> Apply to all subfolders and their contents (files keep no execute bit unless they had one)</label>` : ""}
      <div class="btns"><button class="primary" id="apply" ${p.can_chmod ? "" : "disabled"}>Apply</button></div>
      <p class="hint" id="permmsg"></p>
    </section>
    <section data-p="det" hidden><div class="kv" id="details"><span></span><span>Reading…</span></div></section>
    <section data-p="sum" hidden>
      ${["md5", "sha256"].map((a) => `<p><button data-sum="${a}">${a === "md5" ? "MD5" : "SHA-256"}</button> <code id="s-${a}" style="word-break:break-all;user-select:text"></code> <button data-copy="${a}" hidden>Copy</button></p>`).join("")}
      <label>Compare with <input id="cmp" placeholder="paste an expected checksum" style="flex:1"></label><p class="hint" id="cmpres"></p>
    </section>`);
  // tabs
  body.querySelector(".tabs-h")!.addEventListener("click", (e) => {
    const t = (e.target as HTMLElement).dataset.t; if (!t) return;
    body.querySelectorAll<HTMLElement>(".tabs-h button").forEach((b) => b.classList.toggle("on", b.dataset.t === t));
    body.querySelectorAll<HTMLElement>("section").forEach((s) => (s.hidden = s.dataset.p !== t));
    if (t === "det") loadDetails();
  });
  // rename
  const nm = body.querySelector<HTMLInputElement>("#nm")!;
  const doRename = async () => {
    if (!nm.value.trim() || nm.value === p.name) return;
    try {
      const [to] = await invoke<[string, unknown]>("rename_item", { path: p.path, name: nm.value.trim() });
      p = await invoke<Props>("file_props", { path: to });
      body.querySelector("#msg")!.textContent = "Renamed.";
      win.setTitle(`Properties — ${p.name}`);
    } catch (e) { body.querySelector("#msg")!.textContent = String(e); nm.value = p.name; }
  };
  nm.addEventListener("keydown", (e) => { e.stopPropagation(); if (e.key === "Enter") doRename(); if (e.key === "Escape") { nm.value = p.name; nm.blur(); } });
  nm.addEventListener("blur", doRename);
  // folder size / contents
  if (p.is_dir) {
    invoke<unknown[]>("list_dir", { path }).then((l) => { body.querySelector("#ct")!.textContent = `${l.length} items`; }).catch(() => {});
    body.querySelector<HTMLElement>("#calc")!.onclick = async (e) => {
      (e.target as HTMLButtonElement).textContent = "Calculating…";
      const s = await invoke<{ files: number; dirs: number; bytes: number }>("dir_stats", { path: p.path });
      body.querySelector("#sz")!.textContent = `${fmtSize(s.bytes)} (${s.bytes.toLocaleString()} bytes)`;
      body.querySelector("#ct")!.textContent = `${s.files.toLocaleString()} files, ${s.dirs.toLocaleString()} folders`;
    };
  }
  // permissions
  const boxes = [...body.querySelectorAll<HTMLInputElement>(".perm input")];
  const modeNow = () => (p.mode & ~0o777) | boxes.reduce((m, b) => m | (b.checked ? +b.dataset.b! : 0), 0);
  boxes.forEach((b) => b.addEventListener("change", () => { body.querySelector("#oct")!.textContent = modeNow().toString(8).padStart(4, "0"); }));
  body.querySelector<HTMLElement>("#apply")!.onclick = async () => {
    const rec = body.querySelector<HTMLInputElement>("#rec")?.checked ?? false;
    const errs = await invoke<string[]>("set_mode", { path: p.path, mode: modeNow(), recursive: rec });
    body.querySelector("#permmsg")!.textContent = errs.length ? `${errs.length} problem(s): ${errs.slice(0, 3).join(" · ")}` : "Permissions applied.";
    p = await invoke<Props>("file_props", { path: p.path });
  };
  // details
  let detailsLoaded = false;
  async function loadDetails() {
    if (detailsLoaded) return; detailsLoaded = true;
    const d = await invoke<Record<string, string>>("file_details", { path: p.path }).catch(() => ({}));
    const el = body.querySelector("#details")!;
    el.innerHTML = Object.keys(d).length ? Object.entries(d).map(([k, v]) => `<span>${esc(k)}</span><span>${esc(v)}</span>`).join("") : `<span></span><span class="hint">No extra details for this type of file.</span>`;
  }
  // checksums
  const sums: Record<string, string> = {};
  const compare = () => {
    const want = body.querySelector<HTMLInputElement>("#cmp")!.value.trim().toLowerCase();
    const res = body.querySelector("#cmpres")!;
    if (!want) { res.textContent = ""; return; }
    const hit = Object.entries(sums).find(([, v]) => v === want);
    res.textContent = hit ? `✓ Matches ${hit[0].toUpperCase()}` : Object.keys(sums).length ? "✗ No calculated checksum matches" : "Calculate a checksum first";
  };
  body.querySelector("#cmp")!.addEventListener("input", compare);
  body.querySelectorAll<HTMLElement>("[data-sum]").forEach((b) => b.onclick = async () => {
    const a = b.dataset.sum!;
    const out = body.querySelector(`#s-${a}`)!;
    out.textContent = "Calculating…";
    try { sums[a] = await invoke<string>("checksum", { path: p.path, algo: a }); out.textContent = sums[a]; body.querySelector<HTMLElement>(`[data-copy="${a}"]`)!.hidden = false; }
    catch (e) { out.textContent = String(e); }
    compare();
  });
  body.querySelectorAll<HTMLElement>("[data-copy]").forEach((b) => b.onclick = () => invoke("copy_text", { text: sums[b.dataset.copy!] }));
});

// ---------- Open With: pick any application ----------
register("openwith", async (arg) => {
  const { paths, mime } = JSON.parse(arg) as { paths: string[]; mime: string };
  const apps = await invoke<{ id: string; name: string; icon: string | null }[]>("all_apps");
  const body = frame("Open With", `
    <p>Open <b>${esc(paths.length === 1 ? shown(baseName(paths[0])) : `${paths.length} files`)}</b> with:</p>
    <input id="q" placeholder="Search applications…" style="width:100%;height:30px;padding:0 8px">
    <ul class="applist"></ul>
    <label><input type="checkbox" id="always"> Always use this application for <code>${esc(mime)}</code></label>
    <div class="btns"><button id="cancel">Cancel</button><button class="primary" id="go" disabled>Open</button></div>`);
  const ul = body.querySelector<HTMLElement>(".applist")!, q = body.querySelector<HTMLInputElement>("#q")!, go = body.querySelector<HTMLButtonElement>("#go")!;
  let pick = "";
  const draw = () => {
    const f = q.value.toLowerCase();
    const shownApps = apps.filter((a) => a.name.toLowerCase().includes(f) || a.id.toLowerCase().includes(f));
    ul.innerHTML = shownApps.map((a) => `<li data-id="${esc(a.id)}" class="${a.id === pick ? "on" : ""}">${a.icon ? `<img src="${assetUrl(a.icon)}" loading="lazy">` : `<span class="ico"></span>`}${esc(a.name)}</li>`).join("");
  };
  const open = async () => {
    if (!pick) return;
    if (body.querySelector<HTMLInputElement>("#always")!.checked) await invoke("set_default_app", { mime, id: pick }).catch(() => {});
    await invoke("launch_app", { id: pick, paths });
    close();
  };
  ul.addEventListener("click", (e) => { const li = (e.target as HTMLElement).closest<HTMLElement>("li"); if (!li) return; pick = li.dataset.id!; go.disabled = false; draw(); });
  ul.addEventListener("dblclick", open);
  q.addEventListener("input", draw);
  q.addEventListener("keydown", (e) => { if (e.key === "Enter") { const first = ul.querySelector<HTMLElement>("li"); if (!pick && first) pick = first.dataset.id!; open(); } });
  go.onclick = open;
  body.querySelector<HTMLElement>("#cancel")!.onclick = close;
  draw();
  q.focus();
});
