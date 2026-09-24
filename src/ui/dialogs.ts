/**
 * 三个弹窗：保存确认（流程 D）、手动搜索（流程 C）、完成报告（§6.5.3）。
 */

import { candidateCardHtml, esc, fmtSize, must, mustInput, renderLrc, toast } from "../dom.js";
import { S, planWrite, saveSettings } from "../store.js";
import { invoke } from "../tauri.js";
import type {
  CandidateDto,
  MatchReport,
  PreviewDto,
  Settings,
  WritePlanDto,
  WriteReport,
} from "../types.js";

/** 设置开关的键 ↔ 设置对象的路径。弹窗与设置页共用同一份状态（§6.4）。 */
export type SettingKey =
  | "save_target"
  | "include_translation"
  | "overwrite_existing"
  | "fill_missing_info"
  | "embed_cover";

export function readSetting(key: SettingKey): boolean | "file" | "sidecar" {
  switch (key) {
    case "save_target":
      return S.settings.lyrics.save_target;
    case "include_translation":
      return S.settings.lyrics.include_translation;
    case "overwrite_existing":
      return S.settings.lyrics.overwrite_existing;
    case "fill_missing_info":
      return S.settings.write.fill_missing_info;
    case "embed_cover":
      return S.settings.write.embed_cover;
  }
}

/** 改动立即写回设置并持久化——不是「只对这一次生效」，避免用户以为改了却下次又变回去。 */
export async function writeSetting(key: SettingKey, value: boolean | "file" | "sidecar"): Promise<void> {
  const next: Settings = structuredClone(S.settings);
  switch (key) {
    case "save_target":
      next.lyrics.save_target = value === "sidecar" ? "sidecar" : "file";
      break;
    case "include_translation":
      next.lyrics.include_translation = Boolean(value);
      break;
    case "overwrite_existing":
      next.lyrics.overwrite_existing = Boolean(value);
      break;
    case "fill_missing_info":
      next.write.fill_missing_info = Boolean(value);
      break;
    case "embed_cover":
      next.write.embed_cover = Boolean(value);
      break;
  }
  await saveSettings(next);
}

// ════════════════════════════════════════════════════════════════════════
// 保存确认弹窗
// ════════════════════════════════════════════════════════════════════════

export interface PlanDialogHandlers {
  onConfirm(trackIds: number[]): void;
}

let planHandlers: PlanDialogHandlers | null = null;
let planTrackIds: number[] = [];
let planBusy = false;

export function initPlanDialog(h: PlanDialogHandlers): void {
  planHandlers = h;

  // 「保存到」切换
  must("#dlgTarget")
    .querySelectorAll<HTMLElement>(".segi")
    .forEach((btn) => {
      btn.addEventListener("click", async () => {
        await writeSetting("save_target", btn.dataset.v === "sidecar" ? "sidecar" : "file");
        syncPlanDialog();
        await refreshPlan();
      });
    });

  // 四个开关
  document.querySelectorAll<HTMLElement>("[data-dlg]").forEach((sw) => {
    sw.addEventListener("click", async () => {
      const key = sw.dataset.dlg as SettingKey;
      const now = readSetting(key);
      const nextValue = typeof now === "boolean" ? !now : true;
      await writeSetting(key, nextValue);
      syncPlanDialog();
      await refreshPlan();
    });
  });

  must("#planConfirm").addEventListener("click", () => {
    if (planBusy) return;
    closeScrim("#scrimPlan");
    planHandlers?.onConfirm(planTrackIds);
  });
}

/** 打开保存确认弹窗（§6.4 流程 D） */
export async function openPlanDialog(trackIds: number[]): Promise<void> {
  if (trackIds.length === 0) {
    toast("还没有可以保存的歌曲，先点「开始匹配」");
    return;
  }
  planTrackIds = trackIds;
  syncPlanDialog();
  openScrim("#scrimPlan");
  await refreshPlan();
}

/** 把弹窗控件与当前设置同步（设置页改了，弹窗也要跟着变） */
export function syncPlanDialog(): void {
  document.querySelectorAll<HTMLElement>("#dlgTarget .segi").forEach((b) => {
    b.classList.toggle("on", b.dataset.v === S.settings.lyrics.save_target);
  });
  document.querySelectorAll<HTMLElement>("[data-dlg]").forEach((sw) => {
    const key = sw.dataset.dlg as SettingKey;
    sw.classList.toggle("on", readSetting(key) === true);
  });
}

async function refreshPlan(): Promise<void> {
  planBusy = true;
  try {
    const plan = await planWrite(planTrackIds);
    renderPlanSummary(plan);
  } catch (e) {
    toast(errorText(e));
  } finally {
    planBusy = false;
  }
}

function renderPlanSummary(plan: WritePlanDto): void {
  const sidecar = plan.target === "sidecar";
  must("#planTotal").textContent = String(plan.total);
  must("#planTargetText").textContent = sidecar ? "另存为 .lrc 文件" : "写入歌曲文件";
  must("#planDeltaSize").textContent = plan.deltaBytes > 0 ? fmtSize(plan.deltaBytes) : "0";

  const warn = must("#planWarn");
  const lines: string[] = [];
  if (plan.skipped > 0) {
    // 「另有 15 首会被跳过」等于什么都没说：用户需要知道**为什么**
    // 以及下一句该做什么，否则只会以为软件坏了。
    const why: string[] = [];
    if (plan.skippedExisting > 0) {
      why.push(`${plan.skippedExisting} 首歌曲里已经有歌词——要覆盖就打开「歌曲已有歌词时覆盖」`);
    }
    if (plan.locked > 0) {
      why.push(`${plan.locked} 首正被其他程序使用——关掉播放器后重试`);
    }
    if (plan.skippedUnsupported > 0) {
      why.push(`${plan.skippedUnsupported} 首的格式不支持写入——可以改成「另存为 .lrc 文件」`);
    }
    if (plan.skippedNoMatch > 0) {
      why.push(`${plan.skippedNoMatch} 首还没有匹配到歌词——先点「开始匹配」`);
    }
    const head = `另有 ${plan.skipped} 首不会被保存：`;
    lines.push(why.length > 0 ? head + why.join("；") : head + "原因见「查看日志」");
  }
  if (sidecar) {
    lines.push("另存为 .lrc 文件不会改动歌曲本身，因此在弹窗里关掉的两项不会生效");
  }
  if (lines.length === 0) {
    warn.hidden = true;
    warn.innerHTML = "";
  } else {
    warn.hidden = false;
    warn.innerHTML = `<div class="planwarn">${lines.map(esc).join("<br>")}</div>`;
  }
}

// ════════════════════════════════════════════════════════════════════════
// 手动搜索弹窗
// ════════════════════════════════════════════════════════════════════════

export interface ManualDialogHandlers {
  onPicked(trackId: number, candidate: CandidateDto): void;
}

let manualHandlers: ManualDialogHandlers | null = null;
let manualTrackId: number | null = null;
let manualCandidates: CandidateDto[] = [];
let manualPick: CandidateDto | null = null;

export function initManualDialog(h: ManualDialogHandlers): void {
  manualHandlers = h;

  must("#manualGo").addEventListener("click", () => {
    void runManualSearch(mustInput("#manualQuery").value);
  });
  mustInput("#manualQuery").addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") void runManualSearch(mustInput("#manualQuery").value);
  });

  must("#manualList").addEventListener("click", (e) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>(".cand");
    if (!el) return;
    const idx = Number(el.dataset.cand);
    void selectManualCandidate(idx);
  });

  must("#manualConfirm").addEventListener("click", () => {
    if (!manualPick || manualTrackId === null) return;
    closeScrim("#scrimSearch");
    manualHandlers?.onPicked(manualTrackId, manualPick);
  });
}

export function openManualDialog(trackId: number, defaultQuery: string): void {
  manualTrackId = trackId;
  manualPick = null;
  manualCandidates = [];
  must("#manualList").innerHTML = "";
  must("#manualCount").innerHTML = "";
  must("#manualPreview").innerHTML = "";
  must("#manualHint").textContent = "选中后可先预览歌词";
  (must("#manualConfirm") as HTMLButtonElement).disabled = true;
  mustInput("#manualQuery").value = defaultQuery;
  openScrim("#scrimSearch");
  void runManualSearch(defaultQuery);
}

async function runManualSearch(keyword: string): Promise<void> {
  if (manualTrackId === null) return;
  must("#manualCount").innerHTML = '<span style="font-size:11.5px;color:var(--text-3)">正在搜索…</span>';
  must("#manualList").innerHTML = "";

  try {
    const list = await invoke<CandidateDto[]>("search_candidates", {
      args: { trackId: manualTrackId, keyword: keyword.trim().length ? keyword.trim() : null },
    });
    manualCandidates = list;
    renderManualList();
  } catch (e) {
    must("#manualCount").innerHTML = `<span style="font-size:11.5px;color:var(--st-failed)">${esc(errorText(e))}</span>`;
  }
}

function renderManualList(): void {
  must("#manualCount").innerHTML = `<span style="font-size:11.5px;color:var(--text-3)">共找到 ${manualCandidates.length} 条可能的歌词</span>`;

  if (manualCandidates.length === 0) {
    must("#manualList").innerHTML =
      '<div class="nocand">没有找到结果。换一个关键词再试试，例如只输入歌名</div>';
    return;
  }

  must("#manualList").innerHTML = manualCandidates
    .map((c, i) => candidateCardHtml(c, i, { on: false, pickable: true }))
    .join("");
}

/** 选一条 → 预览。**不立即生效**，用户点「使用这一条」才落地。 */
async function selectManualCandidate(index: number): Promise<void> {
  const cand = manualCandidates[index];
  if (!cand) return;
  manualPick = cand;

  must("#manualList")
    .querySelectorAll<HTMLElement>(".cand")
    .forEach((el, i) => {
      el.classList.toggle("on", i === index);
      const rk = el.querySelector<HTMLElement>(".rk");
      if (rk) rk.classList.toggle("checked", i === index);
    });

  must("#manualHint").textContent = "预览下面确认一下，再点「使用这一条」";
  must("#manualPreview").innerHTML = '<div style="font-size:11.5px;color:var(--text-3)">正在获取歌词…</div>';

  try {
    const preview = await invoke<PreviewDto>("preview_candidate", { candidate: cand });
    must("#manualPreview").innerHTML = `
      <div class="dsec-t">歌词预览 <div class="grow"></div>
        <span style="text-transform:none;font-size:10.5px;letter-spacing:0">${preview.lines} 行 · ${fmtSize(preview.bytes)}</span>
      </div>
      <div class="lrcbox">${renderLrc(preview.text)}</div>`;
    (must("#manualConfirm") as HTMLButtonElement).disabled = false;
  } catch (e) {
    must("#manualPreview").innerHTML = `<div class="banner err">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 9v4M12 17h.01"/></svg>
      <div>${esc(errorText(e))}</div></div>`;
    (must("#manualConfirm") as HTMLButtonElement).disabled = true;
  }
}

// ════════════════════════════════════════════════════════════════════════
// 完成报告弹窗
// ════════════════════════════════════════════════════════════════════════

export function showMatchReport(report: MatchReport): void {
  must("#doneTitle").textContent = report.sourcesDown
    ? "没有连上歌词服务"
    : report.cancelled
      ? "已停止"
      : "匹配完成";
  must("#repOkLabel").textContent = "找到歌词";
  must("#repOk").textContent = String(report.matched + report.confirm);
  must("#repSkip").textContent = String(report.skipped);
  must("#repFailWrap").hidden = true;

  const rows: Array<{ k: string; v: string; c: string }> = [];
  if (report.sourcesDown) {
    rows.push({
      k: "网络不可用或歌词服务暂时无法访问",
      v: "请稍后重试",
      c: "var(--st-failed)",
    });
  } else {
    rows.push({
      k: `${report.matched} 首可以直接保存`,
      v: "点「保存歌词」把它们写进歌曲",
      c: "var(--st-matched)",
    });
    if (report.confirm > 0) {
      rows.push({
        k: `${report.confirm} 首需要你确认`,
        v: "在左侧「待处理」里逐条挑选",
        c: "var(--st-confirm)",
      });
    }
    if (report.failed > 0) {
      rows.push({
        k: `${report.failed} 首没有找到歌词`,
        v: "可以试试手动搜索",
        c: "var(--st-failed)",
      });
    }
    if (report.skipped > 0) {
      rows.push({ k: `${report.skipped} 首是纯音乐`, v: "不需要歌词，已跳过", c: "var(--text-3)" });
    }
  }
  renderReportRows(rows);
  openScrim("#scrimDone");
}

export function showWriteReport(report: WriteReport): void {
  must("#doneTitle").textContent = report.cancelled ? "已停止" : "保存完成";
  must("#repOkLabel").textContent = "保存成功";
  must("#repOk").textContent = String(report.ok);
  must("#repSkip").textContent = String(report.skipped);
  // 跳过与失败是两回事，混在一个数字里会让人以为「跳过」= 出错了
  must("#repFailWrap").hidden = report.failed === 0;
  must("#repFail").textContent = String(report.failed);

  const rows: Array<{ k: string; v: string; c: string }> = [];
  const sidecar = report.target === "sidecar";
  rows.push({
    k: sidecar ? "歌词已保存为 .lrc 文件" : "歌词已保存到歌曲文件",
    v: sidecar ? "与歌曲放在同一个文件夹" : "用任意播放器都能看到",
    c: "var(--st-matched)",
  });
  if (!sidecar) {
    rows.push({ k: "歌曲原有的内容和封面", v: "保持不变", c: "var(--st-matched)" });
  }
  if (report.bytesDelta > 0) {
    rows.push({ k: "文件体积共增加", v: fmtSize(report.bytesDelta), c: "var(--text-2)" });
  }

  // 跳过按原因分列，并给出**下一步该做什么**——只报数字等于什么都没说
  if (report.skippedExisting > 0) {
    rows.push({
      k: `${report.skippedExisting} 首歌曲里已经有歌词`,
      v: "要覆盖就打开「歌曲已有歌词时覆盖」",
      c: "var(--st-confirm)",
    });
  }
  if (report.locked > 0) {
    rows.push({
      k: `${report.locked} 首正被其他程序使用`,
      v: "关掉播放器后重新保存一次",
      c: "var(--st-confirm)",
    });
  }
  if (report.skippedUnsupported > 0) {
    rows.push({
      k: `${report.skippedUnsupported} 首的格式不支持写入`,
      v: "可以在设置里改成「另存为 .lrc 文件」",
      c: "var(--st-confirm)",
    });
  }
  if (report.skippedNoMatch > 0) {
    rows.push({
      k: `${report.skippedNoMatch} 首还没有匹配到歌词`,
      v: "先点「开始匹配」再保存",
      c: "var(--st-confirm)",
    });
  }

  for (const f of report.failures.slice(0, 20)) {
    rows.push({ k: f.title, v: f.reason, c: "var(--st-failed)" });
  }
  if (report.failures.length > 20) {
    rows.push({
      k: `还有 ${report.failures.length - 20} 首未列出`,
      v: "可以在「查看日志」里看到全部",
      c: "var(--text-3)",
    });
  }
  renderReportRows(rows);
  openScrim("#scrimDone");
}

function renderReportRows(rows: Array<{ k: string; v: string; c: string }>): void {
  must("#repList").innerHTML = rows
    .map(
      (r) => `<div class="planrow">
      <span class="ck" style="background:${r.c}">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3.5" stroke-linecap="round"><path d="M20 6L9 17l-5-5"/></svg></span>
      <span class="nm">${esc(r.k)}</span>
      <span style="font-size:12px;color:var(--text-3)">${esc(r.v)}</span>
    </div>`,
    )
    .join("");
}

// ════════════════════════════════════════════════════════════════════════
// 通用
// ════════════════════════════════════════════════════════════════════════

export function openScrim(sel: string): void {
  must(sel).classList.add("on");
}

export function closeScrim(sel: string): void {
  must(sel).classList.remove("on");
}

export function initScrimDismiss(): void {
  document.querySelectorAll<HTMLElement>("[data-close]").forEach((btn) => {
    btn.addEventListener("click", () => btn.closest(".scrim")?.classList.remove("on"));
  });
  document.querySelectorAll<HTMLElement>(".scrim").forEach((scrim) => {
    scrim.addEventListener("click", (e) => {
      if (e.target === scrim) scrim.classList.remove("on");
    });
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      document.querySelectorAll<HTMLElement>(".scrim.on").forEach((s) => s.classList.remove("on"));
    }
  });
}

/** 后端的错误已经是面向用户的中文句子，直接展示 */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
