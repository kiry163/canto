//! 桌面歌词：取词、状态机与窗口位置（平台无关的那一半）。
//!
//! 平台相关的部分在下面 `cfg` 子模块里：Windows 是「专用线程 + 分层窗口 + DIB 绘制」，
//! 其它平台是空实现。状态机是两半共用的——将来写 macOS 那份只需要补窗口与画字，
//! `Frame` / `Payload` / 位置状态一行都不用动。

use std::path::Path;
use std::sync::mpsc::Receiver;

use anyhow::{Context, Result};

use crate::config::DesktopLyricsConfig;
use crate::model::Lyric;
use crate::player::{Phase, Snapshot};

/// 夹取窗口位置时留的边距：中心点不会贴到屏幕边上。
pub const MARGIN: i32 = 40;

/// 解锁但没有歌词可比时显示的那句话——不然解锁了也找不到窗口，没法拖。
pub const UNLOCKED_HINT: &str = "拖动我（再按一次 ctrl+shift+l 锁定）";

/// 一帧的外部输入（都是借来的，不复制）。
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub lyric: Option<&'a Lyric>,
    /// 引擎快照：`phase` 与 `position` 都在里面。
    pub snapshot: &'a Snapshot,
}

/// 覆盖层要显示的东西；`PartialEq` 就是「要不要重画」的判据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    /// `false` = 该隐藏（窗口不销毁，只是 `SW_HIDE`，位置与锁定态留着）。
    pub visible: bool,
    /// 当前句（可能是空串：歌词里的纯音乐段）。
    pub current: String,
    /// 下一句（最后一句之后是空串）。
    pub next: String,
    /// 当前句的译文（配置关掉或这首没有就是 `None`）。
    pub translation: Option<String>,
    /// 锁定态（决定鼠标穿透与那圈提示框）。
    pub locked: bool,
}

/// 状态机：记住上次推出去的 payload 与锁定态。
struct State {
    last: Option<Payload>,
    locked: bool,
}

impl Default for State {
    fn default() -> Self {
        // 启动一律锁定：锁定态刻意不持久化，省得下次开机「怎么点不到」。
        Self { last: None, locked: true }
    }
}

impl State {
    fn toggle_lock(&mut self) {
        self.locked = !self.locked;
    }

    /// 算「理想 payload」，跟上次推出去的比：不同才返回（相同就什么都不做）。
    fn advance(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig) -> Option<Payload> {
        // 取流中沿用上一次的样子，别闪。
        if matches!(frame.snapshot.phase, Phase::Buffering) {
            return None;
        }

        let (mut current, mut next, mut translation) = lines(frame, config);
        let has_something = !current.is_empty() || !next.is_empty() || translation.is_some();

        let playing = match frame.snapshot.phase {
            Phase::Playing => true,
            Phase::Paused => !config.hide_when_paused,
            // `Idle` / `Failed`（`Buffering` 在上面已经返回）
            _ => false,
        };
        let mut visible = playing && has_something;
        if !visible {
            // 隐藏时不带文案：免得下次显示时先闪一下旧句。
            current.clear();
            next.clear();
            translation = None;
        }

        // 解锁时强制可见：否则解锁了却找不到窗口，没法拖。
        if !self.locked {
            visible = true;
            if current.is_empty() && next.is_empty() && translation.is_none() {
                current = UNLOCKED_HINT.to_string();
            }
        }

        let payload = Payload { visible, current, next, translation, locked: self.locked };
        if self.last.as_ref() == Some(&payload) {
            return None;
        }
        self.last = Some(payload.clone());
        Some(payload)
    }
}

/// 当前句、下一句（过界就是空串）、当前句的译文（配置关掉就是 `None`）。
fn lines(frame: Frame<'_>, config: &DesktopLyricsConfig) -> (String, String, Option<String>) {
    let Some(lyric) = frame.lyric else {
        return (String::new(), String::new(), None);
    };
    let index = lyric.line_at(frame.snapshot.position);
    let line = index.and_then(|index| lyric.lines.get(index));
    let current = line.map(|line| line.text.clone()).unwrap_or_default();
    let next = index
        .and_then(|index| lyric.lines.get(index + 1))
        .map(|line| line.text.clone())
        .unwrap_or_default();
    let translation = config
        .show_translation
        .then(|| line.and_then(|line| line.translation.clone()))
        .flatten();
    (current, next, translation)
}

/// 默认位置：主屏底部居中。
fn default_center(screen: (i32, i32)) -> (i32, i32) {
    (screen.0 / 2, screen.1 * 85 / 100)
}

/// 把中心点夹进主屏内（留 `MARGIN` 边距）；屏幕特别小时也不会夹出反区间。
fn clamp_center(center: (i32, i32), screen: (i32, i32)) -> (i32, i32) {
    (
        center.0.clamp(MARGIN, (screen.0 - MARGIN).max(MARGIN)),
        center.1.clamp(MARGIN, (screen.1 - MARGIN).max(MARGIN)),
    )
}

/// 主屏尺寸（宽、高）。
#[cfg(target_os = "windows")]
fn primary_screen() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: 只读两个系统指标，没有所有权问题。
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

#[cfg(not(target_os = "windows"))]
fn primary_screen() -> (i32, i32) {
    // 空实现那份用不到；给个确定的数，好让测试与编译都稳。
    (1920, 1080)
}

/// 位置状态文件的内容。
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Center {
    cx: i32,
    cy: i32,
}

/// 读窗口中心点；文件不在或读坏了就是 `None`（调用方退回默认位置）。
pub fn load_center(path: &Path) -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(path).ok()?;
    let center: Center = serde_json::from_str(&text).ok()?;
    Some((center.cx, center.cy))
}

/// 写窗口中心点（临时文件 + rename，中途失败不留坏文件——跟 `history.rs` 一个套路）。
pub fn save_center(path: &Path, center: (i32, i32)) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
    let text = serde_json::to_string(&Center { cx: center.0, cy: center.1 })?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).with_context(|| format!("写 {} 失败", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("替换 {} 失败", path.display()))
}

/// 桌面歌词的句柄。
pub struct DesktopLyrics {
    state: State,
    imp: imp::Imp,
}

/// 装出来的东西。
pub struct Installed {
    pub overlay: DesktopLyrics,
    /// 拖动结束时窗口线程发过来的新中心点（非 Windows、或关掉功能时是 `None`）。
    pub positions: Option<Receiver<(i32, i32)>>,
    /// `Some` = 起不来（调用方报一句 + 日志），此时 `overlay` 是空句柄。
    pub error: Option<String>,
}

impl DesktopLyrics {
    /// 空句柄：关掉功能、非 Windows、`App::new` 与测试都用它；`sync`/`toggle_lock` 是空操作。
    pub fn none() -> Self {
        Self { state: State::default(), imp: imp::Imp::none() }
    }

    /// 按配置与已存的中心点起覆盖层。
    pub fn install(config: &DesktopLyricsConfig, saved: Option<(i32, i32)>) -> Installed {
        if !config.enabled {
            return Installed { overlay: Self::none(), positions: None, error: None };
        }
        // 存过中心点就用它（顺手夹回主屏内）；没有就用默认的底部居中。
        let screen = primary_screen();
        let center =
            saved.map_or_else(|| default_center(screen), |center| clamp_center(center, screen));
        let (imp, positions, error) = imp::install(config, center);
        Installed { overlay: Self { state: State::default(), imp }, positions, error }
    }

    /// 每帧调一次；只有变化才会真的推给窗口。
    pub fn sync(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig) {
        if let Some(payload) = self.state.advance(frame, config) {
            self.imp.push(payload);
        }
    }

    /// 切锁定/解锁（下一帧 `sync` 自然会把新锁定态推出去）。
    pub fn toggle_lock(&mut self) {
        self.state.toggle_lock();
    }

    /// 现在是解锁的吗（接线与测试用）。
    pub fn locked(&self) -> bool {
        self.state.locked
    }
}

/// Windows：真实现见 Task 3（专用线程 + 分层窗口 + DIB 绘制）。
#[cfg(target_os = "windows")]
mod imp {
    use std::sync::mpsc::Receiver;

    use super::{DesktopLyricsConfig, Payload};

    pub(super) struct Imp;

    impl Imp {
        pub(super) fn none() -> Self {
            Self
        }

        pub(super) fn push(&mut self, _payload: Payload) {}
    }

    /// 平台实现的回报：句柄、位置通道（拖完回传中心点）、失败原因。
    pub(super) type Started = (Imp, Option<Receiver<(i32, i32)>>, Option<String>);

    pub(super) fn install(_config: &DesktopLyricsConfig, _center: (i32, i32)) -> Started {
        (Imp::none(), None, None)
    }
}

/// 其它平台（含 macOS，等以后单独做）：不建窗口，什么都不做。这份之后不再改。
#[cfg(not(target_os = "windows"))]
mod imp {
    use std::sync::mpsc::Receiver;

    use super::{DesktopLyricsConfig, Payload};

    pub(super) struct Imp;

    impl Imp {
        pub(super) fn none() -> Self {
            Self
        }

        pub(super) fn push(&mut self, _payload: Payload) {}
    }

    /// 平台实现的回报：句柄、位置通道（拖完回传中心点）、失败原因。
    pub(super) type Started = (Imp, Option<Receiver<(i32, i32)>>, Option<String>);

    pub(super) fn install(_config: &DesktopLyricsConfig, _center: (i32, i32)) -> Started {
        (Imp::none(), None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DesktopLyricsConfig;
    use crate::model::{Lyric, LyricLine};
    use crate::player::{Phase, Snapshot};
    use std::time::Duration;

    /// 每句隔 10 秒——`line_at(position)` 就是按这个时间找当前句的。
    fn lyric(lines: &[(&str, Option<&str>)]) -> Lyric {
        Lyric {
            lines: lines
                .iter()
                .enumerate()
                .map(|(index, (text, translation))| LyricLine {
                    at: Duration::from_secs(index as u64 * 10),
                    text: text.to_string(),
                    translation: translation.map(str::to_string),
                })
                .collect(),
        }
    }

    fn snapshot_at(phase: Phase, secs: u64) -> Snapshot {
        Snapshot {
            title: None,
            mid: None,
            quality: None,
            size: None,
            phase,
            position: Duration::from_secs(secs),
            duration: Some(Duration::from_secs(600)),
            volume: 100,
            just_finished: false,
        }
    }

    /// 同一帧第二次不该再推——这就是「没有逐帧开销」的判据。
    #[test]
    fn the_same_frame_twice_pushes_once() {
        let lines = lyric(&[("第一句", None), ("第二句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 15);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_some(), "第一次该推");
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_none(), "同一帧第二次不该再推");
    }

    /// 同一句里推进位置（进度条在动）不算变化。
    #[test]
    fn position_within_the_same_line_does_not_repush() {
        let lines = lyric(&[("第一句", None), ("第二句", None)]);
        let mut state = State::default();

        let early = snapshot_at(Phase::Playing, 10);
        let frame = Frame { lyric: Some(&lines), snapshot: &early };
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_some());

        let later = snapshot_at(Phase::Playing, 19);
        let frame = Frame { lyric: Some(&lines), snapshot: &later };
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).is_none(), "还在第二句里");
    }

    /// 换行时当前句与下一句都要对（下一句过界就是空串）。
    #[test]
    fn a_new_line_pushes_current_and_next() {
        let lines = lyric(&[("第一句", None), ("第二句", None), ("第三句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 25);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        let payload = state.advance(frame, &DesktopLyricsConfig::default()).expect("换行该推");
        assert_eq!(payload.current, "第三句");
        assert_eq!(payload.next, "", "最后一句之后没有下一句");
        assert!(payload.visible);
    }

    /// 纯音乐段（当前行是空文本）只显示下一句，不留空框。
    #[test]
    fn an_empty_line_shows_only_the_next_one() {
        let lines = lyric(&[("", None), ("下一句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        let payload = state.advance(frame, &DesktopLyricsConfig::default()).expect("该推");
        assert_eq!(payload.current, "", "这一句本来就是空的");
        assert_eq!(payload.next, "下一句");
        assert!(payload.visible, "有下一句可显示，窗口要留着");
    }

    /// 没在播就整窗隐藏——不显示上一句的残影（隐藏时连文案都不带）。
    #[test]
    fn nothing_playing_hides_the_overlay() {
        let lines = lyric(&[("第一句", None)]);
        let config = DesktopLyricsConfig::default();

        // 播放中 → 取流全挂：真实场景，要推一次「隐藏」。
        let mut state = State::default();
        let playing = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &playing };
        assert!(state.advance(frame, &config).expect("该推").visible);

        let failed = snapshot_at(Phase::Failed("全挂了".to_string()), 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &failed };
        assert!(!state.advance(frame, &config).expect("该推").visible, "取流全挂要收起来");

        // 空转（一直没在播）：隐藏态之间没有变化，不该反复推。
        let mut state = State::default();
        let idle = snapshot_at(Phase::Idle, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &idle };
        let payload = state.advance(frame, &config).expect("第一次要推一次隐藏");
        assert!(!payload.visible);
        assert!(payload.current.is_empty(), "隐藏时不留文案，免得下次显示先闪旧句");
        assert!(state.advance(frame, &config).is_none(), "还是隐藏，不用再推");
    }

    /// 暂停默认留着（想隐藏就配 `hide_when_paused`）。
    #[test]
    fn pausing_keeps_the_line_unless_configured() {
        let lines = lyric(&[("第一句", None)]);
        let snapshot = snapshot_at(Phase::Paused, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        assert!(state.advance(frame, &DesktopLyricsConfig::default()).expect("该推").visible);

        let mut state = State::default();
        let config = DesktopLyricsConfig { hide_when_paused: true, ..DesktopLyricsConfig::default() };
        assert!(!state.advance(frame, &config).expect("该推").visible);
    }

    /// 解锁且没歌词时强制可见并给提示语——否则解锁了却找不到窗口，没法拖。
    #[test]
    fn unlocking_forces_the_overlay_visible() {
        let mut state = State::default();
        state.toggle_lock();

        let snapshot = snapshot_at(Phase::Idle, 0);
        let frame = Frame { lyric: None, snapshot: &snapshot };
        let payload = state
            .advance(frame, &DesktopLyricsConfig::default())
            .expect("锁定态变了就该推");
        assert!(payload.visible);
        assert!(!payload.locked);
        assert_eq!(payload.current, UNLOCKED_HINT);
    }

    /// 译文跟着开关走。
    #[test]
    fn the_translation_follows_the_switch() {
        let lines = lyric(&[("第一句", Some("first"))]);
        let snapshot = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut state = State::default();
        let payload = state.advance(frame, &DesktopLyricsConfig::default()).expect("该推");
        assert_eq!(payload.translation.as_deref(), Some("first"));

        let mut state = State::default();
        let config = DesktopLyricsConfig { show_translation: false, ..DesktopLyricsConfig::default() };
        let payload = state.advance(frame, &config).expect("该推（配置不同→上次推的不一样）");
        assert_eq!(payload.translation, None);
    }

    /// 空句柄（关掉功能 / 非 Windows / 测试）：sync 与 toggle_lock 都是空操作。
    #[test]
    fn a_none_handle_syncs_nothing() {
        let lines = lyric(&[("第一句", None)]);
        let snapshot = snapshot_at(Phase::Playing, 5);
        let frame = Frame { lyric: Some(&lines), snapshot: &snapshot };

        let mut overlay = DesktopLyrics::none();
        overlay.sync(frame, &DesktopLyricsConfig::default());
        overlay.toggle_lock();
        assert!(!overlay.locked(), "空句柄不记得锁定态");
    }

    /// 换过显示器/分辨率后，屏外的中心点要夹回主屏内（留边距）；默认位是底部居中。
    #[test]
    fn a_saved_center_is_clamped_onto_the_primary_screen() {
        assert_eq!(clamp_center((-5000, -5000), (1920, 1080)), (MARGIN, MARGIN));
        assert_eq!(clamp_center((99999, 99999), (1920, 1080)), (1920 - MARGIN, 1080 - MARGIN));
        assert_eq!(clamp_center((800, 500), (1920, 1080)), (800, 500), "本来就在屏内就不动");
        assert_eq!(default_center((1920, 1080)), (960, 918), "主屏底部居中");
    }

    #[test]
    fn the_position_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("canto-lyrics-center-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("desktop-lyrics.json");

        save_center(&path, (123, 456)).expect("落盘");
        assert_eq!(load_center(&path), Some((123, 456)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_position_file_falls_back_to_the_default() {
        let dir = std::env::temp_dir().join(format!("canto-lyrics-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("desktop-lyrics.json");

        assert_eq!(load_center(&path), None, "文件不在就是 None");
        std::fs::write(&path, "{ 不是 JSON").expect("写坏文件");
        assert_eq!(load_center(&path), None, "坏文件当没有");
        std::fs::remove_dir_all(&dir).ok();
    }
}
