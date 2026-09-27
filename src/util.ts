import { convertFileSrc } from "@tauri-apps/api/core";
export { invoke } from "@tauri-apps/api/core";

export type Entry = {
  name: string; path: string; dir: boolean; link: boolean; broken: boolean;
  special: string; hidden: boolean; size: number; mtime: number;
  tags?: string[];
  /** trash view only */ trashId?: string; origPath?: string; deleted?: number;
};

export const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
export const shown = (s: string) => s.replace(/[\n\r\t]/g, "↵");
export const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
export const ext = (e: { name: string; dir: boolean }) => { const i = e.name.lastIndexOf("."); return !e.dir && i > 0 ? e.name.slice(i + 1).toLowerCase() : ""; };
export const parentOf = (p: string) => (p === "/" ? "/" : p.slice(0, p.lastIndexOf("/")) || "/");
export const baseName = (p: string) => (p === "/" ? "/" : p.slice(p.lastIndexOf("/") + 1));
export const joinPath = (d: string, n: string) => (d === "/" ? "/" + n : `${d}/${n}`);
export const assetUrl = (p: string) => convertFileSrc(p);

export function fmtSize(n: number) {
  if (n < 1024) return `${n} B`;
  const u = ["KiB", "MiB", "GiB", "TiB", "PiB"]; let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return `${n.toFixed(n < 10 ? 1 : 0)} ${u[i]}`;
}
const dateFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
export const fmtDate = (ms: number) => (ms ? dateFmt.format(ms) : "");

export const KINDS: Record<string, string> = {};
for (const [k, list] of Object.entries({
  img: "png jpg jpeg gif webp svg bmp tiff tif avif heic ico jxl raw cr2 nef",
  vid: "mp4 mkv webm avi mov wmv flv m4v mpg mpeg",
  aud: "mp3 flac ogg opus wav m4a aac wma",
  arc: "zip tar gz xz bz2 zst 7z rar tgz iso deb rpm appimage",
  code: "ts js mjs py rs c h cpp hpp go java kt sh fish zsh lua json toml yaml yml xml html css scss sql rb php cs qml nix",
  doc: "pdf md txt odt ods odp doc docx xls xlsx ppt pptx rtf epub csv",
})) for (const x of list.split(" ")) KINDS[x] = k;
export const kindOf = (e: Entry) => KINDS[ext(e)] ?? "other";

export const TAG_COLORS = ["red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "grey"];
/** Colour for a `color:` tag: a preset name → its theme variable, otherwise a literal CSS colour. */
export function entryColor(e: Entry): string | undefined {
  const t = e.tags?.find((t) => t.startsWith("color:"));
  if (!t) return;
  const c = t.slice(6);
  return TAG_COLORS.includes(c) ? `var(--t-${c})` : /^#[0-9a-f]{3,8}$/i.test(c) ? c : undefined;
}
export const edgeColor = (e: Entry) => entryColor(e) ?? `var(--k-${kindOf(e)})`;

export function typeName(e: Entry) {
  if (e.broken) return "Broken link";
  if (e.dir) return e.link ? "Folder (link)" : "Folder";
  if (e.special) return ({ fifo: "Named pipe", socket: "Socket", char: "Character device", block: "Block device" } as Record<string, string>)[e.special] ?? e.special;
  const x = ext(e);
  return (x ? `${x.toUpperCase()} file` : "File") + (e.link ? " (link)" : "");
}

export const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
