/**
 * 前端状态与后端调用。
 *
 * 界面的过滤、排序、搜索**全部在本地做**（曲库行很小，一次性全量拿到），
 * 因此这些操作是零延迟的，不会每敲一个字就往后端跑一次。
 */

import { invoke } from "./tauri.js";
import type {
  LibraryStats,
  Settings,
  SnapshotDto,
  TrackDetailDto,
  TrackRowDto,
  WritePlanDto,
} from "./types.js";

export type FilterKey = "all" | "todo" | "completed" | "problem";
/** 可排序的列。必须与 `.thead .sortable[data-sort]` 的取值一致。 */
export type SortKey = "title" | "artist" | "duration" | "state" | "score";

export function emptyStats(): LibraryStats {
  return {
    all: 0,
    todo: 0,
    completed: 0,
    problem: 0,
    idle: 0,
    matched: 0,
    confirm: 0,
    done: 0,
    failed: 0,
    skip: 0,
  };
}

/** 设置项的默认值。**必须与 Rust 侧 `Settings::default` 一致**：
 *  它只在后端还没返回快照前用作占位，一旦拿到真值就会被整体替换。 */
export const DEFAULT_SETTINGS: Settings = {
  general: { theme: "system", first_run_done: false },
  library: { last_scan_path: "", recent_paths: [] },
  lyrics: { save_target: "file", include_translation: true, overwrite_existing: false },
  write: { fill_missing_info: true, embed_cover: false },
};

export const S = {
  root: "",
  rows: [] as TrackRowDto[],
  stats: emptyStats(),
  settings: structuredClone(DEFAULT_SETTINGS),
  selectedId: null as number | null,
  checked: new Set<number>(),
  filter: "all" as FilterKey,
  query: "",
  sort: { key: null as SortKey | null, dir: 1 as 1 | -1 },
  detail: null as TrackDetailDto | null,
  /** 实际生效的主题（`system` 已经解析成 light/dark） */
  theme: "light" as "light" | "dark",
  cacheBytes: 0,
};

// ── 后端调用 ─────────────────────────────────────────────────────────────

export async function refresh(): Promise<SnapshotDto> {
  const snap = await invoke<SnapshotDto>("snapshot");
  applySnapshot(snap);
  return snap;
}

export function applySnapshot(snap: SnapshotDto): void {
  S.root = snap.library.root;
  S.rows = snap.library.tracks;
  S.stats = snap.library.stats;
  S.settings = snap.settings;
  S.cacheBytes = snap.cacheBytes;

  // 选中项还在就直接保留，否则退到第一行
  if (S.selectedId !== null && !S.rows.some((r) => r.id === S.selectedId)) {
    S.selectedId = null;
  }
  if (S.selectedId === null && S.rows.length > 0) {
    S.selectedId = S.rows[0]!.id;
  }
}

export async function loadDetail(id: number): Promise<TrackDetailDto | null> {
  const detail = await invoke<TrackDetailDto | null>("track_detail", { trackId: id });
  if (S.selectedId === id) S.detail = detail;
  return detail;
}

export async function saveSettings(settings: Settings): Promise<Settings> {
  const saved = await invoke<Settings>("save_settings", { settings });
  S.settings = saved;
  return saved;
}

export async function planWrite(trackIds: number[]): Promise<WritePlanDto> {
  return invoke<WritePlanDto>("plan_write", {
    trackIds,
    options: S.settings,
  });
}

// ── 本地过滤 / 排序 ──────────────────────────────────────────────────────

/** 排序用的比较器，全局只建一个。`localeCompare(…, "zh")` 每次调用都要按 locale
 *  重新准备排序规则，万行排序时这部分开销会直接变成滚动卡顿。两者的排序结果相同。 */
const collator = new Intl.Collator("zh");

function inFilter(r: TrackRowDto, filter: FilterKey): boolean {
  switch (filter) {
    // 「待处理」= 已匹配 + 待确认（失败单列为「未找到歌词」，§9.6 缺陷修复 5）
    case "todo":
      return r.state === "matched" || r.state === "confirm";
    case "completed":
      return r.state === "done";
    case "problem":
      return r.state === "failed";
    default:
      return true;
  }
}

/** `q` 须已 trim + 小写 */
function matchesQuery(r: TrackRowDto, q: string): boolean {
  return !q || `${r.title} ${r.artist} ${r.matched?.album ?? ""}`.toLowerCase().includes(q);
}

/** 这一行在当前智能视图 + 搜索条件下是否可见（与 {@link visibleRows} 同一条规则） */
export function isRowVisible(r: TrackRowDto): boolean {
  return inFilter(r, S.filter) && matchesQuery(r, S.query.trim().toLowerCase());
}

function compareRows(key: SortKey, dir: 1 | -1): (a: TrackRowDto, b: TrackRowDto) => number {
  return (a, b) => {
    let x: string | number;
    let y: string | number;
    switch (key) {
      case "state":
        x = a.stateLabel;
        y = b.stateLabel;
        break;
      case "score":
        x = a.matched?.score ?? -1;
        y = b.matched?.score ?? -1;
        break;
      case "duration":
        x = a.duration;
        y = b.duration;
        break;
      default:
        x = (a[key] ?? "") as string;
        y = (b[key] ?? "") as string;
    }
    if (typeof x === "string" || typeof y === "string") {
      return collator.compare(String(x), String(y)) * dir;
    }
    return (x - y) * dir;
  };
}

/** 上一次 {@link visibleRows} 的输入与结果 */
let memo: {
  rows: TrackRowDto[];
  filter: FilterKey;
  query: string;
  sortKey: SortKey | null;
  sortDir: 1 | -1;
  list: TrackRowDto[];
} | null = null;

/** 行对象被就地修改之后调用（例如 `track:updated`），让下一次 visibleRows 重新计算。
 *  整体替换 `S.rows`、改筛选 / 搜索 / 排序都不必调它——那些会被自动识别。 */
export function invalidateRows(): void {
  memo = null;
}

/**
 * 当前视图下的行（筛选 + 搜索 + 排序）。
 *
 * 虚拟滚动每一帧都要取它，一次渲染里也会被取好几次，因此按输入记忆结果：
 * 输入不变时直接返回上一次的数组，不再对上万行重复过滤、排序。
 */
export function visibleRows(): TrackRowDto[] {
  if (
    memo &&
    memo.rows === S.rows &&
    memo.filter === S.filter &&
    memo.query === S.query &&
    memo.sortKey === S.sort.key &&
    memo.sortDir === S.sort.dir
  ) {
    return memo.list;
  }

  const q = S.query.trim().toLowerCase();
  let list =
    S.filter === "all" && !q ? S.rows : S.rows.filter((r) => inFilter(r, S.filter) && matchesQuery(r, q));
  if (S.sort.key) list = [...list].sort(compareRows(S.sort.key, S.sort.dir));

  memo = {
    rows: S.rows,
    filter: S.filter,
    query: S.query,
    sortKey: S.sort.key,
    sortDir: S.sort.dir,
    list,
  };
  return list;
}
