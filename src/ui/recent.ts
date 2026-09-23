/**
 * 「最近使用的文件夹」下拉。
 *
 * 点右侧小箭头展开，自己挑一个目录开始读取——用过的目录往往不止一个，
 * 「点一下就猜你要哪个」的交互在这里没有意义。
 *
 * 用原生按钮 + 绝对定位的列表实现，不引第三方菜单库：这一层需要的只有
 * 「展开 / 收起 / 上下键走一遍」，而这个前端刻意保持零依赖。
 */

import { esc, must } from "../dom.js";
import { S } from "../store.js";
import { invoke } from "../tauri.js";

export interface RecentMenuHandlers {
  /** 用户挑了一个目录 */
  onPick(path: string): void;
}

let handlers: RecentMenuHandlers | null = null;

export function initRecentMenu(h: RecentMenuHandlers): void {
  handlers = h;
  const menu = must("#recentMenu");
  const btn = must("#btnRecent");

  btn.addEventListener("click", (e) => {
    // 不拦住冒泡的话，下面的「点别处关闭」会立刻把它关掉
    e.stopPropagation();
    void toggle();
  });

  menu.addEventListener("click", (e) => {
    const path = (e.target as HTMLElement).closest<HTMLElement>("[data-path]")?.dataset.path;
    if (!path) return;
    close();
    handlers?.onPick(path);
  });

  // 上下键在列表里走一遍（列表项本身是 button，Enter 天然可用）
  menu.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = Array.from(menu.querySelectorAll<HTMLElement>("[data-path]"));
    if (items.length === 0) return;
    const at = items.indexOf(document.activeElement as HTMLElement);
    const step = e.key === "ArrowDown" ? 1 : -1;
    items[(at + step + items.length) % items.length]?.focus();
  });

  document.addEventListener("click", (e) => {
    if (isOpen() && !menu.contains(e.target as Node)) close();
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && isOpen()) {
      close();
      btn.focus();
    }
  });
}

async function toggle(): Promise<void> {
  if (isOpen()) {
    close();
    return;
  }
  const paths = await invoke<string[]>("recent_paths").catch(() => [] as string[]);
  render(paths);
  open();
}

function isOpen(): boolean {
  return !must("#recentMenu").hidden;
}

function open(): void {
  const menu = must("#recentMenu");
  menu.hidden = false;
  must("#btnRecent").setAttribute("aria-expanded", "true");
  menu.querySelector<HTMLElement>("[data-path]")?.focus();
}

function close(): void {
  must("#recentMenu").hidden = true;
  must("#btnRecent").setAttribute("aria-expanded", "false");
}

function render(paths: string[]): void {
  const menu = must("#recentMenu");
  if (paths.length === 0) {
    menu.innerHTML = '<div class="rm-empty">还没有用过的文件夹</div>';
    return;
  }

  menu.innerHTML = paths
    .map((p) => {
      const name = baseName(p);
      // 当前正在看的目录标出来——否则光看路径分不清哪个是「现在这个」
      const current = p === S.root;
      return `<button class="recentitem" type="button" data-path="${esc(p)}" title="${esc(p)}">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z"/></svg>
        <span class="rm-txt">
          <span class="rm-name">${esc(name)}</span>
          ${name === p ? "" : `<span class="rm-path">${esc(p)}</span>`}
        </span>
        ${current ? '<span class="rm-cur">当前</span>' : ""}
      </button>`;
    })
    .join("");
}

/** 列表主标题取末级目录名；盘符根这类没有末级的，直接用整串 */
function baseName(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts.length >= 2 ? (parts[parts.length - 1] ?? p) : p;
}
