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
