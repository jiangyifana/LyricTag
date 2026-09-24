/**
 * 应用入口：把界面接上后端，把事件接上界面。
 *
 * 这一层只做「连线」——所有业务规则都在后端，所有渲染逻辑都在 ui/ 与 views/。
 */

import { $, $$, must, mustInput, STATE_META, toast } from "./dom.js";
import {
  emptyStats,
  invalidateRows,
  isRowVisible,
  loadDetail,
  refresh,
  S,
  visibleRows,
  type FilterKey,
  type SortKey,
} from "./store.js";
import { appWindow, invoke, listen, pickDirectory } from "./tauri.js";
import type {
  CandidateDto,
  LogDto,
  ProgressDto,
  ScanDoneDto,
  ScanResultDto,
  Settings,
  SnapshotDto,
  TrackUpdatedDto,
  TaskDoneDto,
} from "./types.js";
import {
  errorText,
  initManualDialog,
  initPlanDialog,
  initScrimDismiss,
  openManualDialog,
  openPlanDialog,
  showMatchReport,
  showWriteReport,
} from "./ui/dialogs.js";
import { initDetail, renderDetail } from "./ui/detail.js";
import { initRecentMenu } from "./ui/recent.js";
import {
  appendLog,
  initStatusBar,
  resetProgress,
  setPhase,
  setProgress,
  updateStats,
} from "./ui/statusbar.js";
import {
  initTable,
  renderTable,
  repaintRows,
  scrollRowIntoView,
  syncBulkBar,
  syncCheckAll,
} from "./ui/table.js";
import { applyTheme, initSettings, syncSettings, toggleTheme } from "./views/settings.js";

// ── 任务进度：避免并发任务的进度事件互相覆盖 ──────────────────────────

let activeTasks = 0;

function beginTask(): void {
  activeTasks += 1;
  must("#btnStop").removeAttribute("disabled");
  must("#btnMatch").setAttribute("disabled", "disabled");
  must("#btnWrite").setAttribute("disabled", "disabled");
  must("#btnRefresh").setAttribute("disabled", "disabled");
}

function endTask(): void {
  activeTasks = Math.max(0, activeTasks - 1);
  if (activeTasks > 0) return;
  must("#btnStop").setAttribute("disabled", "disabled");
  must("#btnMatch").removeAttribute("disabled");
  must("#btnWrite").removeAttribute("disabled");
  must("#btnRefresh").removeAttribute("disabled");
  void refreshAll();
}

// ── 全量刷新 ─────────────────────────────────────────────────────────────

async function refreshAll(): Promise<void> {
  await refresh();
  renderSmartViews();
  syncToolbar();
  renderTable();
  syncCheckAll();
  syncBulkBar();
  updateStats(S.stats);
  renderDetail(S.detail?.id === S.selectedId ? S.detail : null);
}

// ── 左侧：智能视图 ───────────────────────────────────────────────────────

function renderSmartViews(): void {
  const c = S.stats;
  const items: Array<{ f: FilterKey; name: string; count: number; color: string }> = [
    { f: "all", name: "全部曲目", count: c.all, color: "var(--text-3)" },
    { f: "todo", name: "待处理", count: c.todo, color: "var(--st-confirm)" },
    { f: "completed", name: "已完成", count: c.completed, color: "var(--st-matched)" },
    { f: "problem", name: "未找到歌词", count: c.problem, color: "var(--st-failed)" },
  ];
  must("#smartViews").innerHTML = items
    .map(
      (x) => `<div class="treeitem ${S.filter === x.f ? "on" : ""}" data-view-filter="${x.f}">
      <span class="dot" style="background:${x.color}"></span>
      <span class="grow">${x.name}</span><span class="cnt">${x.count}</span></div>`,
    )
    .join("");
}

// ── 工具栏 ───────────────────────────────────────────────────────────────

function syncToolbar(): void {
  mustInput("#scanPath").value = S.root || S.settings.library.last_scan_path || "";
  const hasLibrary = S.rows.length > 0;
  if (!hasLibrary) {
    must("#btnMatch").setAttribute("disabled", "disabled");
    must("#btnWrite").setAttribute("disabled", "disabled");
  } else {
    must("#btnMatch").removeAttribute("disabled");
    must("#btnWrite").removeAttribute("disabled");
  }
}

/** 「开始匹配」的目标集合：勾选了就用勾选的，否则用当前筛选出的全部。
 *  匹配不改动任何文件，跟随当前筛选是符合直觉的——在「未找到歌词」里
 *  再点一次匹配，就该只匹配这些。 */
function matchTargetIds(): number[] {
  if (S.checked.size > 0) return [...S.checked];
  return visibleRows().map((r) => r.id);
}

/** 「保存歌词」的目标集合：勾选了就用勾选的，否则用**全部已匹配的歌曲**。
 *
 *  刻意不跟随当前筛选：保存是不可逆操作，用户点它的时候想的是「把匹配好的
 *  歌词都存下来」，而不是「把此刻碰巧显示在屏幕上的那几行存下来」。
 *
 *  范围 = 有匹配结果且已进入可写状态的曲子（已匹配 / 待确认 / 已写入）。
 *  失败（认定匹配错了）与跳过（纯音乐）不在其中——它们没有可写的内容；
 *  已写入的留在集合里，是为了让「改了设置再存一次」这件事仍然做得到，
 *  真的重复保存时弹窗也会说明「已有歌词」。 */
function writeTargetIds(): number[] {
  if (S.checked.size > 0) return [...S.checked];
  return S.rows
    .filter((r) => r.matched && (r.state === "matched" || r.state === "confirm" || r.state === "done"))
    .map((r) => r.id);
}

async function startScan(path: string): Promise<void> {
  if (!path) return;
  beginTask();
  setPhase("正在读取文件夹…", "var(--st-matching)", true);
  resetProgress();
  try {
    const result = await invoke<ScanResultDto>("scan_library", { path });
    appendLog("info", `读取完成，共找到 ${result.count} 首歌`);
    if (result.skipped > 0) {
      appendLog("warn", `有 ${result.skipped} 个文件读不出来，已跳过`);
    }
    setPhase("就绪", "var(--st-idle)", false);
  } catch (e) {
    appendLog("err", errorText(e));
    setPhase("出错了", "var(--st-failed)", false);
    toast(errorText(e));
  } finally {
    setProgress(0, 0);
    endTask();
  }
}

async function chooseFolder(): Promise<void> {
  try {
    const dir = await pickDirectory(S.root || S.settings.library.last_scan_path);
    if (dir) await startScan(dir);
  } catch (e) {
    toast(errorText(e));
  }
}

/**
 * 重新读取当前文件夹。
 *
 * 先把列表清空再扫描：否则新结果落地前界面上还挂着上一批数据，
 * 用户分不清看到的是新的还是旧的。没有选过文件夹时退化为「选择文件夹」。
 */
async function refreshFolder(): Promise<void> {
  const root = S.root || S.settings.library.last_scan_path;
  if (!root) {
    await chooseFolder();
    return;
  }
  clearLibrary();
  await startScan(root);
}

/** 清空列表与所有依附于行的状态（选择、勾选、详情、计数） */
function clearLibrary(): void {
  S.rows = [];
  S.checked.clear();
  S.selectedId = null;
  S.detail = null;
  S.stats = emptyStats();
  renderSmartViews();
  syncToolbar();
  renderTable();
  syncCheckAll();
  syncBulkBar();
  updateStats(S.stats);
  renderDetail(null);
}

async function startMatch(trackIds: number[]): Promise<void> {
  if (trackIds.length === 0) {
    toast("没有可以匹配的歌曲，先选择一个音乐文件夹");
    return;
  }
  beginTask();
  resetProgress();
  try {
    await invoke<number>("match_tracks", { trackIds });
  } catch (e) {
    endTask();
    toast(errorText(e));
  }
}

async function startWrite(trackIds: number[]): Promise<void> {
  if (trackIds.length === 0) {
    toast(
      S.rows.length === 0
        ? "还没有可以保存的歌曲，先选择一个音乐文件夹"
        : "没有需要保存的歌词了，先点「开始匹配」",
    );
    return;
  }
  await openPlanDialog(trackIds);
}

// ── 表格交互 ─────────────────────────────────────────────────────────────

function selectTrack(id: number): void {
  S.selectedId = id;
  renderTable();
  void loadDetail(id).then((d) => renderDetail(d));
}

function toggleCheck(id: number): void {
  if (S.checked.has(id)) S.checked.delete(id);
  else S.checked.add(id);
  renderTable();
  syncCheckAll();
  syncBulkBar();
}

function toggleAll(): void {
  const list = visibleRows();
  const allOn = list.length > 0 && list.every((r) => S.checked.has(r.id));
  for (const row of list) {
    if (allOn) S.checked.delete(row.id);
    else S.checked.add(row.id);
  }
  renderTable();
  syncCheckAll();
  syncBulkBar();
}

function sortBy(key: SortKey): void {
  S.sort.dir = S.sort.key === key ? (S.sort.dir === 1 ? -1 : 1) : 1;
  S.sort.key = key;
  renderTable();
}

/** 待确认循环：处理完一首后自动选中并滚动到下一首（§6.4 流程 C）
 *
 *  队列取**全部**待确认曲目，不跟随当前筛选与搜索——跟随筛选会在
 *  「未找到歌词」这类视图下得到空队列，让按钮看起来像坏了。 */
function goNextConfirm(fromId: number | null): void {
  const queue = S.rows.filter((r) => r.state === "confirm");

  if (queue.length === 0) {
    setPhase("全部确认完毕", "var(--st-matched)", false);
    appendLog("ok", "所有待确认的歌词都已经处理完了 🎉");
    // 刚处理掉的那首状态已经变了，详情要重新拉一次，
    // 否则面板上还写着「待确认」、还摆着「跳过这首」。
    if (S.selectedId !== null) selectTrack(S.selectedId);
    return;
  }

  const index = queue.findIndex((r) => r.id === fromId);
  const next = queue[(index + 1) % queue.length] ?? queue[0]!;
  revealRow(next.id);
  selectTrack(next.id);
  scrollRowIntoView(next.id);
}

/** 保证某一行在当前视图里看得见——被筛选或搜索挡住的「下一首」等于没跳 */
function revealRow(id: number): void {
  if (visibleRows().some((r) => r.id === id)) return;

  if (S.query) {
    S.query = "";
    mustInput("#searchBox").value = "";
  }
  // 仍看不见说明被智能视图筛掉了；待确认本来就属于「待处理」这一档
  if (!visibleRows().some((r) => r.id === id)) S.filter = "todo";

  renderSmartViews();
  renderTable();
  syncCheckAll();
}

// ── 事件订阅 ─────────────────────────────────────────────────────────────

async function wireEvents(): Promise<void> {
  await listen<ProgressDto>("progress", (p) => {
    switch (p.phase) {
      case "scanning":
        setPhase("正在读取文件夹…", "var(--st-matching)", true);
        setProgress(p.done, p.total);
        break;
      case "matching":
        setPhase("正在匹配…", "var(--st-matching)", true);
        setProgress(p.done, p.total);
        // 后端推进度时顺手带上了实时计数，状态栏直接用它，
        // 不必等任务结束才刷新
        updateStats({ ...S.stats, done: p.ok, confirm: p.warn, failed: p.err });
        break;
      case "writing":
        setPhase("正在保存…", "var(--st-writing)", true);
        setProgress(p.done, p.total);
        break;
      case "cancelled":
        setPhase("已停止", "var(--st-confirm)", false);
        break;
      case "failed":
        setPhase("无法连接", "var(--st-failed)", false);
        break;
      case "done":
        setPhase("就绪", "var(--st-idle)", false);
        setProgress(0, 0);
        break;
      default:
        break;
    }
  });

  // 单曲状态变更：只更新受影响的那一行，不整表重绘
  await listen<TrackUpdatedDto>("track:updated", (ev) => {
    const row = S.rows.find((r) => r.id === ev.trackId);
    if (!row) return;
    // 只有这一行变了：它的可见性变没变，就等于可见行数变没变——
    // 不必像以前那样在修改前后各对整表做一次筛选 + 排序
    const wasVisible = isRowVisible(row);

    row.state = ev.state;
    // 按「状态」列排序比较的是文案，它得跟着状态一起变，否则批量任务期间排序是旧的
    row.stateLabel = STATE_META[ev.state]?.label ?? row.stateLabel;
    // 「已有歌词」列跟着这一行走：保存完成后它会从「—」/「.lrc」变成「文件内」，
    // 与同一行刚变成的「已写入」对齐
    row.existingLyrics = ev.existingLyrics;
    // 后端只在**有可用的匹配结果**时才带上 matched（失败 / 跳过 / 评分被否决
    // 的行不带）。因此这里要能把它清掉——否则重新匹配失败之后，
    // 列表上那一列还挂着上一轮的结果，看起来像配好了。
    row.matched = ev.matched
      ? {
          provider: ev.matched.provider,
          providerName: "",
          score: ev.matched.score,
          title: ev.matched.title,
          artist: ev.matched.artists.join("/"),
          album: ev.matched.album ?? "",
          year: ev.matched.year,
          duration: Math.round((ev.matched.durationMs ?? 0) / 1000),
        }
      : undefined;
    if (ev.message !== undefined) row.message = ev.message;
    // 行对象是就地改的，visibleRows 的记忆结果要作废
    invalidateRows();

    // 状态变化可能让这一行进入/离开当前筛选。行数没变就只重绘可视窗口，
    // 这样批量任务期间列表不会每首歌都跳一次。
    if (isRowVisible(row) !== wasVisible) renderTable();
    else repaintRows();

    renderSmartViews();
    if (S.selectedId === ev.trackId && ev.state !== "matching" && ev.state !== "writing") {
      void loadDetail(ev.trackId).then((d) => renderDetail(d));
    }
  });

  await listen<LogDto>("log", (ev) => appendLog(ev.level, ev.message));

  await listen<ScanDoneDto>("scan:done", (ev) => {
    appendLog("ok", `已载入 ${ev.count} 首歌`);
  });

  await listen<TaskDoneDto>("task:done", (ev) => {
    if (ev.kind === "match") showMatchReport(ev.report);
    else showWriteReport(ev.report);
    endTask();
  });

  // 把文件夹拖到窗口上直接扫描（§3.2）
  await listen<{ paths: string[] }>("tauri://drag-drop", (ev) => {
    const first = ev.paths?.[0];
    if (first) void startScan(first);
  });
}

// ── 启动 ─────────────────────────────────────────────────────────────────

async function bootstrap(): Promise<void> {
  initStatusBar();
  initScrimDismiss();
  initTable({
    onSelect: selectTrack,
    onToggleCheck: toggleCheck,
    onToggleAll: toggleAll,
    onSort: sortBy,
    onVisibleChange: () => {
      syncCheckAll();
    },
  });
  initDetail({
    onPickCandidate: (index) => {
      if (S.detail) void pickCandidateIndex(S.detail.id, index);
    },
    onAccept: () => {
      void acceptCurrent();
    },
    onSkipThis: () => {
      void skipCurrent();
    },
    onMatchOne: () => {
      if (S.selectedId !== null) void startMatch([S.selectedId]);
    },
    onSaveOne: () => {
      if (S.selectedId !== null) void startWrite([S.selectedId]);
    },
    onRematch: () => {
      if (S.selectedId !== null) void startMatch([S.selectedId]);
    },
    onManual: () => openManualForSelection(),
  });
  initPlanDialog({
    onConfirm: (ids) => {
      beginTask();
      resetProgress();
      void invoke<number>("write_tracks", { trackIds: ids, options: S.settings }).catch((e) => {
        endTask();
        toast(errorText(e));
      });
    },
  });
  initManualDialog({
    onPicked: (trackId, candidate) => {
      void pickCandidate(trackId, candidate);
    },
  });
  initRecentMenu({
    onPick: (path) => void startScan(path),
  });
  initSettings();

  wireToolbar();
  wireNav();
  wireSearch();
  wireSmartViews();
  wireWindowControls();

  applyTheme();
  window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (S.settings.general.theme === "system") applyTheme();
  });
  document.addEventListener("lyrictag:settings-changed", () => {
    renderDetail(S.detail);
  });

  // 载入上次的曲库（若有），再拉一次全量快照
  try {
    const restored = await invoke<ScanResultDto | null>("load_library");
    if (restored && restored.count > 0) {
      appendLog("info", `已载入上次的音乐库：${restored.count} 首歌`);
      if (restored.restored > 0) {
        appendLog("info", `其中 ${restored.restored} 首保留了上次的匹配结果`);
      }
    }
  } catch (e) {
    appendLog("warn", `载入上次的曲库失败：${errorText(e)}`);
  }

  const snap = await refresh();
  applySnapshotToUi(snap);
  await wireEvents();

  setPhase("就绪", "var(--st-idle)", false);
  appendLog("info", "LyricTag 已启动");

  // 通知后端「界面起来了」。发布构建没有控制台，这条日志是白屏问题的唯一线索。
  await invoke("webview_ready").catch(() => undefined);

  // 首次使用：直接弹出文件夹选择框（§4.6.3）
  if (!snap.settings.general.first_run_done && snap.library.tracks.length === 0) {
    await invoke<Settings>("mark_first_run_done").catch(() => undefined);
  }
}

function applySnapshotToUi(snap: SnapshotDto): void {
  renderSmartViews();
  syncToolbar();
  renderTable();
  syncCheckAll();
  syncBulkBar();
  updateStats(snap.library.stats);
  syncSettings();
  applyTheme();

  // 启动时把选中行的详情也载入，否则右侧会一直停在空状态
  if (S.selectedId !== null) {
    void loadDetail(S.selectedId).then((d) => renderDetail(d));
  } else {
    renderDetail(null);
  }
}

function wireToolbar(): void {
  must("#btnScan").addEventListener("click", () => void chooseFolder());
  must("#btnRefresh").addEventListener("click", () => void refreshFolder());
  must("#emptyAction").addEventListener("click", () => void chooseFolder());

  must("#btnMatch").addEventListener("click", () => void startMatch(matchTargetIds()));
  must("#btnWrite").addEventListener("click", () => void startWrite(writeTargetIds()));

  must("#btnStop").addEventListener("click", () => {
    void (async () => {
      appendLog("warn", "收到停止指令，正在结束在途请求…");
      await invoke<boolean>("cancel_task", { taskId: null });
    })();
  });

  must("#bulkMatch").addEventListener("click", () => void startMatch([...S.checked]));
  must("#bulkWrite").addEventListener("click", () => void startWrite([...S.checked]));
  must("#bulkClear").addEventListener("click", () => {
    S.checked.clear();
    renderTable();
    syncCheckAll();
    syncBulkBar();
  });

  must("#btnManual").addEventListener("click", () => openManualForSelection());
}

function openManualForSelection(): void {
  const row = S.rows.find((r) => r.id === S.selectedId);
  if (!row) {
    toast("先在上面的列表里选一首歌");
    return;
  }
  openManualDialog(row.id, `${row.title} ${row.artist}`.trim());
}

function wireNav(): void {
  $$(".navtab").forEach((tab) => {
    tab.addEventListener("click", () => {
      $$(".navtab").forEach((t) => t.classList.remove("on"));
      tab.classList.add("on");
      $$(".view").forEach((v) => v.classList.remove("on"));
      must(`#view-${tab.dataset.view}`).classList.add("on");
      if (tab.dataset.view === "settings") syncSettings();
    });
  });
  must("#themeToggle").addEventListener("click", () => toggleTheme());
}

function wireSearch(): void {
  mustInput("#searchBox").addEventListener("input", (e) => {
    S.query = (e.target as HTMLInputElement).value.trim();
    renderTable();
  });
}

function wireSmartViews(): void {
  must("#smartViews").addEventListener("click", (e) => {
    const item = (e.target as HTMLElement).closest<HTMLElement>("[data-view-filter]");
    if (!item) return;
    S.filter = (item.dataset.viewFilter as FilterKey) ?? "all";
    renderSmartViews();
    renderTable();
    syncCheckAll();
  });
}

function wireWindowControls(): void {
  must("#winMin").addEventListener("click", () => void appWindow.minimize());
  must("#winMax").addEventListener("click", () => void appWindow.toggleMaximize());
  must("#winClose").addEventListener("click", () => void appWindow.close());

  // 双击标题栏最大化
  $(".titlebar")?.addEventListener("dblclick", (e) => {
    if ((e.target as HTMLElement).closest(".winbtns")) return;
    void appWindow.toggleMaximize();
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "d" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      toggleTheme();
    }
    if (e.key === "f" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      mustInput("#searchBox").focus();
    }
  });
}

// ── 候选挑选 ─────────────────────────────────────────────────────────────

/** 点候选：只切换选中项并刷新预览，不改变匹配结果（§6.4 流程 C） */
async function pickCandidateIndex(trackId: number, index: number): Promise<void> {
  const detail = await invoke<typeof S.detail>("set_candidate_pick", {
    trackId,
    pick: index,
  });
  S.detail = detail;
  renderDetail(detail);
}

/** 「使用这一条」：把选中的候选真正落地为该曲的匹配结果 */
async function acceptCurrent(): Promise<void> {
  const detail = S.detail;
  if (!detail) return;
  const candidate = detail.candidates[detail.pick] ?? detail.candidates[0];
  if (!candidate) {
    toast("这首歌还没有候选可选用");
    return;
  }
  await pickCandidate(detail.id, candidate);
}

async function pickCandidate(trackId: number, candidate: CandidateDto): Promise<void> {
  try {
    const detail = await invoke<typeof S.detail>("pick_candidate", { trackId, candidate });
    S.detail = detail;
    renderDetail(detail);
    await refreshRowViews();
    appendLog("ok", "已使用你选择的歌词，可以点「保存歌词」保存下来");
    goNextConfirm(trackId);
  } catch (e) {
    toast(errorText(e));
  }
}

/**
 * 「跳过这首」：**真的跳过**，不是「稍后再问一遍」。
 *
 * 状态落到「跳过」之后，列表上「匹配到的歌词」那一列随之清空（那一列只展示
 * 将被保存的候选），保存歌词也不会再带上它。想反悔就点「重新匹配」。
 */
async function skipCurrent(): Promise<void> {
  const id = S.selectedId;
  if (id === null) return;
  try {
    const detail = await invoke<typeof S.detail>("skip_track", { trackId: id });
    S.detail = detail;
    renderDetail(detail);
    await refreshRowViews();
    appendLog("info", "已跳过这首，保存歌词时不会再带上它");
    goNextConfirm(id);
  } catch (e) {
    toast(errorText(e));
  }
}

/** 单行状态变化之后，把受影响的几块刷新一遍 */
async function refreshRowViews(): Promise<void> {
  await refresh();
  renderTable();
  renderSmartViews();
  syncCheckAll();
  syncBulkBar();
  updateStats(S.stats);
}

// 启动
void bootstrap().catch((e) => {
  appendLog("err", `启动失败：${errorText(e)}`);
  toast(`启动失败：${errorText(e)}`);
});
