/**
 * 前后端数据契约。
 *
 * 与 Rust 侧的 `cmd::dto` **逐字段对应**——改一边必须改另一边。
 * 这里是 TypeScript 唯一的「外部世界」类型定义，UI 层不再自己造结构。
 */

/** 曲目状态键。必须与 Rust `orchestrator::state_key` 和原型 `STATE_META` 一致。 */
export type TrackState =
  | "idle"
  | "matching"
  | "matched"
  | "confirm"
  | "writing"
  | "done"
  | "failed"
  | "skip";

export type ProviderKey = "netease" | "qq" | "kugou" | "kuwo";

export interface MatchedDto {
  provider: ProviderKey;
  providerName: string;
  score: number;
  title: string;
  artist: string;
  album: string;
  year?: number;
  /** 秒 */
  duration: number;
}

/** 文件里已有的歌词形态（与 Rust `LyricsPresence` 逐字段对应） */
export type LyricsPresence = "none" | "sidecarLrc" | "embeddedTag" | "both";

export interface TrackRowDto {
  /** 曲目 ID。后端保证它 ≤ 2^53-1（见 `TrackId::JS_MAX_SAFE_INTEGER`）——
   *  越界的话这个数字在 JS 里会被静默舍入，再传回后端就查不到任何曲目。 */
  id: number;
  title: string;
  artist: string;
  /** 秒 */
  duration: number;
  format: string;
  state: TrackState;
  stateLabel: string;
  /** 这首歌的文件里 / 旁边已经有什么歌词（扫描时探明，与「状态」列是两回事） */
  existingLyrics: LyricsPresence;
  matched?: MatchedDto;
  /** 匹配到的歌词与本地信息不一致 → 高亮并打 ≠ */
  mismatch: boolean;
  message?: string;
}

export interface CandidateDto {
  provider: ProviderKey;
  providerName: string;
  songId: string;
  /** 取词所需的第二个标识（酷狗必需） */
  accessKey?: string;
  title: string;
  artist: string;
  album: string;
  year?: number;
  trackNo?: number;
  /** 秒 */
  duration: number;
  score: number;
  coverUrl?: string;
}

export interface PreviewDto {
  text: string;
  lines: number;
  bytes: number;
  hasTranslation: boolean;
  hasVerbatim: boolean;
}

/** 平台能力提示：说明哪些字段**该平台根本没有**，而不是假装有 */
export interface CapabilitiesDto {
  noYear: boolean;
  noTrackNo: boolean;
  noVerbatim: boolean;
  noTranslationVerified: boolean;
}

export interface TrackDetailDto {
  id: number;
  title: string;
  artist: string;
  album: string;
  year?: number;
  trackNo?: number;
  /** 秒 */
  duration: number;
  format: string;
  fileName: string;
  path: string;
  fileSize: number;
  state: TrackState;
  stateLabel: string;
  message?: string;
  lyricsLabel: string;
  hasCover: boolean;
  metaSource: string;
  metaConfidence: number;
  candidates: CandidateDto[];
  pick: number;
  preview?: PreviewDto;
  capabilities: CapabilitiesDto;
  provider?: ProviderKey;
  canWrite: boolean;
  needsReview: boolean;
}

export interface LibraryStats {
  all: number;
  todo: number;
  completed: number;
  problem: number;
  idle: number;
  matched: number;
  confirm: number;
  done: number;
  failed: number;
  skip: number;
}

export interface LibraryDto {
  root: string;
  tracks: TrackRowDto[];
  stats: LibraryStats;
}

/** 用户设置。字段名与 `config.toml` 一致（snake_case）——配置文件格式以设计文档为准。 */
export interface Settings {
  general: {
    theme: "system" | "light" | "dark";
    first_run_done: boolean;
  };
  library: {
    last_scan_path: string;
    recent_paths: string[];
  };
  lyrics: {
    save_target: "file" | "sidecar";
    include_translation: boolean;
    overwrite_existing: boolean;
  };
  write: {
    fill_missing_info: boolean;
    embed_cover: boolean;
  };
}

export interface ScanResultDto {
  count: number;
  elapsedMs: number;
  skipped: number;
  path: string;
  restored: number;
  downgraded: number;
}

export interface WritePlanDto {
  total: number;
  deltaBytes: number;
  locked: number;
  skipped: number;
  /** 跳过的原因分项：「跳过 N 首」本身不构成可行动的信息 */
  skippedExisting: number;
  skippedUnsupported: number;
  skippedNoMatch: number;
  target: "file" | "sidecar";
}

export interface SnapshotDto {
  library: LibraryDto;
  settings: Settings;
  cacheBytes: number;
  cacheEntries: number;
  running: number[];
}

export interface MatchReport {
  total: number;
  matched: number;
  confirm: number;
  failed: number;
  skipped: number;
  cancelled: boolean;
  sourcesDown: boolean;
}

export interface FailureItem {
  title: string;
  reason: string;
}

export interface WriteReport {
  total: number;
  ok: number;
  failed: number;
  skipped: number;
  locked: number;
  skippedExisting: number;
  skippedUnsupported: number;
  skippedNoMatch: number;
  bytesDelta: number;
  target: string;
  failures: FailureItem[];
  cancelled: boolean;
}

/** `task:done` 事件的载荷（内部标签枚举，按 kind 分支） */
export type TaskDoneDto =
  | { kind: "match"; taskId: number; report: MatchReport }
  | { kind: "write"; taskId: number; report: WriteReport };

export interface TrackUpdatedDto {
  trackId: number;
  state: TrackState;
  score?: number;
  message?: string;
  /** 文件里已有的歌词形态：写完之后会变，所以每行都要跟着事件更新 */
  existingLyrics: LyricsPresence;
  matched?: {
    provider: ProviderKey;
    songId: string;
    title: string;
    artists: string[];
    album?: string;
    year?: number;
    trackNo?: number;
    durationMs?: number;
    coverUrl?: string;
    score: number;
    confidence: number;
  };
}

export interface ProgressDto {
  phase: "scanning" | "matching" | "writing" | "idle" | "done" | "cancelled" | "failed";
  done: number;
  total: number;
  ok: number;
  warn: number;
  err: number;
  skip: number;
}

export interface LogDto {
  /** 与原型日志抽屉的 CSS 类名一致：info / warn / err / ok */
  level: "info" | "warn" | "err" | "ok";
  message: string;
}

export interface ScanDoneDto {
  count: number;
  elapsedMs: number;
  path: string;
}
