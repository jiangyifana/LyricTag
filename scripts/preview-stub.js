/*
 * 开发预览用的假后端。只在 .preview/ 里生效，不参与发布。
 *
 * 目的：在没有编译整个桌面应用的情况下，把**真实的前端代码**跑起来，
 * 用来检查界面渲染、状态配色、交互流程是否与设计一致。
 */
(function () {
  "use strict";

  var providers = ["netease", "qq", "kugou", "kuwo"];
  var providerNames = { netease: "网易云", qq: "QQ音乐", kugou: "酷狗", kuwo: "酷我" };
  var states = ["idle", "matching", "matched", "confirm", "writing", "done", "failed", "skip"];

  var curated = [
    { t: "夜曲", a: "周杰伦", al: "十一月的萧邦", y: 2005, n: 3, d: 227, fmt: "FLAC", st: "done", src: "netease", sc: 0.96, ex: "embeddedTag" },
    { t: "晴天", a: "周杰伦", al: "叶惠美", y: 2003, n: 2, d: 269, fmt: "FLAC", st: "confirm", src: "qq", sc: 0.78 },
    { t: "七里香", a: "周杰伦", al: "七里香", y: 2004, n: 1, d: 299, fmt: "MP3", st: "idle" },
    { t: "告白气球", a: "周杰伦", al: "周杰伦的床边故事", y: 2016, n: 2, d: 215, fmt: "MP3", st: "done", src: "netease", sc: 0.99, ex: "both" },
    { t: "稻香", a: "周杰伦", al: "魔杰座", y: 2008, n: 4, d: 223, fmt: "FLAC", st: "matched", src: "netease", sc: 0.94, ex: "sidecarLrc" },
    { t: "青花瓷", a: "周杰伦", al: "我很忙", y: 2007, n: 6, d: 239, fmt: "FLAC", st: "done", src: "netease", sc: 0.97, ex: "sidecarLrc" },
    { t: "慢慢喜欢你 (Live)", a: "马嘉祺", al: "", y: 2024, n: null, d: 248, fmt: "MP3", st: "confirm", src: "qq", sc: 0.71,
      warn: "这首歌是现场版，找到的候选是录音室版，需要你确认" },
    { t: "A Thousand Years", a: "Christina Perri", al: "The Twilight Saga", y: 2011, n: 5, d: 285, fmt: "WMA", st: "failed", src: null, sc: 0,
      warn: "这种格式不支持保存歌词" },
    { t: "Hide Away", a: "Daya", al: "Sit Still, Look Pretty", y: 2016, n: 4, d: 192, fmt: "MP3", st: "skip", src: null, sc: 0,
      warn: "纯音乐，无需歌词" },
    { t: "Bohemian Rhapsody", a: "Queen", al: "A Night at the Opera", y: 1975, n: 11, d: 354, fmt: "FLAC", st: "done", src: "netease", sc: 0.95 },
    { t: "Hotel California", a: "Eagles", al: "Hotel California", y: 1976, n: 1, d: 391, fmt: "FLAC", st: "matched", src: "kuwo", sc: 0.88 },
    { t: "起风了", a: "买辣椒也用券", al: "", y: 2017, n: null, d: 325, fmt: "MP3", st: "done", src: "kugou", sc: 0.92 },
    { t: "漠河舞厅", a: "柳爽", al: "1st.星球", y: 2021, n: 3, d: 299, fmt: "MP3", st: "done", src: "netease", sc: 0.98 },
    { t: "星辰大海", a: "黄霄雲", al: "", y: 2021, n: null, d: 214, fmt: "MP3", st: "failed", src: null, sc: 0,
      warn: "没有找到歌词，可以试试手动搜索" },
    { t: "如愿", a: "王菲", al: "我和我的父辈 电影原声带", y: 2021, n: 8, d: 265, fmt: "FLAC", st: "done", src: "netease", sc: 0.96 },
    { t: "江南", a: "林俊杰", al: "第二天堂", y: 2004, n: 2, d: 265, fmt: "FLAC", st: "matched", src: "qq", sc: 0.91 },
    { t: "修炼爱情", a: "林俊杰", al: "因你而在", y: 2013, n: 3, d: 308, fmt: "FLAC", st: "idle" },
    { t: "大眠", a: "王心凌", al: "CYNDILOVES2SING 爱。心凌", y: 2018, n: 7, d: 263, fmt: "MP3", st: "done", src: "netease", sc: 0.94 },
    { t: "Counting Stars", a: "OneRepublic", al: "Native", y: 2013, n: 5, d: 257, fmt: "M4A", st: "done", src: "netease", sc: 0.97 },
    { t: "Blinding Lights", a: "The Weeknd", al: "After Hours", y: 2020, n: 9, d: 200, fmt: "M4A", st: "matched", src: "kugou", sc: 0.93 },
    { t: "孤勇者", a: "陈奕迅", al: "孤勇者", y: 2021, n: 1, d: 256, fmt: "FLAC", st: "done", src: "netease", sc: 0.99 },
    { t: "富士山下", a: "陈奕迅", al: "What's Going On…?", y: 2006, n: 2, d: 261, fmt: "FLAC", st: "confirm", src: "kuwo", sc: 0.73,
      warn: "找到的候选和这首歌时长对不上，可能是不同的版本" },
    { t: "突然好想你", a: "五月天", al: "后青春期的诗", y: 2008, n: 4, d: 341, fmt: "MP3", st: "done", src: "netease", sc: 0.95 },
    { t: "贏", a: "", al: "", y: null, n: 10, d: 214, fmt: "M4A", st: "matched", src: "qq", sc: 0.88 },
    { t: "星夢", a: "", al: "", y: null, n: 12, d: 236, fmt: "M4A", st: "idle" }
  ];

  var lrcPool = [
    "[00:00.00] 作词 : 方文山\n[00:01.00] 作曲 : 周杰伦\n[00:03.42] 一群嗜血的蚂蚁 被腐肉所吸引\n[00:07.85] 我面无表情 看孤独的风景\n[00:12.30] 失去你 爱恨开始分明\n[00:16.72] 失去你 还有什么事好关心\n[00:21.15] 当鸽子不再象征和平\n[00:25.58] 我终于被提醒 广场上喂食的是秃鹰",
    "[00:00.00] 作词 : 周杰伦\n[00:01.00] 作曲 : 周杰伦\n[00:03.10] 故事的小黄花 从出生那年就飘着\n[00:11.60] 童年的荡秋千 随记忆一直晃到现在\n[00:20.20] 吹着前奏望着天空\n[00:26.40] 我想起花瓣试着掉落"
  ];

  function hash(s) {
    var h = 0;
    for (var i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
    return h;
  }
  function rnd(seed, i) {
    var x = Math.sin(seed + i * 9301) * 43758.5453;
    return x - Math.floor(x);
  }

  // 构造曲库：24 首人工 + 约 80 首生成，凑出一定的规模感
  var rows = [];
  var id = 0;
  curated.forEach(function (c) {
    id += 1;
    rows.push({
      id: id,
      title: c.t,
      artist: c.a,
      duration: c.d,
      format: c.fmt,
      state: c.st,
      stateLabel: { idle: "未处理", matching: "匹配中", matched: "已匹配", confirm: "待确认", writing: "写入中", done: "已写入", failed: "失败", skip: "跳过" }[c.st],
      mismatch: c.t.indexOf("(Live)") >= 0 || c.t === "贏",
      message: c.warn,
      // 文件里本来带着什么歌词：下载时自带、或别的工具写过——
      // 与「状态」列是两回事，这一列就是为了让人别重复保存
      existingLyrics: c.ex || "none",
      matched: c.src
        ? {
            provider: c.src,
            providerName: providerNames[c.src],
            score: c.sc,
            title: c.t.indexOf("(Live)") >= 0 ? "慢慢喜欢你" : c.t,
            artist: c.a || "未知",
            album: c.al || "同名专辑",
            year: c.y || undefined,
            duration: c.d
          }
        : undefined,
      _lrc: lrcPool[id % 2],
      _meta: c
    });
  });

  var extraTitles = ["给我一首歌的时间", "说好的幸福呢", "蒲公英的约定", "珊瑚海", "彩虹", "发如雪", "菊花台", "千里之外", "听妈妈的话", "夜的第七章", "退后", "白色风车", "心墙", "小酒窝", "背对背拥抱", "可惜没如果", "关键词", "交换余生", "伟大的渺小"];
  var artists = ["周杰伦", "林俊杰", "陈奕迅", "五月天", "邓紫棋", "薛之谦", "李荣浩", "毛不易", "Taylor Swift", "Adele"];
  var albums = ["精选集", "同名专辑", "Live 演唱会", "OST 原声带", "Remastered 2011", "Deluxe Edition", "单曲"];
  var formats = ["MP3", "FLAC", "FLAC", "MP3", "M4A", "OGG", "WMA"];

  for (var i = 0; i < 80; i++) {
    var t = extraTitles[i % extraTitles.length];
    var a = artists[Math.floor(rnd(i, 1) * artists.length)];
    var sd = hash(t + a + i);
    var st = states[Math.floor(rnd(sd, 3) * states.length)];
    var src = ["matched", "done", "confirm"].indexOf(st) >= 0 ? providers[Math.floor(rnd(sd, 4) * 4)] : null;
    id += 1;
    rows.push({
      id: id,
      title: t,
      artist: a,
      duration: 150 + Math.floor(rnd(sd, 11) * 250),
      format: formats[Math.floor(rnd(sd, 6) * formats.length)],
      state: st,
      stateLabel: { idle: "未处理", matching: "匹配中", matched: "已匹配", confirm: "待确认", writing: "写入中", done: "已写入", failed: "失败", skip: "跳过" }[st],
      mismatch: false,
      existingLyrics:
        st === "done" ? (rnd(sd, 12) > 0.5 ? "embeddedTag" : "both")
        : rnd(sd, 12) > 0.72 ? "sidecarLrc"
        : "none",
      matched: src
        ? {
            provider: src,
            providerName: providerNames[src],
            score: 0.62 + rnd(sd, 5) * 0.38,
            title: t,
            artist: a,
            album: albums[Math.floor(rnd(sd, 2) * albums.length)],
            year: 1998 + Math.floor(rnd(sd, 8) * 28),
            duration: 150 + Math.floor(rnd(sd, 11) * 250)
          }
        : undefined,
      _lrc: lrcPool[i % 2]
    });
  }

  // 每次快照都重算：预览里也能「使用这一条 / 跳过这首」，计数必须跟着走，
  // 否则侧栏和状态栏会停在打开页面那一刻
  function computeStats() {
    var s = { all: rows.length, todo: 0, completed: 0, problem: 0, idle: 0, matched: 0, confirm: 0, done: 0, failed: 0, skip: 0 };
    rows.forEach(function (r) {
      if (s[r.state] !== undefined) s[r.state] += 1;
    });
    s.todo = s.matched + s.confirm;
    s.completed = s.done;
    s.problem = s.failed;
    return s;
  }

  var presets = new URLSearchParams(location.search);
  var presetTheme = presets.get("theme");
  if (presetTheme === "dark" || presetTheme === "light") {
    document.documentElement.dataset.theme = presetTheme;
  }

  var settings = {
    general: { theme: presetTheme === "dark" ? "dark" : "light", first_run_done: true },
    library: {
      last_scan_path: "D:/CloudMusic",
      recent_paths: [
        "D:/CloudMusic",
        "E:/Game/GAI - REAL G (2026) ALAC",
        "D:/Music/无损收藏",
        "C:/Users/JIANGYIFAN/Music",
        "D:/临时测试"
      ]
    },
    lyrics: { save_target: "file", include_translation: true, overwrite_existing: false },
    write: { fill_missing_info: true, embed_cover: false }
  };

  /** 匹配失败时「找到过、但匹配度不够」的候选。真实后端会保留它们，
   *  好让用户看到「找的是什么、为什么不能用」；只是不给歌词预览。 */
  function rejectedFor(row) {
    var a = row.artist || "未知";
    return [
      { provider: "qq", providerName: "QQ音乐", songId: "901", title: row.title, artist: a, album: "同名专辑", year: 2024, duration: row.duration, score: 0.55 },
      { provider: "kugou", providerName: "酷狗", songId: "902", title: row.title.split(" ")[0], artist: "网络歌手", album: "翻唱合集", duration: row.duration - 17, score: 0.41 }
    ];
  }

  function candidatesFor(row) {
    if (!row.matched) return [];
    var base = row.matched;
    var out = [
      { provider: base.provider, providerName: base.providerName, songId: "1", title: base.title, artist: base.artist, album: base.album, year: base.year, duration: base.duration, score: base.score },
      { provider: "qq", providerName: "QQ音乐", songId: "2", title: base.title, artist: base.artist, album: base.album + " (Remastered)", year: base.year, duration: base.duration + 5, score: Math.max(0.55, base.score - 0.09) },
      { provider: "kugou", providerName: "酷狗", songId: "3", title: base.title + " (Live)", artist: base.artist, album: "演唱会现场", year: base.year, duration: base.duration + 26, score: Math.max(0.42, base.score - 0.24) },
      { provider: "kuwo", providerName: "酷我", songId: "4", title: base.title, artist: base.artist, album: "典藏合辑", duration: base.duration + 2, score: Math.max(0.38, base.score - 0.31) }
    ];
    return out;
  }

  function detailFor(row) {
    var cands = candidatesFor(row);
    // 失败的行：候选还在（被否掉的），但**没有**可预览的歌词——
    // 与后端 displayable_match 的口径保持一致
    if (row.state === "failed") cands = rejectedFor(row);
    return {
      id: row.id,
      title: row.title,
      artist: row.artist,
      album: row.matched ? row.matched.album : "",
      year: row.matched ? row.matched.year : undefined,
      trackNo: row.title === "夜曲" ? 3 : undefined,
      duration: row.duration,
      format: row.format,
      fileName: (row.artist ? row.artist + " - " : "") + row.title + "." + row.format.toLowerCase(),
      path: "D:/CloudMusic/" + row.title + "." + row.format.toLowerCase(),
      fileSize: row.duration * 45000,
      state: row.state,
      stateLabel: row.stateLabel,
      message: row.message,
      lyricsLabel: row.state === "done" ? "已保存到歌曲" : "尚未保存",
      hasCover: row.title !== "星夢",
      metaSource: "文件名",
      metaConfidence: 0.77,
      candidates: cands,
      pick: 0,
      // 预览的是「这首歌会保存成什么样」。失败 / 跳过没有可保存的结果，
      // 因此不给预览——给了就等于说「歌词已经就绪」。
      preview:
        row.matched && row.state !== "failed" && row.state !== "skip"
          ? { text: row._lrc, lines: row._lrc.split("\n").length, bytes: row._lrc.length, hasTranslation: false, hasVerbatim: false }
          : undefined,
      capabilities: {
        noYear: false,
        noTrackNo: !(cands[0] && cands[0].provider === "netease"),
        noVerbatim: !(cands[0] && cands[0].provider === "netease"),
        noTranslationVerified: false
      },
      provider: cands[0] ? cands[0].provider : undefined,
      needsReview: row.title === "贏" || row.title === "星夢"
    };
  }

  function snapshot() {
    return {
      library: { root: "D:/CloudMusic", tracks: rows.map(strip), stats: computeStats() },
      settings: settings,
      cacheBytes: 44 * 1024 * 1024,
      cacheEntries: 312,
      running: []
    };
  }

  function strip(r) {
    var o = {};
    for (var k in r) if (k.charAt(0) !== "_") o[k] = r[k];
    // 失败 / 跳过的行不向界面暴露匹配信息（与后端 displayable_match 同口径）：
    // 「找到过候选」不等于「匹配上了歌词」
    if (o.state === "skip" || o.state === "failed") o.matched = undefined;
    return o;
  }

  var callbacks = {};
  var nextCb = 1;

  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" } },
    callbacks: callbacks,
    transformCallback: function (cb) {
      var id = nextCb++;
      callbacks[id] = cb;
      return id;
    },
    invoke: function (cmd, args) {
      return new Promise(function (resolve, reject) {
        setTimeout(function () {
          switch (cmd) {
            case "snapshot":
              return resolve(snapshot());
            case "load_library":
              return resolve({ count: rows.length, restored: 0, downgraded: 0, path: "D:/CloudMusic", elapsedMs: 120, skipped: 0 });
            case "track_detail": {
              var row = rows.find(function (r) { return r.id === args.trackId; });
              return resolve(row ? detailFor(row) : null);
            }
            case "skip_track": {
              var rowSkip = rows.find(function (r) { return r.id === args.trackId; });
              if (!rowSkip) return reject("这首歌已不在曲库中");
              // 真的跳过：状态落到「跳过」，列表上不再暴露匹配信息，
              // 保存歌词也不会再带上它（与后端 skip_track 一致）
              rowSkip.state = "skip";
              rowSkip.stateLabel = "跳过";
              rowSkip.message = "已跳过，保存歌词时不会再带上它";
              return resolve(detailFor(rowSkip));
            }
            case "set_candidate_pick": {
              var row2 = rows.find(function (r) { return r.id === args.trackId; });
              if (!row2) return reject("这首歌已不在曲库中");
              var d = detailFor(row2);
              d.pick = args.pick;
              return resolve(d);
            }
            case "pick_candidate": {
              var row3 = rows.find(function (r) { return r.id === args.trackId; });
              if (!row3) return reject("这首歌已不在曲库中");
              row3.state = "matched";
              row3.stateLabel = "已匹配";
              row3.matched = {
                provider: args.candidate.provider,
                providerName: args.candidate.providerName,
                score: args.candidate.score || 0.96,
                title: args.candidate.title,
                artist: args.candidate.artist,
                album: args.candidate.album,
                year: args.candidate.year,
                duration: args.candidate.duration
              };
              row3.mismatch = false;
              var d2 = detailFor(row3);
              d2.pick = 0;
              return resolve(d2);
            }
            case "search_candidates":
              return resolve(candidatesFor(rows[0]).concat(candidatesFor(rows[1])));
            case "preview_candidate":
              return resolve({ text: lrcPool[0], lines: lrcPool[0].split("\n").length, bytes: lrcPool[0].length, hasTranslation: false, hasVerbatim: false });
            case "plan_write": {
              var ids = args.trackIds || [];
              // 演示用：3 首已有歌词、1 首正被占用、其余可写
              var existing = Math.min(3, ids.length);
              var locked = ids.length > 3 ? 1 : 0;
              return resolve({
                total: Math.max(0, ids.length - existing - locked),
                deltaBytes: ids.length * 3400,
                locked: locked,
                skipped: existing + locked,
                skippedExisting: existing,
                skippedUnsupported: 0,
                skippedNoMatch: 0,
                target: (args.options && args.options.lyrics.save_target) || "file"
              });
            }
            case "write_tracks":
            case "match_tracks":
              return resolve(1);
            case "save_settings":
              settings = args.settings;
              return resolve(settings);
            case "get_settings":
              return resolve(settings);
            case "recent_paths":
              return resolve(settings.library.recent_paths);
            case "mark_first_run_done":
              settings.general.first_run_done = true;
              return resolve(settings);
            case "cancel_task":
              return resolve(true);
            case "running_tasks":
              return resolve([]);
            case "webview_ready":
              return resolve();
            case "cache_usage":
              return resolve(44 * 1024 * 1024);
            case "clear_cache":
              return resolve(44 * 1024 * 1024);
            case "scan_library":
              return resolve({ count: rows.length, elapsedMs: 900, skipped: 2, path: args.path, restored: 0, downgraded: 0 });
            case "plugin:event|listen":
              return resolve(1);
            case "plugin:event|unlisten":
              return resolve();
            case "plugin:dialog|open":
              return resolve("D:/CloudMusic");
            case "plugin:window|is_maximized":
              return resolve(false);
            case "plugin:window|minimize":
            case "plugin:window|toggle_maximize":
            case "plugin:window|close":
            case "plugin:opener|reveal_item_in_dir":
              return resolve();
            default:
              console.warn("[preview] 未实现的命令：", cmd);
              return resolve(null);
          }
        }, 30);
      });
    }
  };

  // 供截图脚本手动触发的钩子
  window.__previewRows = rows;

  /*
   * 场景钩子：用 `?scene=xxx` 直接把界面推到某个状态，方便截图核对。
   * 通过模拟真实点击来驱动，因此走的完全是生产代码的路径。
   */
  // 计算样式探针：把关键元素的实际颜色写进 title，供 --dump-dom 读取
  if (new URLSearchParams(location.search).get("probe")) {
    setTimeout(function () {
      var pick = function (sel) {
        var el = document.querySelector(sel);
        if (!el) return sel + "=<missing>";
        var cs = getComputedStyle(el);
        return sel + " color=" + cs.color + " bg=" + cs.backgroundColor;
      };
      document.title = "PROBE theme=" + document.documentElement.dataset.theme + " | " +
        ["body", ".mini", ".btn"]
          .map(pick).join(" | ");
    }, 2200);
  }

  var scene = new URLSearchParams(location.search).get("scene");
  if (scene) {
    var pick = function (sel) {
      var el = document.querySelector(sel);
      if (el) el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      else console.warn("[preview] 场景元素不存在：", sel);
    };
    setTimeout(function () {
      switch (scene) {
        case "confirm":
          // 选中「晴天」（待确认），看候选列表与确认操作区
          pick('.trow[data-id="2"]');
          break;
        case "plan":
          pick("#btnWrite");
          break;
        case "manual":
          pick("#btnManual");
          break;
        case "settings":
          pick('.navtab[data-view="settings"]');
          break;
        case "log":
          pick("#btnLog");
          break;
        default:
          break;
      }
    }, 300);
  }
})();
