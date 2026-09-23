/** DOM 与格式化的小工具。 */

import type { LyricsPresence } from "./types.js";

type Root = Document | Element;

const root = (r: Root): Root => r;

export function $(sel: string, r: Root = document): HTMLElement | null {
  return root(r).querySelector<HTMLElement>(sel);
}

export function $$(sel: string, r: Root = document): HTMLElement[] {
  return Array.from(root(r).querySelectorAll<HTMLElement>(sel));
}

/** 取元素；不存在就直接抛——模板 id 写错时应该立刻暴露，而不是静默不工作。 */
export function must(sel: string, r: Root = document): HTMLElement {
  const el = $(sel, r);
  if (!el) throw new Error(`缺少必要的界面元素：${sel}`);
  return el;
}

export function mustInput(sel: string, r: Root = document): HTMLInputElement {
  return must(sel, r) as HTMLInputElement;
}

/** HTML 转义。所有插入 innerHTML 的动态文本都必须过这一层。 */
export function esc(value: unknown): string {
  return String(value ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] ?? c,
  );
}

/** 秒 → `3:47` */
export function fmtDur(secs: number): string {
  if (!Number.isFinite(secs) || secs <= 0) return "—";
  const s = Math.round(secs);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** 字节 → `3.1 MB` / `420 KB` */
export function fmtSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0";
  if (bytes >= 1048576) return `${(bytes / 1048576).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

let toastTimer: number | undefined;

/** 瞬时提示。用于「没有可保存的歌曲」这类不需要弹窗的反馈。 */
export function toast(message: string): void {
  let el = $("#toast");
  if (!el) {
    el = document.createElement("div");
    el.id = "toast";
    el.className = "toast";
    document.body.appendChild(el);
  }
  el.textContent = message;
  el.classList.add("on");
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => el?.classList.remove("on"), 2600);
}

/** 状态 → 主题类名与文案。与原型 `STATE_META` 完全一致。 */
export const STATE_META: Record<string, { label: string; cls: string }> = {
  idle: { label: "未处理", cls: "idle" },
  matching: { label: "匹配中", cls: "matching" },
  matched: { label: "已匹配", cls: "matched" },
  confirm: { label: "待确认", cls: "confirm" },
  writing: { label: "写入中", cls: "writing" },
  done: { label: "已写入", cls: "done" },
  failed: { label: "失败", cls: "failed" },
  skip: { label: "跳过", cls: "skip" },
};

export const SRC_META: Record<string, { name: string; cls: string }> = {
  netease: { name: "网易云", cls: "netease" },
  qq: { name: "QQ音乐", cls: "qq" },
  kugou: { name: "酷狗", cls: "kugou" },
  kuwo: { name: "酷我", cls: "kuwo" },
};

/**
 * 「已有歌词」列：**动手之前文件里已经有什么**。
 *
 * 这一列存在的唯一目的是防止重复保存——用户扫完一遍库，往往不知道
 * 哪些歌本来就有歌词（下载时自带、或别的工具写过）。它与「状态」列是两回事：
 * 状态说「我们做到哪一步」，它说「文件本来是什么样」。
 */
export const LYRICS_META: Record<
  LyricsPresence,
  { label: string; cls: "none" | "lrc" | "file"; hint: string }
> = {
  none: {
    label: "—",
    cls: "none",
    hint: "",
  },
  sidecarLrc: {
    label: ".lrc",
    cls: "lrc",
    hint: "旁边已有同名 .lrc 文件。保存到歌曲文件不会动它；另存为 .lrc 时会替换它。",
  },
  embeddedTag: {
    label: "文件内",
    cls: "file",
    hint: "歌曲文件里已经有歌词。默认不会再写一遍——要覆盖就打开设置里的「歌曲已有歌词时覆盖」。",
  },
  both: {
    label: "文件内 + .lrc",
    cls: "file",
    hint: "歌曲文件里和旁边的 .lrc 里都已经有歌词。保存前先确认是否需要覆盖。",
  },
};

/** 匹配度分档（与后端的 0.85 / 0.65 两个阈值一致） */
export function scoreClass(score: number): "hi" | "md" | "lo" {
  if (score >= 0.85) return "hi";
  if (score >= 0.65) return "md";
  return "lo";
}

/**
 * 把 LRC 文本渲染成带高亮时间戳的 HTML。
 * 只用于预览展示，不参与任何写盘。
 */
export function renderLrc(text: string | undefined): string {
  if (!text) return '<span style="color:var(--text-3)">暂无歌词数据</span>';
  return text
    .split("\n")
    .map((line) => {
      const m = line.match(/^(\[[\d:.]+\])(.*)$/);
      if (m) return `<span class="ts">${m[1]}</span>${esc(m[2])}`;
      return esc(line);
    })
    .join("\n");
}

export function sleep(ms: number): Promise<void> {
  return new Promise((r) => window.setTimeout(r, ms));
}
