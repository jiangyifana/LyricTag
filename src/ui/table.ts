/** 曲库表格：虚拟滚动 + 「匹配到的歌词」列。 */

import {
  $,
  $$,
  esc,
  fmtDur,
  LYRICS_META,
  must,
  SRC_META,
  STATE_META,
  scoreClass,
} from "../dom.js";
import { S, visibleRows } from "../store.js";
import type { TrackRowDto } from "../types.js";

/** 行高（与 CSS 的 `--row-h` 一致，虚拟滚动靠它算位置） */
const ROW_H = 40;
/** 视口外多渲染几行，滚动时不至于露出空白 */
const OVERSCAN = 8;

/** 可排序的列。必须与 `.thead .sortable[data-sort]` 的取值一致。 */
export type SortKey = "title" | "artist" | "duration" | "state" | "score";

export interface TableHandlers {
  onSelect(id: number): void;
  onToggleCheck(id: number): void;
  onToggleAll(): void;
  onSort(key: SortKey): void;
  onVisibleChange(): void;
}

let handlers: TableHandlers | null = null;
let frame = 0;

export function initTable(h: TableHandlers): void {
  handlers = h;
  const tbody = must("#tbody");

  tbody.addEventListener("scroll", () => {
    // 滚动事件比帧率高，用 rAF 合并
    if (frame) return;
    frame = window.requestAnimationFrame(() => {
      frame = 0;
      paintRows();
    });
  });

  tbody.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const check = target.closest<HTMLElement>("[data-check]");
    if (check) {
      e.stopPropagation();
      handlers?.onToggleCheck(Number(check.dataset.check));
      return;
    }
    const row = target.closest<HTMLElement>(".trow");
    if (row?.dataset.id) handlers?.onSelect(Number(row.dataset.id));
  });

  must("#checkAll").addEventListener("click", () => handlers?.onToggleAll());

  $$(".thead .sortable").forEach((head) => {
    head.addEventListener("click", () => {
      const key = head.dataset.sort as SortKey | undefined;
      if (key) handlers?.onSort(key);
    });
  });

  // 窗口尺寸变化会改变可视行数
  window.addEventListener("resize", () => paintRows());
}

/** 整表重绘（数据变化后调用）。**会保持滚动位置**。 */
export function renderTable(): void {
  const tbody = must("#tbody");
  const empty = must("#emptyState");
  const alldone = must("#alldoneState");
  const list = visibleRows();

  updateHeaderSort();

  if (list.length === 0) {
    tbody.innerHTML = "";
    tbody.hidden = true;
    // 两种空状态：还没有曲库 vs 筛选后没有结果
    const noLibrary = S.rows.length === 0;
    empty.hidden = !noLibrary;
    alldone.hidden = noLibrary;
    if (!noLibrary) {
      must("#alldoneTitle").textContent = "这个筛选条件下没有歌曲";
    }
    handlers?.onVisibleChange();
    return;
  }

  // 重建 `#tbody` 的子节点会把 scrollTop 归零。批量任务期间每首歌都会触发一次
  // 重绘，不保住滚动位置的话，列表会一直往顶部跳。
  const keepTop = tbody.scrollTop;

  tbody.hidden = false;
  empty.hidden = true;
  alldone.hidden = true;

  // 撑高层决定滚动条长度，行本身按需渲染
  tbody.innerHTML = `<div class="vspacer" data-total="${list.length}" style="height:${list.length * ROW_H}px"></div>`;
  tbody.scrollTop = keepTop;
  paintRows();
  handlers?.onVisibleChange();
}

/**
 * 只重绘可视窗口内的行。
 *
 * 批量任务进行中，每一首歌的状态变更只需要刷新这几行——
 * 重建整个撑高层既浪费，也会丢掉滚动位置。
 */
export function repaintRows(): void {
  paintRows();
}

/** 只重绘可视窗口内的行。 */
function paintRows(): void {
  const tbody = must("#tbody");
  const spacer = $(".vspacer", tbody);
  if (!spacer) return;

  const list = visibleRows();
  const scrollTop = tbody.scrollTop;
  const viewHeight = tbody.clientHeight || 600;

  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(list.length, Math.ceil((scrollTop + viewHeight) / ROW_H) + OVERSCAN);

  let html = "";
  for (let i = start; i < end; i++) {
    const row = list[i];
    if (!row) continue;
    html += rowHtml(row, i * ROW_H);
  }
  spacer.innerHTML = html;
}

function rowHtml(row: TrackRowDto, top: number): string {
  const meta = STATE_META[row.state] ?? STATE_META["idle"]!;
  const checked = S.checked.has(row.id);
  return `<div class="trow ${S.selectedId === row.id ? "on" : ""}" data-id="${row.id}" style="top:${top}px;height:${ROW_H}px">
    <div><span class="cbox ${checked ? "on" : ""}" data-check="${row.id}"></span></div>
    <div style="justify-content:center">
      <span class="st ${meta.cls}" style="width:20px;height:20px;padding:0;justify-content:center" title="${esc(meta.label)}">
        <span class="d"></span></span>
    </div>
    <div class="c-title trunc" title="${esc(row.title)}">${esc(row.title)}</div>
    <div class="c-artist trunc" title="${esc(row.artist)}">${esc(row.artist)}</div>
    <div class="c-dur num">${fmtDur(row.duration)}</div>
    <div><span class="st ${meta.cls}"><span class="d"></span>${esc(meta.label)}</span></div>
    <div>${lyricsCell(row)}</div>
    <div>${matchCell(row)}</div>
  </div>`;
}

/**
 * 「已有歌词」列。
 *
 * 保存前先看清这首歌里已经有什么——否则用户会以为「保存歌词」是必要的，
 * 实际上那批歌早就带着歌词了（下载时自带、或别的工具写过）。
 * 与「状态」列分工明确：状态是「我们做到哪一步」，这一列是「文件本来是什么样」。
 */
function lyricsCell(row: TrackRowDto): string {
  const meta = LYRICS_META[row.existingLyrics] ?? LYRICS_META.none;
  if (meta.cls === "none") return '<span class="mc-none">—</span>';
  return `<span class="lp ${meta.cls}" title="${esc(meta.hint)}">${esc(meta.label)}</span>`;
}

/**
 * 「匹配到的歌词」列。
 *
 * 上行 = 来源 + 匹配度条 + 数字；下行 = 候选的歌名 · 歌手。
 * **与本地信息不一致时高亮并打 `≠`**——这正是用户最需要看到的情况：
 * 本地文件名是错的，或者匹配到了别的版本（§6.3）。
 */
function matchCell(row: TrackRowDto): string {
  const m = row.matched;
  if (!m) return '<span class="mc-none">—</span>';

  const src = SRC_META[m.provider] ?? { name: m.provider, cls: "netease" };
  const cls = scoreClass(m.score);
  const differs = row.mismatch;
  const title = differs
    ? `匹配到的是：${m.title} · ${m.artist}${m.album ? " · " + m.album : ""}\n（与本地文件的信息不一致，请确认是否正确）`
    : `匹配到的是：${m.title} · ${m.artist}${m.album ? " · " + m.album : ""}`;

  return `<span class="mcell">
    <span class="mc-1">
      <span class="sq ${src.cls}"></span>${esc(src.name)}
      <span class="mc-bar"><i class="${cls}" style="width:${Math.round(m.score * 100)}%"></i></span>
      <span class="mc-sc ${cls}">${m.score.toFixed(2)}</span>
    </span>
    <span class="mc-2 ${differs ? "diff" : ""}" title="${esc(title)}">
      ${esc(m.title)} · ${esc(m.artist)}${differs ? '<span class="mc-neq">≠</span>' : ""}
    </span>
  </span>`;
}

/** 表头的排序指示器跟着当前排序键走 */
function updateHeaderSort(): void {
  $$(".thead .sortable").forEach((head) => {
    const key = head.dataset.sort;
    const active = S.sort.key === key;
    const label = head.textContent?.trim().replace(/[▲▼]\s*$/, "").trim() ?? "";
    head.innerHTML = active
      ? `${esc(label)} <span style="opacity:.9">${S.sort.dir === 1 ? "▲" : "▼"}</span>`
      : esc(label);
  });
}

/** 全选框的三态：全选 / 半选 / 未选 */
export function syncCheckAll(): void {
  const box = must("#checkAll");
  const list = visibleRows();
  const picked = list.filter((r) => S.checked.has(r.id)).length;
  box.className =
    "cbox" + (picked > 0 && picked === list.length ? " on" : picked > 0 ? " part" : "");
}

/** 批量操作条 */
export function syncBulkBar(): void {
  const n = S.checked.size;
  must("#bulkbar").classList.toggle("on", n > 0);
  must("#selCount").textContent = String(n);
}

/** 滚动到某一行并居中（「确认后自动跳到下一首」用） */
export function scrollRowIntoView(id: number): void {
  const tbody = must("#tbody");
  const index = visibleRows().findIndex((r) => r.id === id);
  if (index < 0) return;
  const target = index * ROW_H - tbody.clientHeight / 2 + ROW_H / 2;
  tbody.scrollTo({ top: Math.max(0, target), behavior: "smooth" });
  paintRows();
}
