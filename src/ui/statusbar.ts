/** 状态栏与日志抽屉（§6.2 信息架构：进度由状态栏承担，任务历史不持久化）。 */

import { esc, must } from "../dom.js";
import type { LibraryStats, LogDto } from "../types.js";

/** 日志环形缓冲上限。与设计文档 §5 的「前端环形缓冲 2000 条」一致。 */
const LOG_LIMIT = 2000;

let logCount = 0;
/** 速率估算的起点。每次新任务开始时重置。 */
let rateStart = 0;

export function initStatusBar(): void {
  must("#btnLog").addEventListener("click", () => {
    must("#logDrawer").classList.toggle("on");
  });
}

/**
 * 设置左侧的表述与指示点。
 * `running` 为真时用旋转指示器（表示「正在做事」），否则用静态圆点。
 */
export function setPhase(text: string, color: string, running: boolean): void {
  must("#stPhaseText").textContent = text;
  const dot = must("#stDot");
  if (running) {
    if (!dot.classList.contains("spin")) {
      dot.outerHTML = '<span class="spin" id="stDot"></span>';
    }
    return;
  }
  if (!dot.classList.contains("spin")) {
    dot.setAttribute("style", `width:12px;height:12px;border-radius:50%;background:${color};flex:0 0 auto`);
    return;
  }
  document.querySelector("#stDot")?.replaceWith(makeDot(color));
}

function makeDot(color: string): HTMLElement {
  const el = document.createElement("span");
  el.id = "stDot";
  el.setAttribute("style", `width:12px;height:12px;border-radius:50%;background:${color};flex:0 0 auto`);
  return el;
}

export function setProgress(done: number, total: number): void {
  const bar = must("#stBar");
  const label = must("#stProgress");
  const rate = must("#stRate");

  if (total <= 0) {
    bar.style.width = "0%";
    label.textContent = "";
    rate.textContent = "";
    return;
  }

  bar.style.width = `${Math.round((done / total) * 100)}%`;
  label.textContent = `${done} / ${total}`;

  // 速率：给用户一个「还要多久」的直觉（§6.2 状态栏）
  const elapsed = (Date.now() - rateStart) / 1000;
  if (elapsed > 1.5 && done > 0) {
    const perSec = done / elapsed;
    const remain = total > done ? ` · 约 ${Math.max(1, Math.round((total - done) / perSec))} 秒` : "";
    rate.textContent = `${perSec.toFixed(1)} 首/秒${remain}`;
  } else {
    rate.textContent = "";
  }
}

/** 状态栏的三个计数：已写入 / 待确认 / 未找到 */
export function updateStats(stats: LibraryStats): void {
  must("#stOk").textContent = String(stats.done);
  must("#stWarn").textContent = String(stats.confirm);
  must("#stErr").textContent = String(stats.failed);
}

/** 重置进度显示（新任务开始时） */
export function resetProgress(): void {
  rateStart = Date.now();
  setProgress(0, 0);
}

export function appendLog(level: LogDto["level"], message: string): void {
  const inner = must("#logInner");
  const line = document.createElement("div");
  line.className = `logline ${level}`;

  const now = new Date();
  const ts = [now.getHours(), now.getMinutes(), now.getSeconds()]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
  const label = { info: "INFO", warn: "WARN", err: "ERROR", ok: "OK" }[level] ?? "INFO";

  line.innerHTML = `<span class="t">${ts}</span><span class="l">${label}</span><span>${esc(message)}</span>`;
  inner.appendChild(line);
  logCount += 1;

  // 环形缓冲：超出上限就丢最早的
  while (logCount > LOG_LIMIT && inner.firstChild) {
    inner.removeChild(inner.firstChild);
    logCount -= 1;
  }
  inner.scrollTop = inner.scrollHeight;
}
