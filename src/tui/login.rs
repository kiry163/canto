//! 登录屏：终端里画二维码、粘贴 cookie、会话失效后的回退处。
//!
//! 与主界面共用同一个 ratatui 终端与事件循环——不是一个程序退出再进另一个，
//! 所以二维码、粘贴输入、错误提示都在同一屏里。

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::paths;
use crate::qq;
use crate::qr;
use crate::session;

use super::keys;
use super::state::Msg;

/// 最多换几张二维码。
const QR_ATTEMPTS: u32 = 3;
/// 轮询间隔。
const QR_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 终端高度里留给边框与两行文案的行数（上下边框 + 提示行 + 状态行）。
/// 少留一行，大一点点的终端就白判「装不下」。
const CHROME_ROWS: u16 = 4;
/// 每个模块最多放大到几像素：再大只是白占屏幕，扫起来并不更省事。
const MAX_SCALE: u16 = 4;
/// 登录屏的阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoginPhase {
    /// 正在取二维码。
    Loading,
    /// 二维码在屏幕上，等扫码。
    Ready,
    /// 扫码后等手机上确认。
    Scanned,
    /// 出错了（取码失败 / 多次过期 / 服务端拒绝），按 `r` 重来。
    Failed(String),
}

/// 登录屏状态。
#[derive(Debug)]
pub(super) struct LoginState {
    pub phase: LoginPhase,
    /// 二维码的模块网格（还没取到、或解不出来时是 `None`）。
    pub modules: Option<qr::Modules>,
    /// 状态行文案。
    pub message: String,
    /// `Some` = cookie 输入态（整串 cookie，可能几百字符，绘制时横向截取）。
    pub input: Option<String>,
    /// 已经换过几张二维码。
    pub attempt: u32,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            phase: LoginPhase::Loading,
            modules: None,
            message: "正在取二维码…".to_string(),
            input: None,
            attempt: 1,
        }
    }
}

impl LoginState {
    /// 当前终端放得下的**最大**放大倍数（4/2/1）；连 1 倍都放不下就是 `None`。
    ///
    /// 每模块 `scale` 像素：横向占 `scale` 列，纵向两个像素一行所以是一半的行数。
    /// 奇数倍（3）会让模块跨在半格上对不齐，所以只认 1 与偶数。
    fn scale_for(&self, columns: u16, rows: u16) -> Option<u16> {
        let (width, height) = self.modules.as_ref()?.dims();
        (1..=MAX_SCALE).rev().find(|scale| {
            (*scale == 1 || scale.is_multiple_of(2)) && fits(width, height, *scale, columns, rows)
        })
    }

    /// 终端装不下二维码时，给一句「需要多大」；装得下就是 `None`。
    pub fn size_hint(&self, columns: u16, rows: u16) -> Option<String> {
        let (width, height) = self.modules.as_ref()?.dims();
        if self.scale_for(columns, rows).is_some() {
            return None;
        }
        let need_columns = width as u16 + 1;
        let need_rows = (height as u16).div_ceil(2) + CHROME_ROWS;
        // 一行放得下：状态行只有一行高，换行会把后半句（当前尺寸）裁掉。
        Some(format!(
            "装不下二维码：需要 ≥{need_columns}×{need_rows}（当前 {columns}×{rows}），按 c 粘贴 cookie"
        ))
    }

    /// 输入框里显示的内容（横向只留尾部，整串 cookie 有几百字符）。
    fn input_tail(&self, width: u16) -> String {
        let Some(buffer) = &self.input else {
            return String::new();
        };
        let limit = width.saturating_sub(3).max(8) as usize;
        let characters: Vec<char> = buffer.chars().collect();
        if characters.len() <= limit {
            return buffer.clone();
        }
        characters[characters.len() - limit..].iter().collect()
    }
}

/// 一个模块 `scale` 像素时，`width`×`height` 个模块要多少终端格子。
fn fits(width: usize, height: usize, scale: u16, columns: u16, rows: u16) -> bool {
    let scale = scale as usize;
    width * scale <= columns as usize
        && (height * scale).div_ceil(2) + CHROME_ROWS as usize <= rows as usize
}

/// 扫码登录的完整流程跑在 tokio 任务里，每一步结果都用 `Msg` 送回界面线程。
///
/// 过期就自动换一张（最多 [`QR_ATTEMPTS`] 张），全部过期后把状态置回 `Failed`，
/// 由使用者按 `r` 决定要不要再来一轮——无限刷码只会在服务端留下垃圾会话。
pub(super) fn spawn_qr_login(handle: &tokio::runtime::Handle, tx: std::sync::mpsc::Sender<Msg>) {
    handle.spawn(async move {
        for attempt in 1..=QR_ATTEMPTS {
            let png = match qq::login::create().await {
                Ok(png) => png,
                Err(error) => {
                    fail(&tx, format!("取二维码失败：{error:#}"));
                    return;
                }
            };
            let _ = tx.send(Msg::Qr { png, attempt });
            loop {
                match qq::login::poll().await {
                    Ok(qq::login::Poll::Waiting) => tokio::time::sleep(QR_POLL_INTERVAL).await,
                    Ok(qq::login::Poll::Scanned) => {
                        let _ = tx.send(Msg::QrScanned);
                        tokio::time::sleep(QR_POLL_INTERVAL).await;
                    }
                    // 过期：跳出内层循环，外层换一张。
                    Ok(qq::login::Poll::Expired) => break,
                    Ok(qq::login::Poll::Success(identity)) => {
                        let _ = tx.send(Msg::LoggedIn(identity));
                        return;
                    }
                    Ok(qq::login::Poll::Failed(message)) => {
                        fail(&tx, message);
                        return;
                    }
                    Err(error) => {
                        fail(&tx, format!("{error:#}"));
                        return;
                    }
                }
            }
        }
        fail(&tx, format!("二维码已过期 {QR_ATTEMPTS} 张，按 r 重新扫码"));
    });
}

fn fail(tx: &std::sync::mpsc::Sender<Msg>, message: String) {
    let _ = tx.send(Msg::LoginFailed(message));
}

/// 画登录屏。
pub(super) fn draw(state: &LoginState, frame: &mut Frame, area: Rect) {
    let block = Block::bordered().title(" 登录 canto ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // 二维码居中，其余行留给文案与提示。尺寸提示按**终端**大小算：使用者关心的是
    // 「窗口要拉多大」，不是这块带边框的区域有多大。窗口拉大/缩小会在下一帧重画，
    // 所以放大倍数每次绘制按当前尺寸重挑。
    let hint = state.size_hint(area.width, area.height);
    let lines = match (state.scale_for(area.width, area.height), &state.modules) {
        (Some(scale), Some(modules)) => qr::render(modules, scale),
        _ => Vec::new(),
    };
    let rows = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length((lines.len() as u16).min(inner.height.saturating_sub(2))),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(inner);

    if !lines.is_empty() {
        let qr_text: Vec<Line> = lines.iter().map(|line| Line::from(line.clone())).collect();
        frame.render_widget(
            Paragraph::new(qr_text).alignment(Alignment::Center),
            rows[1],
        );
    }

    let prompt = match &state.input {
        Some(_) => Line::from(vec![
            Span::styled("cookie：", Style::new().fg(Color::Yellow)),
            Span::raw(state.input_tail(inner.width)),
            Span::styled("▏", Style::new().fg(Color::Yellow)),
        ]),
        None => Line::from("手机 QQ 扫一扫 · [c] 粘贴 cookie · [o] 打开二维码图片"),
    };
    frame.render_widget(Paragraph::new(prompt).alignment(Alignment::Center), rows[2]);

    let message = match (&hint, &state.phase) {
        (_, LoginPhase::Failed(error)) => format!("✗ {error}"),
        (Some(hint), _) => hint.clone(),
        (None, LoginPhase::Scanned) => "已扫码，请在手机上确认登录…".to_string(),
        (None, LoginPhase::Loading) => "正在取二维码…".to_string(),
        (None, LoginPhase::Ready) => format!("等待扫码…（第 {} 张）", state.attempt),
    };
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(Style::new().fg(Color::DarkGray)),
        rows[3],
    );
}

/// 登录屏的按键上下文：cookie 输入态复用搜索框那一套（Enter 提交 / Esc 退出 / Backspace 删字符）。
pub(super) fn context(input: bool) -> keys::Context {
    if input {
        keys::Context::SearchInput
    } else {
        keys::Context::Login
    }
}

/// 把粘贴的 cookie 写进会话并落盘。失败就原样报出来（就地显示，不留到日志里）。
///
/// 落盘路径由调用方给（同 `sign_out`）：测试不该往使用者真实的 `~/.config/canto/session.json` 里写。
pub(super) fn login_with_cookie(raw: &str, path: &std::path::Path) -> Result<String, String> {
    let jar = session::jar();
    let state = jar.login_with_cookie(raw).map_err(|error| format!("{error:#}"))?;
    jar.save_to(path).map_err(|error| format!("{error:#}"))?;
    Ok(state.identity.unwrap_or_default())
}

/// `o`：用系统查看器打开二维码图片（想用相册扫、或终端里扫不出来时的兜底）。
pub(super) fn open_qr_png() -> Result<(), String> {
    let path = paths::qr_png_path();
    if !path.exists() {
        return Err("二维码图片还没生成".to_string());
    }
    qr::open_with_system(&path).map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一张 21 模块（+2 模块静区 = 25 单位）的假二维码：1 倍占 25 列 × 13 行。
    fn state_with_qr() -> LoginState {
        let png = qr::test_support::png(&qr::test_support::checkerboard(21), 3, 2);
        LoginState {
            modules: Some(qr::modules(&png).expect("还原模块网格")),
            ..LoginState::default()
        }
    }

    #[test]
    fn a_short_terminal_says_what_size_is_needed_instead_of_drawing_half_a_qr() {
        let state = state_with_qr();
        let hint = state.size_hint(20, 10).expect("20 列装不下");
        assert!(hint.contains("需要 ≥26×17"), "提示里要有需要的尺寸：{hint}");
        assert!(hint.contains("当前 20×10"), "也要说当前多大：{hint}");
        assert!(hint.chars().count() < 60, "一行放得下才不会被裁：{hint}");
        assert!(state.size_hint(160, 70).is_none(), "装得下就不提示");
        assert!(
            LoginState::default().size_hint(40, 10).is_none(),
            "还没有二维码时不提示尺寸"
        );
    }

    #[test]
    fn the_qr_grows_by_whole_modules_when_the_terminal_is_big() {
        let state = state_with_qr();
        assert_eq!(state.scale_for(80, 24), Some(1), "25 列 × 13 行，1 倍就够");
        assert_eq!(state.scale_for(120, 50), Some(2));
        assert_eq!(state.scale_for(200, 120), Some(4), "再大也停在 4 倍");
        assert_eq!(state.scale_for(24, 24), None, "连 1 倍都放不下");
        assert_eq!(LoginState::default().scale_for(200, 120), None, "还没有二维码");
    }

    #[test]
    fn cookie_input_is_kept_but_only_its_tail_is_drawn() {
        let state = LoginState {
            input: Some("uin=o1; qqmusic_key=abcdef".to_string()),
            ..LoginState::default()
        };
        let tail = state.input_tail(12);
        assert_eq!(tail.chars().count(), 9, "只留尾部 9 个字符");
        assert!(tail.ends_with("abcdef"), "留下的该是结尾：{tail}");
        assert_eq!(state.input_tail(200), "uin=o1; qqmusic_key=abcdef", "装得下就整串");
    }

    #[test]
    fn the_cookie_box_reuses_the_search_input_context() {
        assert_eq!(context(true), keys::Context::SearchInput);
        assert_eq!(context(false), keys::Context::Login);
    }
}
