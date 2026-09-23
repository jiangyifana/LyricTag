//! 文件占用检测（§4.4.4）。
//!
//! **「文件被占用」比「写坏」更常见**——它才是批量任务里最主要的失败来源。
//! 写入前用它探测，失败则列入「稍后重试」，而不是报错中断整批任务。

use std::path::Path;

use crate::infra::error::{AppError, Result};

/// Windows 的 ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
#[cfg(windows)]
const SHARING_VIOLATION: i32 = 32;
#[cfg(windows)]
const LOCK_VIOLATION: i32 = 33;

/// 试探性打开文件，问的是**「我能不能拿到写权限」**。
///
/// 访问方式必须与上层标签写入所用的完全一致（`lofty` 就是
/// `OpenOptions::new().read(true).write(true)`）：这样「探测通过」才等价于
/// 「写得进去」。
///
/// **不要用 `share_mode(0)`。** 那是「独占」语义——只要有任何进程以共享方式
/// 打开过这个文件（资源管理器预览、杀毒、云同步、系统索引都会这么干），
/// 独占打开就以 SHARING_VIOLATION 失败，于是把一个**完全可写**的文件报成
/// 「正被其他程序使用」。实测踩坑：曲库 16 首里有 1 首被这样误判，而
/// 同一时刻以「读写 + 允许共享」打开它是成功的。
pub fn ensure_not_locked(path: &Path) -> Result<()> {
    match std::fs::OpenOptions::new().read(true).write(true).open(path) {
        Ok(_) => Ok(()),
        Err(e) => {
            if denies_writing(&e) {
                Err(AppError::FileLocked { path: path.to_path_buf() })
            } else {
                // 文件不存在、路径非法之类——不是「稍后重试」能解决的问题
                Err(AppError::Io(e))
            }
        }
    }
}

/// 这个 IO 错误是否等于「写不进去」。
fn denies_writing(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        // 只读属性或权限不足——同样写不进去，按「被占用」处理。
        // 界面文案统一为「这个文件正被其他程序使用，已跳过」，不暴露系统错误码。
        return true;
    }
    #[cfg(windows)]
    {
        // 别人以「拒绝写入者」的方式持有它——播放器就是这么干的
        let raw = e.raw_os_error().unwrap_or(0);
        raw == SHARING_VIOLATION || raw == LOCK_VIOLATION
    }
    #[cfg(not(windows))]
    false
}

/// 只做判断、不产生错误对象的便捷版本（写入计划预演时用）
pub fn is_locked(path: &Path) -> bool {
    ensure_not_locked(path).is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, b"x").unwrap();
        p
    }

    #[test]
    fn existing_unlocked_file_passes() {
        let p = temp_file("lyrictag_lock_probe.txt");
        assert!(ensure_not_locked(&p).is_ok());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_file_reports_error() {
        let p = std::env::temp_dir().join("lyrictag_definitely_missing.bin");
        let _ = std::fs::remove_file(&p);
        assert!(ensure_not_locked(&p).is_err());
    }

    /// 回归：别的进程**只是打开着**这个文件（共享持有），不该判成被占用。
    ///
    /// 旧实现用 `share_mode(0)` 独占探测，把这类文件统统报成「正被其他程序使用」，
    /// 结果是用户看到「另有 1 首不会被保存」，而那首歌其实完全可以写入。
    #[test]
    fn permissive_holder_does_not_count_as_locked() {
        let p = temp_file("lyrictag_lock_probe_shared.txt");
        let holder = std::fs::OpenOptions::new().read(true).open(&p).unwrap();
        assert!(
            ensure_not_locked(&p).is_ok(),
            "共享持有者不该让文件被判定为占用"
        );
        drop(holder);
        let _ = std::fs::remove_file(&p);
    }

    /// 反过来必须仍然守得住：以「只允许读」持有（拒绝写入者）时，探测必须失败。
    /// 典型场景就是播放器正在播这首歌。
    #[cfg(windows)]
    #[test]
    fn holder_that_denies_writing_is_detected() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;

        let p = temp_file("lyrictag_lock_probe_denied.txt");
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&p)
            .unwrap();
        assert!(
            ensure_not_locked(&p).is_err(),
            "拒绝写入的持有者必须被判为占用，否则保存会在写到一半时失败"
        );
        drop(holder);
        let _ = std::fs::remove_file(&p);
    }
}
