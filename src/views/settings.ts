/** 设置页（唯一一页，7 项）。 */

import { fmtSize, must, toast } from "../dom.js";
import { S, saveSettings } from "../store.js";
import { invoke } from "../tauri.js";
import { readSetting, syncPlanDialog, writeSetting } from "../ui/dialogs.js";

/** 设置开关 → 中文说明。开关本身只有「开/关」两个状态，含义靠描述传达。 */
const SWITCH_LABELS: Record<string, string> = {
  include_translation: "包含翻译歌词",
  overwrite_existing: "歌曲已有歌词时覆盖",
  fill_missing_info: "补全歌曲缺少的信息",
  embed_cover: "保存专辑封面到歌曲文件",
};

export function initSettings(): void {
  const themeSelect = must("#setTheme") as HTMLSelectElement;
  themeSelect.addEventListener("change", () => {
    void (async () => {
      const next = structuredClone(S.settings);
      next.general.theme = themeSelect.value as "system" | "light" | "dark";
      await saveSettings(next);
      applyTheme();
    })();
  });

  // 「歌词保存方式」分段选择器
  must("#segTarget")
    .querySelectorAll<HTMLElement>(".segi")
    .forEach((btn) => {
      btn.addEventListener("click", () => {
        void (async () => {
          await writeSetting("save_target", btn.dataset.v === "sidecar" ? "sidecar" : "file");
          syncSettings();
          syncPlanDialog();
        })();
      });
    });

  // 四个开关
  document.querySelectorAll<HTMLElement>(".sw[data-key]").forEach((sw) => {
    sw.addEventListener("click", () => {
      void (async () => {
        const key = sw.dataset.key!;
        const now = readSetting(key as Parameters<typeof readSetting>[0]);
        const nextValue = typeof now === "boolean" ? !now : true;
        await writeSetting(key as Parameters<typeof writeSetting>[0], nextValue);
        syncSettings();
        syncPlanDialog();
        // 设置变了，详情面板里「保存到哪儿」这类文案也要跟着变
        document.dispatchEvent(new CustomEvent("lyrictag:settings-changed"));
      })();
    });
  });

  must("#btnClearCache").addEventListener("click", () => {
    void (async () => {
      try {
        const freed = await invoke<number>("clear_cache");
        S.cacheBytes = 0;
        syncSettings();
        toast(freed > 0 ? `已清空歌词缓存，释放 ${fmtSize(freed)}` : "歌词缓存已经是空的");
      } catch (e) {
        toast(typeof e === "string" ? e : String(e));
      }
    })();
  });
}

/** 把当前设置反映到控件上。设置页与保存弹窗共享同一份状态（§6.4）。 */
export function syncSettings(): void {
  const s = S.settings;

  (must("#setTheme") as HTMLSelectElement).value = s.general.theme;

  must("#segTarget")
    .querySelectorAll<HTMLElement>(".segi")
    .forEach((b) => b.classList.toggle("on", b.dataset.v === s.lyrics.save_target));

  must("#targetHint").textContent =
    s.lyrics.save_target === "file"
      ? "歌词会保存进歌曲里，换播放器、换电脑都跟着走"
      : "在歌曲旁边生成一个同名的 .lrc 文件，不改动原文件";

  document.querySelectorAll<HTMLElement>(".sw[data-key]").forEach((sw) => {
    const key = sw.dataset.key!;
    sw.classList.toggle("on", readSetting(key as Parameters<typeof readSetting>[0]) === true);
    if (SWITCH_LABELS[key]) sw.title = SWITCH_LABELS[key];
  });

  must("#cacheUsage").textContent = `已用 ${fmtSize(S.cacheBytes)}`;
}

/** 主题：`system` 时跟随操作系统的深浅色偏好 */
export function applyTheme(): void {
  const pref = S.settings.general.theme;
  const dark =
    pref === "dark" ||
    (pref === "system" &&
      window.matchMedia?.("(prefers-color-scheme: dark)").matches === true);
  S.theme = dark ? "dark" : "light";
  document.documentElement.dataset.theme = S.theme;

  const icon = must("#themeIcon");
  icon.innerHTML = dark
    ? '<path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.2 2.2M16.9 16.9l2.2 2.2M19.1 4.9l-2.2 2.2M7.1 16.9l-2.2 2.2"/><circle cx="12" cy="12" r="4.2"/>'
    : '<path d="M21 12.8A9 9 0 1111.2 3a7 7 0 009.8 9.8z"/>';
}

/** 主题按钮：在浅色/深色之间手动切换（会写回设置） */
export function toggleTheme(): void {
  void (async () => {
    const next = structuredClone(S.settings);
    next.general.theme = S.theme === "light" ? "dark" : "light";
    await saveSettings(next);
    applyTheme();
    syncSettings();
  })();
}
