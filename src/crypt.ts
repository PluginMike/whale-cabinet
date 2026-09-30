// Encrypt / decrypt with gpg: right-click → Encrypt… (password, or keys in your keyring; folders become
// name.tar.gpg); opening a .gpg file decrypts it next to itself, asking for the password when it needs one.
import { Entry, invoke, esc, shown, baseName, parentOf } from "./util";
import { hooks, pane, flash, allPanes } from "./app";

const ENC = /\.(gpg|pgp)$/i;
const isEnc = (e: Entry) => !e.dir && ENC.test(e.name);

function modal(html: string) {
  const m = document.createElement("div");
  m.className = "modal";
  m.innerHTML = `<div class="modal-box wide">${html}</div>`;
  document.body.append(m);
  const close = () => { m.remove(); window.removeEventListener("keydown", key, true); pane().scroller.focus(); };
  let onEnter = () => {};
  const key = (e: KeyboardEvent) => { e.stopPropagation(); if (e.key === "Escape") close(); if (e.key === "Enter") { e.preventDefault(); onEnter(); } };
  window.addEventListener("keydown", key, true);
  m.addEventListener("mousedown", (e) => { if (e.target === m) close(); });
  return { m, close, enter: (f: () => void) => (onEnter = f) };
}
async function reveal(path: string) {
  for (const p of allPanes()) if (p.loc === parentOf(path)) { await p.load(p.loc); if (p === pane()) p.selectPaths([path]); }
}

export async function encryptDialog(e: Entry) {
  const keys = await invoke<{ id: string; uid: string }[]>("gpg_keys").catch(() => []);
  const { m, close, enter } = modal(`<h3>Encrypt “${esc(shown(e.name))}”</h3>
    ${e.dir ? `<p class="hint">The folder is packed into ${esc(shown(e.name))}.tar.gpg.</p>` : ""}
    ${keys.length ? `<label><input type="radio" name="how" value="pw" checked> With a password</label><label><input type="radio" name="how" value="key"> To a key: <select id="en-key">${keys.map((k) => `<option value="${esc(k.id)}">${esc(k.uid || k.id)}</option>`).join("")}</select></label>` : ""}
    <div id="en-pw"><label class="full">Password <input id="en-p1" type="password"></label><label class="full">Again <input id="en-p2" type="password"></label></div>
    <label><input type="checkbox" id="en-trash"> Move the original to the trash afterwards</label>
    <p id="en-err" class="err" hidden></p>
    <div class="btns"><button data-v="0">Cancel</button><button data-v="1" class="primary">Encrypt</button></div>`);
  const q = <T extends HTMLElement = HTMLInputElement>(s: string) => m.querySelector<T>(s)!;
  const byKey = () => m.querySelector<HTMLInputElement>("input[name=how][value=key]")?.checked ?? false;
  m.addEventListener("change", () => { q("#en-pw").hidden = byKey(); });
  const err = (s: string) => { q("#en-err").hidden = false; q("#en-err").textContent = s; };
  const go = async () => {
    const pw = q("#en-p1").value;
    if (!byKey()) {
      if (!pw) { err("Type a password"); q("#en-p1").focus(); return; }
      if (pw !== q("#en-p2").value) { err("The passwords don't match"); q("#en-p2").focus(); return; }
    }
    const btn = q<HTMLButtonElement>("[data-v='1']"); btn.disabled = true; btn.textContent = "Encrypting…";
    try {
      const out = await invoke<string>("encrypt_item", { path: e.path, password: byKey() ? null : pw, recipients: byKey() ? [q<HTMLSelectElement>("#en-key").value] : [] });
      const trash = q("#en-trash").checked;
      close();
      if (trash) await invoke("op_trash", { items: [e.path] });
      flash(`Encrypted to ${baseName(out)}${trash ? "; the original is in the trash" : ""}`);
      reveal(out);
    } catch (x) { err(String(x)); btn.disabled = false; btn.textContent = "Encrypt"; }
  };
  enter(go);
  m.addEventListener("click", (ev) => { const v = (ev.target as HTMLElement).dataset.v; if (v === "0") close(); if (v === "1") go(); });
  q("#en-p1").focus();
}

export async function decrypt(e: Entry) {
  const done = (out: string) => { flash(`Decrypted to ${baseName(out)}`); reveal(out); };
  if (!(await invoke<boolean>("is_symmetric", { path: e.path }).catch(() => true))) {
    // encrypted to a key: gpg-agent asks for that key's passphrase itself
    flash("Decrypting…");
    invoke<string>("decrypt_item", { path: e.path, password: null }).then(done).catch((x) => flash(String(x)));
    return;
  }
  const { m, close, enter } = modal(`<h3>Decrypt “${esc(shown(e.name))}”</h3>
    <label class="full">Password <input id="de-p" type="password"></label><p id="de-err" class="err" hidden></p>
    <div class="btns"><button data-v="0">Cancel</button><button data-v="1" class="primary">Decrypt</button></div>`);
  const input = m.querySelector<HTMLInputElement>("#de-p")!, errEl = m.querySelector<HTMLElement>("#de-err")!;
  const go = async () => {
    try { const out = await invoke<string>("decrypt_item", { path: e.path, password: input.value }); close(); done(out); }
    catch (x) { errEl.hidden = false; errEl.textContent = String(x); input.select(); }
  };
  enter(go);
  m.addEventListener("click", (ev) => { const v = (ev.target as HTMLElement).dataset.v; if (v === "0") close(); if (v === "1") go(); });
  input.focus();
}

hooks.menuExtra.push((p, es) => {
  if (es.length !== 1 || es[0].trashId || p.loc === "trash:/") return [];
  const e = es[0];
  return [isEnc(e) ? { label: "Decrypt…", act: () => decrypt(e) } : { label: "Encrypt…", act: () => encryptDialog(e) }, "-"];
});
// opening an encrypted file decrypts it
const prevOpen = hooks.open;
hooks.open = (e, p) => (isEnc(e) ? decrypt(e) : prevOpen?.(e, p));
