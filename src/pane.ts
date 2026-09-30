// One file view (a tab can hold two for split view): location, history, listing, selection, virtualized rendering.
import { Entry, invoke, esc, shown, ext, parentOf, fmtSize, fmtDate, edgeColor, entryColor, collator, kindOf, tagColor } from "./util";

export type View = "cabinet" | "compact" | "grid";
/** `head` rows are group headers (Recent: "Today", duplicates: "3 copies") — never selectable. */
export type Row = { e: Entry; depth: number; parent: string; head?: string; n?: number };

export interface Host {
  showHidden: boolean;
  singleClick: boolean;
  /** Double-clicking a folder pulls it out inline (cabinet style) instead of opening it (Windows style). */
  dblExpand: boolean;
  activate(p: Pane): void;
  changed(p: Pane): void;
  open(e: Entry, p: Pane): void;
  openInTab(path: string): void;
  menu(p: Pane, ev: MouseEvent): void;
  startDrag(p: Pane, ev: MouseEvent): void;
  loadVirtual(loc: string, p: Pane): Promise<Entry[]>;
  thumb(e: Entry): string | undefined;
  /** "12 items" for a folder, loaded lazily. */
  count(e: Entry): string | undefined;
  /** Group label per entry for ranked views (headers between groups), if the location has groups. */
  groupOf(loc: string): ((e: Entry) => string) | undefined;
  /** Row height multiplier for a location (Recent shows bigger thumbnails). */
  rowScale(loc: string): number;
  /** Sort a location opens with (e.g. "rank" for ranked views). */
  defaultSort(loc: string): string | undefined;
  /** View a folder opens with: what was last used there, else the defaults. */
  folderView(loc: string): { view: View; sortKey: string; asc: boolean; zoom: number } | undefined;
  /** The user changed this pane's view, sort or zoom (remember it for the folder). */
  viewChanged(p: Pane): void;
  /** git state letter (C M S U I) for badges. */
  git(e: Entry): string | undefined;
  /** a plugin's status badge (state = ok | sync | warn | error | shared) */
  badge(e: Entry): { text: string; title: string; state: string } | undefined;
  /** View a location opens with (plugin photo folders open as icons). */
  defaultView(loc: string): View | undefined;
  syncWatch(): void;
  flash(msg: string): void;
}

const PAD = 8;
const GIT_MARKS: Record<string, string> = { C: "!", M: "M", S: "+", U: "?" };
const GIT_NAMES: Record<string, string> = { C: "Git: conflict", M: "Git: modified", S: "Git: staged", U: "Git: untracked" };
export const isFolder = (loc: string) => loc.startsWith("/");

export class Pane {
  el: HTMLElement; scroller: HTMLElement; spacer: HTMLElement; items: HTMLElement; band: HTMLElement; empty: HTMLElement;
  loc = "";
  back: string[] = []; fwd: string[] = [];
  cache = new Map<string, Entry[]>();
  expanded = new Set<string>();
  rows: Row[] = [];
  index = new Map<string, number>();
  sel = new Set<string>();
  anchor = ""; focus = "";
  sortKey = "name"; asc = true; filter = "";
  view: View = "cabinet"; zoom = 1;
  space: [number, number] | null = null;
  private seqs = new Map<string, number>();
  private pop: { parent: string; until: number } | null = null;
  private pending = new Set<string>(); private pendingTimer = 0;
  private bandState: { x0: number; y0: number; x: number; y: number; base: Set<string>; moved: boolean; i: number; ev: MouseEvent } | null = null;
  private press: { x: number; y: number; i: number; ev: MouseEvent; deferred: boolean } | null = null;

  constructor(private host: Host) {
    this.el = document.createElement("section");
    this.el.className = "view";
    this.el.innerHTML = `<div class="scroller" tabindex="0"><div class="spacer"></div><div class="items"></div><div class="band" hidden></div><div class="empty" hidden></div></div>`;
    this.scroller = this.el.querySelector(".scroller")!;
    [this.spacer, this.items, this.band, this.empty] = [".spacer", ".items", ".band", ".empty"].map((s) => this.el.querySelector<HTMLElement>(s)!);
    this.scroller.addEventListener("scroll", () => { this.render(); if (this.bandState) this.moveBand(); }, { passive: true });
    new ResizeObserver(() => { this.layout(); }).observe(this.scroller);
    this.scroller.addEventListener("mousedown", (ev) => this.mousedown(ev));
    this.scroller.addEventListener("auxclick", (ev) => { if (ev.button === 1) { const i = this.rowAt(ev.target); if (i >= 0 && this.rows[i].e.dir) this.host.openInTab(this.rows[i].e.path); } });
    this.scroller.addEventListener("contextmenu", (ev) => {
      ev.preventDefault();
      const i = this.rowAt(ev.target);
      if (i >= 0 && !this.sel.has(this.rows[i].e.path)) this.clickRow(i, new MouseEvent("mousedown"));
      if (i < 0) { this.sel.clear(); this.refreshSel(); }
      this.host.menu(this, ev);
    });
    this.scroller.addEventListener("wheel", (ev) => {
      if (!ev.ctrlKey) return;
      ev.preventDefault();
      this.setZoom(this.zoom * (ev.deltaY < 0 ? 1.1 : 1 / 1.1));
      this.host.viewChanged(this);
    }, { passive: false });
  }

  // ---------- geometry ----------
  geom() {
    const z = this.zoom;
    if (this.view === "grid") {
      const w = Math.round(156 * z), h = Math.round(168 * z);
      return { grid: true, w, h, cols: Math.max(1, Math.floor((this.scroller.clientWidth - PAD * 2) / w)), rowH: h };
    }
    const rowH = Math.round((this.view === "compact" ? 36 : 54) * z * this.host.rowScale(this.loc));
    return { grid: false, w: 0, h: rowH, cols: 1, rowH };
  }
  private layout() {
    const g = this.geom();
    this.spacer.style.height = `${Math.ceil(this.rows.length / g.cols) * g.rowH + PAD * 2 + 16}px`;
    this.render();
  }
  setZoom(z: number) {
    this.zoom = Math.max(0.5, Math.min(2.5, z));
    this.el.style.setProperty("--z", String(this.zoom));
    this.layout();
    this.host.changed(this);
  }
  setView(v: View) {
    this.view = v;
    if (v === "grid") { for (const p of this.expanded) this.cache.delete(p); this.expanded.clear(); this.host.syncWatch(); }
    this.el.dataset.view = v;
    this.rebuild();
  }

  // ---------- listing ----------
  private cmp = (a: Entry, b: Entry) => {
    // "rank" (relevance) keeps the source's scores and ignores folders-first
    if (this.sortKey === "rank") { const r = (b.score ?? 0) - (a.score ?? 0) || (a.group ?? 0) - (b.group ?? 0) || collator.compare(a.name, b.name); return this.asc ? r : -r; }
    if (a.dir !== b.dir) return a.dir ? -1 : 1;
    let r = 0;
    if (this.sortKey === "size") r = a.size - b.size;
    else if (this.sortKey === "mtime") r = a.mtime - b.mtime;
    else if (this.sortKey === "type") r = collator.compare(ext(a), ext(b));
    if (r === 0) r = collator.compare(a.name, b.name);
    return this.asc ? r : -r;
  };

  rebuild() {
    const f = this.filter.toLowerCase();
    const out: Row[] = [];
    const walk = (dir: string, depth: number): boolean => {
      let any = false;
      const list = (this.cache.get(dir) ?? []).filter((e) => this.host.showHidden || !e.hidden).sort(this.cmp);
      for (const e of list) {
        const at = out.length;
        out.push({ e, depth, parent: dir });
        const kids = e.dir && this.expanded.has(e.path) && walk(e.path, depth + 1);
        // With a filter, keep a row if it matches or an expanded descendant does.
        if (f && !kids && !e.name.toLowerCase().includes(f)) out.length = at;
        else any = true;
      }
      return any;
    };
    walk(this.loc, 0);
    const group = this.sortKey === "rank" && this.view !== "grid" ? this.host.groupOf(this.loc) : undefined;
    if (group) {
      const grouped: Row[] = [];
      let last = null as Row | null;
      for (const r of out) {
        const g = r.depth === 0 ? group(r.e) : null;
        if (g !== null && g !== last?.head) {
          last = { e: { name: g, path: `\0${g}`, dir: false, link: false, broken: false, special: "", hidden: false, size: 0, mtime: 0 }, depth: 0, parent: this.loc, head: g, n: 0 };
          grouped.push(last);
        }
        if (last && r.depth === 0) last.n!++;
        grouped.push(r);
      }
      out.splice(0, out.length, ...grouped);
    }
    this.rows = out;
    this.index = new Map(out.map((r, i) => [r.e.path, i]));
    if (this.cache.has(this.loc)) for (const p of this.sel) if (!this.index.has(p)) this.sel.delete(p); // keep pre-selection until the listing lands
    this.empty.hidden = out.length > 0;
    this.empty.textContent = this.cache.has(this.loc) ? (this.filter ? "Nothing matches the filter" : "Empty drawer") : "Opening…";
    this.layout();
    this.host.changed(this);
  }

  async load(dir: string) {
    const seq = (this.seqs.get(dir) ?? 0) + 1;
    this.seqs.set(dir, seq);
    try {
      const list = isFolder(dir) ? await invoke<Entry[]>("list_dir", { path: dir }) : await this.host.loadVirtual(dir, this);
      if (this.seqs.get(dir) !== seq || (dir !== this.loc && !this.expanded.has(dir))) return;
      this.cache.set(dir, list);
    } catch (err) {
      this.host.flash(String(err));
      // Location vanished or is unreadable: fall back to its parent.
      if (dir === this.loc) { if (isFolder(dir) && dir !== "/") this.navigate(parentOf(dir), false); return; }
      this.expanded.delete(dir);
      this.cache.delete(dir);
    }
    this.rebuild();
  }
  /** For streaming sources (search): replace the listing of the current location. */
  setListing(list: Entry[]) { this.cache.set(this.loc, list); this.rebuild(); }

  watched() { return isFolder(this.loc) ? [this.loc, ...this.expanded] : []; }

  onFsChange(dirs: string[]) {
    for (const d of dirs) if (d === this.loc || this.expanded.has(d)) this.pending.add(d);
    if (this.pending.size && !this.pendingTimer) this.pendingTimer = window.setTimeout(() => {
      this.pendingTimer = 0;
      const ds = [...this.pending]; this.pending.clear();
      ds.forEach((d) => this.load(d));
    }, 120);
  }
  reload() { this.load(this.loc); for (const d of this.expanded) this.load(d); }

  async navigate(loc: string, push = true, select?: string) {
    if (loc === this.loc) { if (select) { this.sel.clear(); this.sel.add(select); this.focus = this.anchor = select; this.scrollTo(select); this.refreshSel(); } return; }
    if (push && this.loc) { this.back.push(this.loc); this.fwd.length = 0; }
    this.loc = loc;
    const ds = this.host.defaultSort(loc), fv = isFolder(loc) ? this.host.folderView(loc) : undefined, dv = this.host.defaultView(loc);
    if (dv && dv !== this.view) { this.view = dv; this.el.dataset.view = dv; }
    if (ds) this.sortKey = ds;
    else if (fv) {
      this.sortKey = fv.sortKey; this.asc = fv.asc;
      if (fv.view !== this.view) { this.view = fv.view; this.el.dataset.view = fv.view; }
      if (fv.zoom !== this.zoom) { this.zoom = fv.zoom; this.el.style.setProperty("--z", String(fv.zoom)); }
    }
    else if (isFolder(loc) && this.sortKey === "rank") this.sortKey = "name"; // relevance only means something in ranked views
    this.cache.clear(); this.expanded.clear(); this.sel.clear();
    this.filter = "";
    this.focus = this.anchor = select ?? "";
    if (select) this.sel.add(select);
    this.scroller.scrollTop = 0;
    this.items.classList.remove("enter"); void this.items.offsetWidth; this.items.classList.add("enter");
    this.space = null;
    this.host.syncWatch();
    this.rebuild();
    if (isFolder(loc)) invoke<[number, number] | null>("disk_space", { path: loc }).then((s) => { this.space = s; this.host.changed(this); });
    await this.load(loc);
    if (select) this.scrollTo(select);
  }
  goBack() { const p = this.back.pop(); if (p) { this.fwd.push(this.loc); this.navigate(p, false); } }
  goFwd() { const p = this.fwd.pop(); if (p) { this.back.push(this.loc); this.navigate(p, false); } }
  goUp() { if (isFolder(this.loc) && this.loc !== "/") this.navigate(parentOf(this.loc), true, this.loc); }

  async toggleExpand(p: string, open = !this.expanded.has(p)) {
    if (open === this.expanded.has(p) || this.view === "grid") return;
    if (!open) {
      for (const x of [...this.expanded]) if (x === p || x.startsWith(p + "/")) { this.expanded.delete(x); this.cache.delete(x); }
    } else {
      this.expanded.add(p);
      this.pop = { parent: p, until: performance.now() + 700 };
      await this.load(p);
    }
    this.host.syncWatch();
    this.rebuild();
  }

  selected(): Entry[] { return [...this.sel].map((p) => this.rows[this.index.get(p)!]?.e).filter(Boolean); }
  entry(path: string) { const i = this.index.get(path); return i === undefined ? undefined : this.rows[i].e; }
  /** Folder that new items / pastes go into. */
  targetDir() { return isFolder(this.loc) ? this.loc : ""; }

  // ---------- rendering (virtualized) ----------
  private editing = false;
  render() {
    if (this.editing) return; // don't clobber an inline rename
    const g = this.geom();
    const top = this.scroller.scrollTop, h = this.scroller.clientHeight;
    const first = Math.max(0, (Math.floor((top - PAD) / g.rowH) - 4) * g.cols);
    const last = Math.min(this.rows.length, (Math.ceil((top + h) / g.rowH) + 4) * g.cols);
    if (this.pop && performance.now() > this.pop.until) this.pop = null;
    let html = "", popN = 0;
    for (let i = first; i < last; i++) {
      const { e, depth, parent, head, n } = this.rows[i];
      if (head !== undefined) {
        html += `<div class="item ghead" data-i="${i}" style="transform:translateY(${PAD + i * g.rowH}px);height:${g.rowH}px"><span>${esc(head)}</span><b>${n}</b></div>`;
        continue;
      }
      const cls = ["item", e.dir ? "folder" : "file"];
      if (this.sel.has(e.path)) cls.push("sel");
      if (this.focus === e.path) cls.push("focus");
      if (e.link) cls.push("link");
      if (e.broken) cls.push("broken");
      if (e.hidden) cls.push("hiddenf");
      if (e.dir && this.expanded.has(e.path)) cls.push("open");
      if (this.cutSet?.has(e.path)) cls.push("cut");
      const gs = this.host.git(e);
      if (gs === "I") cls.push("gitign");
      let style = g.grid
        ? `transform:translate(${PAD + (i % g.cols) * g.w}px,${PAD + Math.floor(i / g.cols) * g.rowH}px);width:${g.w}px;height:${g.h}px`
        : `transform:translateY(${PAD + i * g.rowH}px);height:${g.rowH}px;--depth:${depth}`;
      if (this.pop && parent === this.pop.parent) { cls.push("pop"); style += `;--d:${Math.min(popN++, 20) * 22}ms`; }
      const color = entryColor(e);
      if (color) style += `;--tabc:${color}`;
      style += `;--edge:${edgeColor(e)}`;
      const name = `<span class="name grab">${esc(shown(e.name))}</span>`;
      const thumb = !e.dir ? this.host.thumb(e) : undefined;
      const badge = `<span class="badge">${esc((ext(e) || e.special).slice(0, 5)) || "—"}</span>`;
      const tags = e.tags?.filter((t) => !t.startsWith("color:")).length ? `<span class="tagdots">${e.tags!.filter((t) => !t.startsWith("color:")).slice(0, 3).map((t) => `<i title="${esc(t)}"${tagColor(t) ? ` style="background:${tagColor(t)}"` : ""}></i>`).join("")}</span>` : "";
      const size = e.dir ? (e.trashId ? "" : this.host.count(e) ?? "") : e.special || e.broken ? "" : fmtSize(e.size);
      // virtual views (search, tags, trash) mix folders: show where each item lives
      const where = !isFolder(this.loc) ? `<span class="meta where">${esc(shown(e.where ?? parentOf(e.origPath ?? e.path)))}</span>` : "";
      const pb = this.host.badge(e);
      const gitb = (gs && gs !== "I" ? `<span class="gitb g-${gs}" title="${GIT_NAMES[gs]}">${GIT_MARKS[gs]}</span>` : "") +
        (pb ? `<span class="gitb pb-${esc(pb.state)}" title="${esc(pb.title)}">${esc(pb.text)}</span>` : "");
      const meta = `${where}${tags}${gitb}<span class="meta size">${size}</span>${e.app ? `<span class="meta app">${esc(e.app)}</span>` : ""}<span class="meta date">${fmtDate(e.deleted ?? e.used ?? e.mtime)}</span>`;
      if (g.grid) {
        const art = e.dir ? `<div class="gfold grab"></div>` : thumb ? `<img class="gthumb grab" src="${thumb}" loading="lazy" draggable="false">` : `<div class="gpaper grab">${badge}</div>`;
        html += `<div class="${cls.join(" ")}" data-i="${i}" style="${style}">${art}${name}</div>`;
      } else if (e.dir) {
        style += `;--stagger:${(i % 5) * 58 + 24}px`;
        html += `<div class="${cls.join(" ")}" data-i="${i}" style="${style}"><div class="tab grab"></div>` +
          `<div class="body"><span class="chev">›</span>${name}${meta}</div></div>`;
      } else {
        const lead = thumb ? `<img class="thumb grab" src="${thumb}" loading="lazy" draggable="false">` : badge;
        html += `<div class="${cls.join(" ")}" data-i="${i}" style="${style}"><div class="paper">${lead}${name}${meta}</div></div>`;
      }
    }
    this.items.innerHTML = html;
  }
  cutSet?: Set<string>;

  private scrollTo(p: string) {
    const i = this.index.get(p); if (i === undefined) return;
    const g = this.geom();
    const top = PAD + Math.floor(i / g.cols) * g.rowH, h = this.scroller.clientHeight;
    if (top < this.scroller.scrollTop) this.scroller.scrollTop = top - PAD;
    else if (top + g.rowH + PAD > this.scroller.scrollTop + h) this.scroller.scrollTop = top + g.rowH + PAD - h;
  }
  refreshSel() { this.render(); this.host.changed(this); }

  // ---------- selection ----------
  private selectRange(a: string, b: string, add: boolean) {
    const i = this.index.get(a) ?? 0, j = this.index.get(b) ?? 0;
    if (!add) this.sel.clear();
    for (let k = Math.min(i, j); k <= Math.max(i, j); k++) if (!this.rows[k].head) this.sel.add(this.rows[k].e.path);
  }
  clickRow(i: number, ev: { shiftKey: boolean; ctrlKey: boolean }) {
    if (this.rows[i].head) return;
    const p = this.rows[i].e.path;
    if (ev.shiftKey && this.anchor && this.index.has(this.anchor)) this.selectRange(this.anchor, p, ev.ctrlKey);
    else if (ev.ctrlKey) { this.sel.has(p) ? this.sel.delete(p) : this.sel.add(p); this.anchor = p; }
    else { this.sel.clear(); this.sel.add(p); this.anchor = p; }
    this.focus = p;
    this.refreshSel();
  }
  selectPaths(paths: string[]) { this.sel.clear(); paths.forEach((p) => this.sel.add(p)); if (paths[0]) { this.focus = this.anchor = paths[0]; this.scrollTo(paths[0]); } this.refreshSel(); }
  selectAll() { this.rows.forEach((r) => { if (!r.head) this.sel.add(r.e.path); }); this.refreshSel(); }
  invertSel() { for (const r of this.rows) if (!r.head) this.sel.has(r.e.path) ? this.sel.delete(r.e.path) : this.sel.add(r.e.path); this.refreshSel(); }

  // ---------- mouse ----------
  private rowAt(t: EventTarget | null) { const r = (t as HTMLElement).closest?.(".item") as HTMLElement | null; return r ? +r.dataset.i! : -1; }
  private point(ev: MouseEvent) { const r = this.scroller.getBoundingClientRect(); return { x: ev.clientX - r.left, y: ev.clientY - r.top + this.scroller.scrollTop }; }

  private mousedown(ev: MouseEvent) {
    this.host.activate(this);
    if (ev.button !== 0) return;
    this.scroller.focus({ preventScroll: true });
    const t = ev.target as HTMLElement, i = this.rowAt(t);
    // Double-click from the second press's click count: the row re-renders between the clicks, and WebKit
    // drops `dblclick` when the element that was pressed is gone.
    if (ev.detail === 2 && !t.classList.contains("chev")) { this.dblclick(ev); return; }
    if (i >= 0 && t.classList.contains("chev")) { this.toggleExpand(this.rows[i].e.path); return; }
    if (i >= 0 && t.closest(".grab, .badge, .thumb")) {
      const p = this.rows[i].e.path;
      // Pressing an already-selected item keeps the selection so it can be dragged; plain click narrows on release.
      const deferred = this.sel.has(p) && !ev.ctrlKey && !ev.shiftKey;
      if (!deferred) this.clickRow(i, ev);
      this.press = { ...this.point(ev), i, ev, deferred };
      const move = (m: MouseEvent) => {
        const q = this.point(m);
        if (this.press && Math.hypot(q.x - this.press.x, q.y - this.press.y) > 6) {
          if (this.press.ev.ctrlKey && !this.sel.has(p)) this.sel.add(p);
          this.press = null; cleanup();
          this.host.startDrag(this, m);
        }
      };
      const up = () => {
        cleanup();
        const pr = this.press; this.press = null;
        if (!pr) return;
        if (pr.deferred) this.clickRow(pr.i, pr.ev);
        if (this.host.singleClick && !pr.ev.ctrlKey && !pr.ev.shiftKey) this.host.open(this.rows[pr.i].e, this);
      };
      const cleanup = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
      return;
    }
    // Anywhere else (row whitespace or empty pane) starts a rubber band; a click without a drag acts as a click.
    const { x, y } = this.point(ev);
    this.bandState = { x0: x, y0: y, x, y, base: ev.ctrlKey ? new Set(this.sel) : new Set(), moved: false, i, ev };
    const move = (m: MouseEvent) => {
      const b = this.bandState; if (!b) return;
      const q = this.point(m); b.x = q.x; b.y = q.y;
      if (!b.moved && Math.hypot(b.x - b.x0, b.y - b.y0) < 5) return;
      b.moved = true;
      // NOTE: autoscroll only while the mouse moves; add a rAF loop if holding still at the edge matters.
      const r = this.scroller.getBoundingClientRect();
      if (m.clientY > r.bottom - 24) this.scroller.scrollTop += 20; else if (m.clientY < r.top + 24) this.scroller.scrollTop -= 20;
      this.moveBand();
    };
    const up = () => {
      window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up);
      const b = this.bandState; this.bandState = null; this.band.hidden = true;
      if (!b || b.moved) return;
      if (b.i >= 0 && this.view !== "grid") { this.clickRow(b.i, b.ev); if (this.host.singleClick && !b.ev.ctrlKey && !b.ev.shiftKey) this.host.open(this.rows[b.i].e, this); }
      else if (!b.ev.ctrlKey) { this.sel.clear(); this.refreshSel(); }
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  }

  private moveBand() {
    const b = this.bandState!; if (!b.moved) return;
    const top = Math.min(b.y0, b.y), bot = Math.max(b.y0, b.y), left = Math.min(b.x0, b.x), right = Math.max(b.x0, b.x);
    Object.assign(this.band.style, { left: `${left}px`, top: `${top}px`, width: `${right - left}px`, height: `${bot - top}px` });
    this.band.hidden = false;
    this.sel.clear(); b.base.forEach((p) => this.sel.add(p));
    const g = this.geom();
    const r0 = Math.max(0, Math.floor((top - PAD) / g.rowH)), r1 = Math.floor((bot - PAD) / g.rowH);
    const c0 = g.grid ? Math.max(0, Math.floor((left - PAD) / g.w)) : 0, c1 = g.grid ? Math.min(g.cols - 1, Math.floor((right - PAD) / g.w)) : 0;
    let last = -1;
    for (let r = r0; r <= r1; r++) for (let c = c0; c <= c1; c++) {
      const k = r * g.cols + c;
      if (k < this.rows.length && !this.rows[k].head) { this.sel.add(this.rows[k].e.path); last = k; }
    }
    if (last >= 0) this.focus = this.rows[last].e.path;
    this.refreshSel();
  }

  private dblclick(ev: MouseEvent) {
    const i = this.rowAt(ev.target);
    if (i >= 0 && this.rows[i].head) return;
    if (i < 0) { if (this.host.singleClick) return; if (!(ev.target as HTMLElement).closest(".item")) this.goUp(); return; }
    if ((ev.target as HTMLElement).classList.contains("chev") || this.host.singleClick) return;
    const e = this.rows[i].e;
    if (e.dir && this.host.dblExpand && this.view !== "grid" && !e.trashId) { this.toggleExpand(e.path); return; }
    this.host.open(e, this);
  }

  // ---------- keyboard ----------
  private moveFocus(to: number, ev: KeyboardEvent) {
    if (!this.rows.length) return;
    to = Math.max(0, Math.min(this.rows.length - 1, to));
    // step over group headers (in the direction of travel; the first row is always a header when grouped)
    const fi = this.index.get(this.focus) ?? -1;
    while (this.rows[to]?.head) to += to >= fi || to === 0 ? 1 : -1;
    if (!this.rows[to]) return;
    const p = this.rows[to].e.path;
    if (ev.shiftKey) { if (!this.anchor || !this.index.has(this.anchor)) this.anchor = this.focus || p; this.selectRange(this.anchor, p, ev.ctrlKey); }
    else if (!ev.ctrlKey) { this.sel.clear(); this.sel.add(p); this.anchor = p; }
    this.focus = p;
    this.scrollTo(p);
    this.refreshSel();
  }

  /** Returns true when the key was handled. */
  key(ev: KeyboardEvent): boolean {
    const k = ev.key, fi = this.index.get(this.focus) ?? -1;
    const g = this.geom();
    const page = Math.max(1, Math.floor(this.scroller.clientHeight / g.rowH) - 1) * g.cols;
    const stop = () => { ev.preventDefault(); return true; };
    switch (k) {
      case "ArrowDown": this.moveFocus(fi < 0 ? 0 : fi + g.cols, ev); return stop();
      case "ArrowUp": this.moveFocus(fi < 0 ? 0 : fi - g.cols, ev); return stop();
      case "PageDown": this.moveFocus(fi + page, ev); return stop();
      case "PageUp": this.moveFocus(fi - page, ev); return stop();
      case "Home": if (ev.altKey) return false; this.moveFocus(0, ev); return stop();
      case "End": this.moveFocus(this.rows.length - 1, ev); return stop();
      case "ArrowRight":
        if (ev.altKey) return false;
        if (g.grid) { this.moveFocus(fi + 1, ev); return stop(); }
        if (fi >= 0 && this.rows[fi].e.dir) { this.toggleExpand(this.focus, true); return stop(); }
        return false;
      case "ArrowLeft":
        if (ev.altKey) return false;
        if (g.grid) { this.moveFocus(fi - 1, ev); return stop(); }
        if (fi < 0) return false;
        if (this.expanded.has(this.focus)) this.toggleExpand(this.focus, false);
        else if (this.rows[fi].depth > 0) this.moveFocus(this.index.get(this.rows[fi].parent)!, ev);
        return stop();
      case "Enter": {
        if (ev.altKey || (fi < 0 && !this.sel.size)) return false;
        const picked = this.sel.size ? this.selected() : [this.rows[fi].e];
        const dir = picked.find((e) => e.dir);
        if (dir) this.host.open(dir, this); else picked.forEach((e) => this.host.open(e, this));
        return stop();
      }
      case " ":
        if (ev.ctrlKey && this.focus) { this.sel.has(this.focus) ? this.sel.delete(this.focus) : this.sel.add(this.focus); this.refreshSel(); return stop(); }
        return false;
    }
    return false;
  }

  /** Inline rename editor over the item's name. Resolves with the new name, or null if cancelled. */
  editName(path: string, initial?: string): Promise<string | null> {
    this.scrollTo(path);
    this.render();
    const i = this.index.get(path);
    const nameEl = this.items.querySelector<HTMLElement>(`.item[data-i="${i}"] .name`);
    if (i === undefined || !nameEl) return Promise.resolve(null);
    const input = document.createElement("input");
    input.className = "rename";
    const name = initial ?? this.rows[i].e.name;
    input.value = name;
    nameEl.replaceWith(input);
    this.editing = true;
    input.focus();
    const dot = name.lastIndexOf(".");
    input.setSelectionRange(0, this.rows[i].e.dir || dot <= 0 ? name.length : dot);
    return new Promise((resolve) => {
      let done = false;
      const finish = (v: string | null) => { if (done) return; done = true; this.editing = false; input.remove(); this.render(); this.scroller.focus({ preventScroll: true }); resolve(v); };
      input.addEventListener("keydown", (e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(input.value.trim() && input.value !== name ? input.value : null);
        if (e.key === "Escape") finish(null);
      });
      input.addEventListener("blur", () => finish(input.value.trim() && input.value !== name ? input.value : null));
      input.addEventListener("mousedown", (e) => e.stopPropagation());
    });
  }
}

export { kindOf };
