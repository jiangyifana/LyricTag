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
  running: false,
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
  S.running = snap.running.length > 0;

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

export function visibleRows(): TrackRowDto[] {
  let list = S.rows;

  switch (S.filter) {
    case "todo":
      // 「待处理」= 已匹配 + 待确认（失败单列为「未找到歌词」，§9.6 缺陷修复 5）
      list = list.filter((r) => r.state === "matched" || r.state === "confirm");
      break;
    case "completed":
      list = list.filter((r) => r.state === "done");
      break;
    case "problem":
      list = list.filter((r) => r.state === "failed");
      break;
    default:
      break;
  }

  const q = S.query.trim().toLowerCase();
  if (q) {
    list = list.filter((r) =>
      `${r.title} ${r.artist} ${r.matched?.album ?? ""}`.toLowerCase().includes(q),
    );
  }

  if (S.sort.key) {
    const key = S.sort.key;
    const dir = S.sort.dir;
    list = [...list].sort((a, b) => {
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
        return String(x).localeCompare(String(y), "zh") * dir;
      }
      return (x - y) * dir;
    });
  }

  return list;
}
