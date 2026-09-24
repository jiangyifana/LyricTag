//! LyricTag 命令行验证工具。
//!
//! 存在的意义：**在没有图形界面的情况下把整条链路跑通**——
//! 元信息四级降级链、四平台真实检索、评分、取词、写入与回读校验。
//! 它调用的是与 GUI 完全相同的那套 `pipeline` / `provider` / `tag` 代码，
//! 因此命令行里验证过的行为，界面上就是同样的行为。
//!
//! ```text
//! lyrictag-cli scan   <目录>                  # 扫描并打印元信息提取结果
//! lyrictag-cli inspect <文件>                 # 看单个文件的标签与可写性
//! lyrictag-cli match  <目录> [--limit N]      # 真实检索四个平台并打印评分
//! lyrictag-cli write  <目录> [--sidecar] [--cover]   # 取词并写入（含回读校验）
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use lyrictag_lib::domain::candidate::SearchQuery;
use lyrictag_lib::domain::track::{LyricsPresence, Track, TrackState};
use lyrictag_lib::infra::config::{SaveTarget, Settings};
use lyrictag_lib::infra::http;
use lyrictag_lib::pipeline::{downloader, matcher, scanner, writer, ProviderGate};
use lyrictag_lib::provider::ProviderRegistry;
use lyrictag_lib::tag;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print_usage();
        return ExitCode::from(2);
    };
    let rest = &args[1..];

    let result = match cmd.as_str() {
        "scan" => cmd_scan(rest),
        "inspect" => cmd_inspect(rest),
        "match" => cmd_match(rest),
        "write" => cmd_write(rest),
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("未知命令：{other}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("失败：{e}");
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    println!(
        "LyricTag 命令行验证工具\n\n\
         用法：\n  \
         lyrictag-cli scan    <目录>                  扫描并打印元信息提取结果\n  \
         lyrictag-cli inspect <文件>                  查看单个文件的标签与格式可写性\n  \
         lyrictag-cli match   <目录> [--limit N]      真实检索四个平台并打印评分\n  \
         lyrictag-cli write   <目录> [--sidecar] [--cover]\n                       \
         取词并写入（默认写进文件，含写后回读校验）\n"
    );
}

// ── scan ─────────────────────────────────────────────────────────────────

fn cmd_scan(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("缺少目录参数")?;
    let root = PathBuf::from(dir);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let started = std::time::Instant::now();
    let outcome = scanner::scan(&root, cancel, |_, _| {}).map_err(|e| e.user_message())?;
    let elapsed = started.elapsed();

    println!("扫描目录：{dir}");
    println!(
        "共 {} 首歌，耗时 {:.2} 秒{}",
        outcome.tracks.len(),
        elapsed.as_secs_f64(),
        if outcome.skipped > 0 {
            format!("，跳过 {} 个文件", outcome.skipped)
        } else {
            String::new()
        }
    );
    println!();

    for t in &outcome.tracks {
        print_track_meta(t);
    }
    Ok(())
}

fn print_track_meta(t: &Track) {
    println!("── {} ──", t.path.file_name().unwrap_or_default().to_string_lossy());
    println!(
        "   标题     {:?}",
        t.meta.title.as_deref().unwrap_or("<空>")
    );
    println!(
        "   艺术家   {:?}",
        t.meta.artist.as_deref().unwrap_or("<空>")
    );
    println!(
        "   专辑     {:?}",
        t.meta.album.as_deref().unwrap_or("<空>")
    );
    println!("   音轨号   {:?}", t.meta.track_no);
    println!("   年份     {:?}", t.meta.year);
    println!(
        "   时长     {}",
        t.duration_ms
            .map(|ms| format!("{}.{:02}", ms / 1000, (ms % 1000) / 10))
            .unwrap_or_else(|| "未知".into())
    );
    println!("   格式     {}", t.format.label());
    println!("   有封面   {}", t.meta.has_cover);
    println!("   元信息来源 {:?}", t.meta.source);
    println!("   可信度   {:.2}", t.meta_confidence);
    println!("   已有歌词 {:?}", t.existing_lyrics);
    // 归一化之后的检索词——繁体标题会在这里变成简体
    println!("   检索词   {:?}", SearchQuery::from_track(t).keyword());
    println!();
}

// ── inspect ──────────────────────────────────────────────────────────────

fn cmd_inspect(args: &[String]) -> Result<(), String> {
    let file = args.first().ok_or("缺少文件参数")?;
    let path = Path::new(file);

    let probed = tag::probe::probe(path).map_err(|e| e.user_message())?;
    let cap = tag::supported::capability(path).map_err(|e| e.user_message())?;

    println!("文件：{}", path.display());
    println!("  格式           {}", cap.format.label());
    println!("  主标签类型     {:?}", cap.tag_type);
    println!("  标签可写       {}", cap.writable);
    println!("  时长           {:?} ms", probed.duration_ms);
    println!("  标签内标题     {:?}", probed.meta.title);
    println!("  标签内艺术家   {:?}", probed.meta.artist);
    println!("  标签内专辑     {:?}", probed.meta.album);
    println!("  有封面         {}", probed.meta.has_cover);
    println!("  标签内已有歌词 {}", probed.has_embedded_lyrics);
    println!(
        "  同目录 .lrc    {}",
        tag::writer_lofty::sidecar_path_for(path).is_file()
    );
    println!("  文件被占用     {}", tag::lock_check::is_locked(path));
    Ok(())
}

// ── match ────────────────────────────────────────────────────────────────

fn cmd_match(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("缺少目录参数")?;
    let limit = flag_value(args, "--limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(usize::MAX);

    let tracks = scan_dir(dir)?;
    let registry = ProviderRegistry::new(http::build_client());
    let gate = ProviderGate::default();

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;

    runtime.block_on(async {
        // ① 静默检查源可用性（与界面一致，§4.5.2）
        let (up, down) = lyrictag_lib::pipeline::orchestrator::check_sources(&registry, &gate).await;
        println!(
            "歌词源可用性：{} 个可用{}",
            up.len(),
            if down.is_empty() {
                String::new()
            } else {
                format!("，不可用：{}", down.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、"))
            }
        );
        println!();

        let mut matched = 0;
        let mut confirm = 0;
        let mut failed = 0;

        for t in tracks.iter().take(limit) {
            let query = SearchQuery::from_track(t);
            println!("══ {} ══", t.path.file_name().unwrap_or_default().to_string_lossy());
            println!(
                "   本地：{:?} / {:?} / {}",
                query.title,
                query.artist,
                t.duration_ms.map(|d| format!("{}s", d / 1000)).unwrap_or_default()
            );

            let outcome = matcher::search_all(&registry, &gate, &query).await;
            if outcome.failed.is_empty() && outcome.candidates.is_empty() {
                println!("   （没有任何候选）");
            }
            for (id, reason) in &outcome.failed {
                println!("   [{id}] 检索失败：{reason}");
            }

            // 用与界面完全相同的候选裁剪逻辑，命令行看到的顺序就是用户看到的顺序
            let shown = matcher::shortlist(&outcome.candidates);
            for (i, c) in shown.iter().enumerate() {
                println!(
                    "   {}. {:.3}  {:<10} {} · {} · {}s{}",
                    i + 1,
                    c.score.total,
                    c.provider.display_name(),
                    c.title,
                    c.artist_joined(),
                    c.duration_ms.map(|d| d / 1000).unwrap_or(0),
                    match c.year {
                        Some(y) => format!(" · {y}"),
                        None => String::new(),
                    }
                );
            }

            match outcome.confidence() {
                Some(c) => {
                    let label = match c {
                        lyrictag_lib::domain::candidate::Confidence::Auto(_) => {
                            matched += 1;
                            "已匹配（自动采纳）"
                        }
                        lyrictag_lib::domain::candidate::Confidence::Confirm(_) => {
                            confirm += 1;
                            "待确认"
                        }
                        lyrictag_lib::domain::candidate::Confidence::Rejected(_) => {
                            failed += 1;
                            "未找到"
                        }
                    };
                    println!("   → {label}（{:.3}）", c.value());
                }
                None => {
                    failed += 1;
                    println!("   → 未找到");
                }
            }
            println!();
        }
        println!("小计：已匹配 {matched} · 待确认 {confirm} · 未找到 {failed}");
    });
    Ok(())
}

// ── write ────────────────────────────────────────────────────────────────

fn cmd_write(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("缺少目录参数")?;
    let sidecar = args.iter().any(|a| a == "--sidecar");
    let cover = args.iter().any(|a| a == "--cover");
    let limit = flag_value(args, "--limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(usize::MAX);

    let mut settings = Settings::default();
    settings.lyrics.save_target = if sidecar { SaveTarget::Sidecar } else { SaveTarget::File };
    settings.write.embed_cover = cover;
    // 命令行是验证工具，重复运行时允许覆盖，否则第二次啥也写不了
    settings.lyrics.overwrite_existing = true;

    let tracks = scan_dir(dir)?;
    // 封面下载复用同一个客户端：每首歌新建一个会重复做 TLS 初始化
    let client = http::build_client();
    let registry = ProviderRegistry::new(client.clone());
    let gate = ProviderGate::default();

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;

    runtime.block_on(async {
        let mut ok = 0usize;
        let mut failed = 0usize;
        let mut bytes_total = 0i64;

        for t in tracks.iter().take(limit) {
            let query = SearchQuery::from_track(t);
            println!("══ {} ══", t.path.file_name().unwrap_or_default().to_string_lossy());

            // ① 匹配
            let outcome = matcher::search_all(&registry, &gate, &query).await;
            let Some(mut best) = outcome.candidates.first().cloned() else {
                println!("   没有候选，跳过\n");
                failed += 1;
                continue;
            };
            if best.score.total < lyrictag_lib::domain::score::NEED_REVIEW {
                println!("   最佳候选评分 {:.3} 过低，跳过\n", best.score.total);
                failed += 1;
                continue;
            }
            println!(
                "   选中：{} · {} · {:.3}",
                best.provider.display_name(),
                best.title,
                best.score.total
            );

            // ② 补齐富字段 + 取词
            downloader::enrich(&registry, &mut best).await;
            let lyrics = match downloader::fetch_lyrics(&registry, &gate, &best).await {
                Ok(l) => l,
                Err(e) => {
                    println!("   取词失败：{}\n", e.user_message());
                    failed += 1;
                    continue;
                }
            };
            if lyrics.is_instrumental {
                println!("   纯音乐，无需歌词\n");
                continue;
            }
            if !lyrics.has_content() {
                println!("   这个来源没有可用歌词\n");
                failed += 1;
                continue;
            }
            println!(
                "   歌词 {} 行 · {} 字节{}",
                lyrics.content_line_count(),
                lyrics.estimated_bytes(),
                if lyrics.has_translation() { " · 含翻译" } else { "" }
            );

            // ③ 组装载荷（与界面共用同一条代码路径）
            let mut track = t.clone();
            track.state = TrackState::Matched;
            let confidence = lyrictag_lib::domain::score::decide(best.score.total);
            track.matched = Some(lyrictag_lib::domain::plan::MatchResult {
                metadata: best.to_meta(),
                cover_url: best.cover_url.clone(),
                candidate: best.clone(),
                lyrics,
                confidence,
            });

            let cover_bytes = if cover && !track.meta.has_cover {
                match &track.matched.as_ref().unwrap().cover_url {
                    Some(url) => match downloader::fetch_cover(&client, url).await {
                        Ok(c) => {
                            println!("   封面 {} 字节（{}）", c.bytes.len(), c.mime);
                            Some(c)
                        }
                        Err(e) => {
                            println!("   封面下载失败，只写歌词：{}", e.user_message());
                            None
                        }
                    },
                    None => None,
                }
            } else {
                None
            };

            // ④ 写入（内部含格式预检与写后回读校验）
            match writer::execute(&track, settings.lyrics.save_target, &settings, cover_bytes) {
                Ok(outcome) => {
                    ok += 1;
                    bytes_total += outcome.bytes_delta;
                    match &outcome.target {
                        lyrictag_lib::domain::plan::WrittenTarget::Sidecar(p) => println!(
                            "   ✓ 已生成 {}",
                            p.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        lyrictag_lib::domain::plan::WrittenTarget::EmbeddedTag(key) => println!(
                            "   ✓ 已写入歌曲文件（{}），体积变化 {:+} 字节，回读校验通过",
                            key, outcome.bytes_delta
                        ),
                    }
                }
                Err(e) => {
                    failed += 1;
                    println!("   ✗ 写入失败：{}", e.user_message());
                }
            }
            println!();
        }

        println!("结果：成功 {ok} 首，失败 {failed} 首，总体积变化 {bytes_total:+} 字节");

        // 结束后核对一次：真的读得回来吗
        println!("\n回读核对：");
        for t in tracks.iter().take(limit) {
            let embedded = tag::probe::probe(&t.path)
                .map(|p| p.has_embedded_lyrics)
                .unwrap_or(false);
            let sidecar = tag::writer_lofty::sidecar_path_for(&t.path);
            let sidecar_ok = sidecar.is_file();
            let presence = match (sidecar_ok, embedded) {
                (true, true) => LyricsPresence::Both,
                (true, false) => LyricsPresence::SidecarLrc,
                (false, true) => LyricsPresence::EmbeddedTag,
                (false, false) => LyricsPresence::None,
            };
            println!(
                "   {:<44} {:?}",
                t.path.file_name().unwrap_or_default().to_string_lossy(),
                presence
            );
        }
    });

    Ok(())
}

// ── 公共 ─────────────────────────────────────────────────────────────────

fn scan_dir(dir: &str) -> Result<Vec<Track>, String> {
    let root = PathBuf::from(dir);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let outcome = scanner::scan(&root, cancel, |_, _| {}).map_err(|e| e.user_message())?;
    if outcome.tracks.is_empty() {
        return Err(format!("{dir} 里没有找到音乐文件"));
    }
    Ok(outcome.tracks)
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1)
}
