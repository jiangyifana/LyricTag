/**
 * 曲目详情面板与底部操作区。
 *
 * 这是「待确认」流程的核心（§6.4 流程 C）：点候选**只高亮 + 刷新预览，不立即生效**，
 * 用户确认后再点「使用这一条」——避免手快点错就写进去了。
 * 操作区固定在面板底部（不随内容滚动），选完候选不需要滚动去找按钮。
 */

import {
  candidateCardHtml,
  esc,
  fmtDur,
  fmtSize,
  must,
  renderLrc,
  scoreClass,
  STATE_META,
} from "../dom.js";
import { S } from "../store.js";
import type { TrackDetailDto } from "../types.js";

export interface DetailHandlers {
  onPickCandidate(index: number): void;
  onAccept(): void;
  onSkipThis(): void;
  onMatchOne(): void;
  onSaveOne(): void;
  onRematch(): void;
  onManual(): void;
}

let handlers: DetailHandlers | null = null;

export function initDetail(h: DetailHandlers): void {
  handlers = h;
  must("#detail").addEventListener("click", (e) => {
    const target = e.target as HTMLElement;

    const cand = target.closest<HTMLElement>("[data-cand]");
    if (cand) {
      handlers?.onPickCandidate(Number(cand.dataset.cand));
      return;
    }

    const act = target.closest<HTMLElement>("[data-act]");
    if (!act) return;
    switch (act.dataset.act) {
      case "accept":
        handlers?.onAccept();
        break;
      case "skip":
        handlers?.onSkipThis();
        break;
      case "match-one":
        handlers?.onMatchOne();
        break;
      case "save-one":
        handlers?.onSaveOne();
        break;
      case "rematch":
        handlers?.onRematch();
        break;
      case "manual":
        handlers?.onManual();
        break;
      default:
        break;
    }
  });
}

export function renderDetail(detail: TrackDetailDto | null): void {
  const scroll = must("#detailScroll");
  const actions = must("#detailActions");

  if (!detail) {
    actions.innerHTML = "";
    scroll.innerHTML = `<div class="empty">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
        <path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
      <p>从左侧列表选择一首歌曲<br>查看歌曲信息、候选歌词与歌词预览</p></div>`;
    return;
  }

  const meta = STATE_META[detail.state] ?? STATE_META["idle"]!;
  const pickable = isPickable(detail);
  // 不可选时连分数徽标都不给：那个数字取自候选，摆在曲目卡片上会被读成
  // 「这首歌的匹配度」——而它其实是「某条被否掉的候选的匹配度」。
  const selected = pickable ? (detail.candidates[detail.pick] ?? detail.candidates[0]) : undefined;
  const score = selected?.score ?? 0;

  scroll.innerHTML = `
    <div class="dsec">
      <div class="trackcard">
        <div class="cover">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6">
            <path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
          ${detail.hasCover ? "" : '<div class="cap">无封面</div>'}
        </div>
        <div class="tmeta">
          <div class="tt">${esc(detail.title)}</div>
          <div class="ta">${esc(detail.artist) || "未知艺术家"}</div>
          <div class="tb">${detail.album ? esc(detail.album) : '<span style="color:var(--st-confirm)">专辑未填</span>'}
            · ${detail.year ?? "年份未填"} · ${fmtDur(detail.duration)}</div>
          <div class="row">
            <span class="st ${meta.cls}"><span class="d"></span>${esc(meta.label)}</span>
            <span class="st skip" style="font-weight:500">${esc(detail.format)}</span>
            ${score > 0 ? `<span class="st ${scoreClass(score) === "hi" ? "matched" : scoreClass(score) === "md" ? "confirm" : "failed"}" style="font-weight:600">${score.toFixed(2)}</span>` : ""}
          </div>
        </div>
      </div>
    </div>

    ${detail.message ? banner(detail.message) : ""}
    ${detail.needsReview ? banner("这首歌的文件名信息不太确定，建议核对一下匹配结果") : ""}

    <div class="dsec">
      <div class="dsec-t">歌曲信息</div>
      <dl class="kv">
        <dt>文件</dt><dd class="mono" title="${esc(detail.path)}">${esc(detail.fileName)}</dd>
        <dt>大小</dt><dd class="num">${fmtSize(detail.fileSize)}</dd>
        <dt>音轨号</dt><dd>${detail.trackNo ?? '<span class="miss">音轨号</span>'}</dd>
        <dt>年份</dt><dd>${detail.year ?? '<span class="miss">年份</span>'}</dd>
        <dt>歌词</dt><dd>${esc(detail.lyricsLabel)}</dd>
      </dl>
      ${capabilityHints(detail)}
    </div>

    <div class="dsec">
      <div class="dsec-t">${detail.state === "confirm" ? "请选择正确的歌词" : "候选歌词"}
        <div class="grow"></div>
        <span style="text-transform:none;font-size:10.5px;letter-spacing:0">${detail.candidates.length} 条</span>
      </div>
      ${candidateNote(detail)}
      <div class="candlist ${detail.state === "confirm" ? "pick" : ""}">
        ${
          detail.candidates.length
            ? detail.candidates
                .map((c, i) => candidateCardHtml(c, i, { on: pickable && i === detail.pick, pickable }))
                .join("")
            : '<div class="nocand">还没有候选。点「匹配歌词」或「手动搜索」试试</div>'
        }
      </div>
    </div>

    ${previewBlock(detail)}
  `;

  actions.innerHTML = actionArea(detail);
}

/**
 * 候选能不能被选中。
 *
 * 待确认 = 就是要你挑一条；已匹配 / 已写入 = 可以换一条重配。
 * 失败与跳过的曲目**不参与选择**：那些候选只是「找到过什么」的说明，
 * 高亮其中一条会让人以为这首歌已经配上了歌词（§6.4 流程 C 的例外）。
 */
function isPickable(detail: TrackDetailDto): boolean {
  return detail.state === "confirm" || detail.state === "matched" || detail.state === "done";
}

/** 候选不可选时，说明「这些是什么、下一步该做什么」 */
function candidateNote(detail: TrackDetailDto): string {
  // 连候选都没有时不必解释「候选为什么没用」——下面那句引导已经够了
  if (isPickable(detail) || detail.candidates.length === 0) return "";

  const text =
    detail.state === "failed"
      ? "这些候选的匹配度都不够，没有被自动采用。想用其中一条：点「手动搜索」预览后再选。"
      : detail.state === "skip"
        ? "这首歌已被跳过，这些候选不会被保存。"
        : "";
  return text ? `<div class="candnote">${esc(text)}</div>` : "";
}

/** 歌词预览。失败 / 跳过没有「将会保存成什么样」可看，只给一句说明。 */
function previewBlock(detail: TrackDetailDto): string {
  if (!detail.preview && (detail.state === "failed" || detail.state === "skip")) {
    const text =
      detail.state === "failed"
        ? "这首歌还没有可用的歌词。"
        : "这首歌被跳过了，不需要歌词。";
    return `<div class="dsec"><div class="dsec-t">歌词预览</div>
      <div class="candnote">${esc(text)}</div></div>`;
  }

  return `<div class="dsec">
      <div class="dsec-t">歌词预览 <div class="grow"></div>
        <span style="text-transform:none;font-size:10.5px;letter-spacing:0">
          ${detail.preview ? `${detail.preview.lines} 行 · ${fmtSize(detail.preview.bytes)}` : "—"}</span></div>
      <div class="lrcbox">${renderLrc(detail.preview?.text)}</div>
      ${
        detail.preview?.hasTranslation
          ? '<div style="margin-top:7px;font-size:11px;color:var(--text-3)">这首歌带官方翻译歌词</div>'
          : ""
      }
      ${
        detail.preview?.hasVerbatim
          ? '<div style="margin-top:4px;font-size:11px;color:var(--text-3)">这首歌有逐字歌词，已按普通歌词保存</div>'
          : ""
      }
    </div>`;
}

function banner(text: string, kind: "warn" | "ok" | "err" = "warn"): string {
  const icon =
    kind === "ok"
      ? '<path d="M20 6L9 17l-5-5"/>'
      : '<path d="M12 9v4M12 17h.01M10.3 3.9L1.8 18a2 2 0 001.7 3h17a2 2 0 001.7-3L13.7 3.9a2 2 0 00-3.4 0z"/>';
  return `<div class="dsec"><div class="banner ${kind === "warn" ? "" : kind}">
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">${icon}</svg>
    <div>${esc(text)}</div></div></div>`;
}

/** 平台能力提示：说明哪些字段**该平台根本没有**，而不是让它看起来像「忘了填」 */
function capabilityHints(detail: TrackDetailDto): string {
  const caps = detail.capabilities;
  const notes: string[] = [];
  if (caps.noYear && detail.year == null) {
    notes.push("这首歌的信息来自酷我，该平台没有发行年份，因此这一项不会被补全。");
  }
  if (caps.noTrackNo && detail.trackNo == null) {
    notes.push("音轨号只有网易云提供，其他来源的歌曲这一项不会被补全。");
  }
  if (caps.noVerbatim && detail.preview?.hasVerbatim === false) {
    notes.push("逐字歌词只有网易云提供，这首歌会保存为普通歌词。");
  }
  if (notes.length === 0) return "";
  return `<div class="caphint">${notes.map((n) => `<div class="caphint-n">${esc(n)}</div>`).join("")}</div>`;
}

/** 底部操作区：按当前状态决定给用户什么动作 */
function actionArea(detail: TrackDetailDto): string {
  const manual = `<button class="btn" data-act="manual">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/></svg>
      手动搜索</button>`;
  const target = S.settings.lyrics.save_target === "file" ? "歌曲文件" : ".lrc 文件";

  switch (detail.state) {
    case "confirm": {
      const remain = S.stats.confirm;
      return `<div class="dsec actions">
        <div class="reviewbar">
          <div class="rb-t">这首歌有 ${detail.candidates.length} 个可能的歌词</div>
          <div class="rb-s">点上面任意一条可以预览歌词。选好后点「使用这一条」</div>
        </div>
        <div style="display:flex;gap:8px">
          <button class="btn primary" style="flex:1;justify-content:center" data-act="accept">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg>
            使用这一条
          </button>
          <button class="btn" data-act="skip">跳过这首</button>
        </div>
        <div class="reviewfoot">确认后自动跳到下一首 · 共 <b>${remain}</b> 首待确认</div>
      </div>`;
    }
    case "matched":
      return `<div class="dsec actions">
        <div class="reviewbar ok">
          <div class="rb-t">歌词已就绪</div>
          <div class="rb-s">保存到${target}后，用播放器就能看到</div>
        </div>
        <div style="display:flex;gap:8px">
          <button class="btn primary" style="flex:1;justify-content:center" data-act="save-one">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12M7 10l5 5 5-5M4 21h16"/></svg>
            保存歌词</button>
          ${manual}
        </div>
      </div>`;
    case "done":
      return `<div class="dsec actions">
        <div class="reviewbar ok">
          <div class="rb-t">这首歌已经保存好了</div>
          <div class="rb-s">歌词已保存到${target}。不满意可以重新匹配换一个版本</div>
        </div>
        <div style="display:flex;gap:8px">
          <button class="btn" style="flex:1;justify-content:center" data-act="rematch">重新匹配</button>
          ${manual}
        </div>
      </div>`;
    case "failed":
      return `<div class="dsec actions">
        <div class="reviewbar warn">
          <div class="rb-t">没有找到合适的歌词</div>
          <div class="rb-s">试试手动搜索，或换一个关键词</div>
        </div>
        <div style="display:flex;gap:8px">
          <button class="btn primary" style="flex:1;justify-content:center" data-act="manual">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/></svg>
            手动搜索</button>
          <button class="btn" data-act="rematch">重新匹配</button>
        </div>
      </div>`;
    case "idle":
      return `<div class="dsec actions" style="display:flex;gap:8px">
        <button class="btn primary" style="flex:1;justify-content:center" data-act="match-one">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/></svg>
          匹配歌词</button>
        ${manual}
      </div>`;
    case "skip":
      return `<div class="dsec actions" style="display:flex;gap:8px">
        <button class="btn" style="flex:1;justify-content:center" data-act="rematch">重新匹配</button>
        ${manual}
      </div>`;
    default:
      // 匹配中 / 写入中：只给「手动搜索」一个出口，避免用户在任务进行中触发冲突操作
      return `<div class="dsec actions" style="display:flex;gap:8px">
        <div style="flex:1;font-size:12px;color:var(--text-3);display:flex;align-items:center">
          ${
            detail.state === "matching"
              ? "正在查找歌词…"
              : detail.state === "writing"
                ? "正在保存歌词…"
                : ""
          }
        </div>
        ${manual}
      </div>`;
  }
}
