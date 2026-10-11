//! 单例运行：同一份数据目录只允许一个 canto。
//!
//! 用文件锁（Unix `flock`、Windows `LockFileEx`，都由标准库的 `File::try_lock`
//! 落到系统调用）锁住 `<配置目录>/canto/canto.lock`：锁跟着**进程**走，进程一死
//! 内核自动释放，所以不需要处理「上次没退干净留下的僵尸锁」——锁文件留着没关系，
//! 有它在只说明「文件存在过」，不代表有人在跑。
//!
//! 为什么要单例：会话文件、历史、日志都是同一批文件，两个实例同时写会互相覆盖
//! （尤其 `session.json` 与 `history.json` 都是「读-改-写」）。

use std::fs::{File, TryLockError};
use std::io::{Read, Seek, Write};
use std::path::Path;

use anyhow::{Context, Result};

/// 持有它就代表持有锁；`Drop`（进程退出）时文件描述符关闭，锁自动释放。
pub struct Instance {
    _file: File,
}

impl Instance {
    /// 尝试独占。`Ok(None)` = 已经有一个 canto 在跑。
    pub fn acquire(path: &Path) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
        }
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("打开锁文件失败：{}", path.display()))?;
        // 标准库的 `try_lock`：Unix 上等于 `flock(LOCK_EX | LOCK_NB)`，Windows 上是
        // `LockFileEx`；锁跟着这个 `File` 走，句柄一关（进程退出也算）自动释放。
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("锁 {} 失败", path.display()));
            }
        }
        // 锁拿到手了：把 pid 写进去，第二个实例能报出「谁在跑」。
        // 只有 Unix 读得到：Windows 那边锁是强制的，别人正持锁时读不了文件内容（见 `pid_in`）。
        let _ = file.set_len(0);
        let _ = file.rewind();
        let _ = write!(file, "{}", std::process::id());
        Ok(Some(Self { _file: file }))
    }
}

/// 读锁文件里的 pid（只为把错误信息说清楚，读不到就当没有）。
///
/// Windows 上「别人正持锁」时这里必然读不到：`LockFileEx` 的字节范围锁是**强制**的，
/// 另一个句柄读被锁住的字节会直接失败（实测 `ERROR_LOCK_VIOLATION`）；Unix 的 `flock`
/// 只是劝告式锁，不影响读。所以那边第二个实例只会说「已经有一个在跑」，报不出 pid。
pub fn pid_in(path: &Path) -> Option<u32> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("canto-lock-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("canto.lock")
    }

    #[test]
    fn the_second_holder_is_refused() {
        let path = temp_path("second");
        let first = Instance::acquire(&path).expect("第一次该成功");
        assert!(first.is_some(), "没人占着，第一次必须拿到锁");
        assert!(
            Instance::acquire(&path).expect("第二次调用本身不该报错").is_none(),
            "已经有一个在跑，第二次必须被拒"
        );
        // 正持锁时只有 Unix 读得到 pid：Windows 的锁是强制的，另一个句柄读不了被锁住的字节。
        if cfg!(unix) {
            assert_eq!(pid_in(&path), Some(std::process::id()), "锁文件里该写着持有者的 pid");
        }

        // 第一个实例退出（drop）后，锁就该能再拿到——不靠删文件自愈。
        drop(first);
        assert_eq!(pid_in(&path), Some(std::process::id()), "锁一松，pid 就该读得到");
        assert!(
            Instance::acquire(&path).expect("释放后该能拿").is_some(),
            "上一个实例退出后锁要能重新拿到"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
