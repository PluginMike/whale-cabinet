// Network locations (gvfs): sidebar section with saved connections (user-places.xbel, shared with Dolphin) and
// live mounts, plus a Connect dialog — URL, SSH hosts from ~/.ssh/config, SMB browsing, credentials,
// host-key questions, optional keyring password.
import { $, invoke, esc } from "./util";
import { pane, newTab, flash, allPanes, HOME } from "./app";
import { showMenu } from "./menu";

type Place = { href: string; title: string };
type State = { saved: Place[]; mounted: { name: string; uri: string }[]; ssh_hosts: string[]; gio: boolean; keyring: boolean };
let state: State = { saved: [], mounted: [], ssh_hosts: [], gio: true, keyring: false };

const norm = (u: string) => u.replace(/\/+$/, "");
const isMounted = (uri: string) => state.mounted.some((m) => norm(m.uri) === norm(uri) || norm(m.uri).startsWith(norm(uri)));

export async function refreshNetwork() {
  state = await invoke<State>("net_state").catch(() => state);
  const extra = state.mounted.filter((m) => !state.saved.some((s) => norm(s.href) === norm(m.uri)));
  $("network").innerHTML =
    state.saved.map((s) => `<div class="drawer small net${isMounted(s.href) ? "" : " unmounted"}" data-uri="${esc(s.href)}" title="${esc(s.href)}"><span class="label">${esc(s.title || s.href)}</span><span class="handle"></span>${isMounted(s.href) ? `<button data-act="eject" title="Disconnect">⏏</button>` : ""}</div>`).join("") +
    extra.map((m) => `<div class="drawer small net" data-uri="${esc(m.uri)}" title="${esc(m.uri)}"><span class="label">${esc(m.name)}</span><span class="handle"></span><button data-act="eject" title="Disconnect">⏏</button></div>`).join("") +
    `<button class="sb-add" id="net-connect">${state.gio ? "+ Connect to server…" : "Network needs gvfs (gio)"}</button>`;
}

/** Mount (asking for whatever is missing) and open the location. */
export async function connect(uri: string, opts: { newTab?: boolean } = {}) {
  flash(`Connecting to ${uri}…`);
  try {
    const path = await invoke<string>("net_mount", { uri, user: "", domain: "", password: "", trust: false, remember: false });
    opts.newTab ? newTab(path) : pane().navigate(path);
    refreshNetwork();
  } catch (e) {
    const err = String(e);
    if (err.startsWith("auth:") || err.startsWith("question:")) connectDialog(uri, err);
    else flash(err);
  }
}

async function disconnect(uri: string) {
  // step out of it first so nothing keeps the mount busy
  for (const p of allPanes()) if (p.loc.includes("/gvfs/")) await p.navigate(HOME);
  await invoke("net_unmount", { uri }).catch((e) => flash(String(e)));
  refreshNetwork();
}

$("network").addEventListener("click", (ev) => {
  const t = ev.target as HTMLElement;
  if (t.closest("#net-connect")) { connectDialog(); return; }
  const d = t.closest<HTMLElement>(".drawer[data-uri]"); if (!d) return;
  ev.stopPropagation();
  if (t.closest("[data-act=eject]")) disconnect(d.dataset.uri!);
  else connect(d.dataset.uri!);
}, true);
$("network").addEventListener("contextmenu", (ev) => {
  const d = (ev.target as HTMLElement).closest<HTMLElement>(".drawer[data-uri]"); if (!d) return;
  ev.preventDefault();
  const uri = d.dataset.uri!, saved = state.saved.some((s) => norm(s.href) === norm(uri));
  showMenu(ev.clientX, ev.clientY, [
    { label: "Open", act: () => connect(uri) },
    { label: "Open in New Tab", act: () => connect(uri, { newTab: true }) },
    "-",
    { label: "Save in Network", off: saved, act: async () => { await invoke("net_save", { uri, title: d.textContent!.trim() }); refreshNetwork(); } },
    { label: "Disconnect", off: !isMounted(uri), act: () => disconnect(uri) },
    { label: "Remove (and forget password)", off: !saved, danger: true, act: async () => { await invoke("net_forget", { uri }); refreshNetwork(); } },
  ]);
});
window.addEventListener("focus", refreshNetwork);

// ---------- connect dialog ----------
export function connectDialog(uri = "", why = "") {
  document.querySelector(".modal.netdlg")?.remove();
  const m = document.createElement("div");
  m.className = "modal netdlg";
  const smb = (u: string) => u.startsWith("smb://");
  m.innerHTML = `<div class="modal-box wide">
    <h3>Connect to server</h3>
    <label class="full">Address <input id="nd-uri" spellcheck="false" placeholder="sftp://host/  smb://server/share  davs://host/path  nfs://host/export" value="${esc(uri)}"></label>
    ${state.ssh_hosts.length ? `<div class="mini">SSH hosts</div><div class="chips">${state.ssh_hosts.map((h) => `<button class="chip-btn" data-host="${esc(h)}">${esc(h)}</button>`).join("")}</div>` : ""}
    <div class="mini">Windows shares <button class="linkish" id="nd-browse">Browse network…</button></div><div id="nd-found" class="found"></div>
    <div id="nd-auth" hidden>
      <p class="why"></p>
      <div class="row2"><label>User <input id="nd-user" spellcheck="false"></label><label class="smb-only">Domain <input id="nd-domain" spellcheck="false" placeholder="WORKGROUP"></label></div>
      <label class="full">Password <input id="nd-pass" type="password"></label>
      ${state.keyring ? `<label><input type="checkbox" id="nd-remember"> Remember password (keyring)</label>` : ""}
    </div>
    <div id="nd-q" hidden><p class="why"></p><label><input type="checkbox" id="nd-trust"> Trust it and connect</label></div>
    <label><input type="checkbox" id="nd-save"> Save in the sidebar's Network section</label>
    <p id="nd-err" class="err" hidden></p>
    <div class="btns"><button data-v="0">Cancel</button><button data-v="1" class="primary">Connect</button></div>
  </div>`;
  document.body.append(m);
  const q = <T extends HTMLElement = HTMLInputElement>(s: string) => m.querySelector<T>(s)!;
  const input = q("#nd-uri");
  const syncSmb = () => m.querySelectorAll<HTMLElement>(".smb-only").forEach((x) => (x.hidden = !smb(input.value)));
  syncSmb();
  input.addEventListener("input", syncSmb);
  const show = (err: string) => {
    if (err.startsWith("auth:")) { q("#nd-auth").hidden = false; q("#nd-auth .why").textContent = err.slice(5) || "This server needs a user name and password."; q("#nd-pass").focus(); }
    else if (err.startsWith("question:")) { q("#nd-q").hidden = false; q("#nd-q .why").textContent = err.slice(9); }
    else { q("#nd-err").hidden = false; q("#nd-err").textContent = err; }
  };
  if (why) show(why);
  const close = () => { m.remove(); window.removeEventListener("keydown", key, true); pane().scroller.focus(); };
  const go = async () => {
    const u = input.value.trim();
    if (!/^[a-z]+:\/\//.test(u)) { flash("Enter an address like sftp://host/"); input.focus(); return; }
    const btn = q<HTMLButtonElement>("[data-v='1']"); btn.disabled = true; btn.textContent = "Connecting…";
    try {
      const path = await invoke<string>("net_mount", { uri: u, user: q("#nd-user").value, domain: q("#nd-domain").value, password: q("#nd-pass").value, trust: q("#nd-trust").checked, remember: !!m.querySelector<HTMLInputElement>("#nd-remember")?.checked });
      if (q("#nd-save").checked) await invoke("net_save", { uri: u, title: u.replace(/^[a-z]+:\/\//, "").replace(/\/$/, "") });
      close();
      pane().navigate(path);
      refreshNetwork();
    } catch (e) { show(String(e)); }
    btn.disabled = false; btn.textContent = "Connect";
  };
  const browse = async (at: string) => {
    const box = q<HTMLElement>("#nd-found");
    box.innerHTML = `<span class="hint">Looking in ${esc(at)}…</span>`;
    try {
      const found = await invoke<{ name: string; uri: string }[]>("net_browse", { uri: at });
      box.innerHTML = found.length ? found.map((f) => `<button class="chip-btn" data-found="${esc(f.uri)}" title="${esc(f.uri)}">${esc(f.name)}</button>`).join("") : `<span class="hint">Nothing found in ${esc(at)}. Windows 10/11 shares are found through WS-Discovery (install gvfs-wsdd); or type smb://server/share above.</span>`;
    } catch (err) { box.innerHTML = `<span class="hint">${esc(String(err))}</span>`; }
  };
  const key = (e: KeyboardEvent) => { e.stopPropagation(); if (e.key === "Escape") close(); if (e.key === "Enter" && !(e.target as HTMLElement).closest("button")) { e.preventDefault(); go(); } };
  window.addEventListener("keydown", key, true);
  m.addEventListener("click", async (e) => {
    const t = e.target as HTMLElement;
    if (t === m || t.dataset.v === "0") close();
    else if (t.dataset.v === "1") go();
    else if (t.dataset.host) { input.value = `sftp://${t.dataset.host}/`; syncSmb(); }
    else if (t.id === "nd-browse") browse("smb://");
    else if (t.dataset.found) {
      // workgroups and servers list further; a share (smb://server/share) is the pick
      const u = t.dataset.found;
      input.value = u; syncSmb();
      if (u.replace(/^smb:\/\//, "").split("/").filter(Boolean).length < 2) browse(u);
    }
  });
  input.focus();
}

refreshNetwork();
