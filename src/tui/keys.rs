//! 键位 → 动作。
//!
//! 抽成纯函数的理由：键位优先级（模态 > 输入态 > 焦点）是最容易写错、又最难靠手点
//! 验证的一部分，放在这里可以用单测钉住；`state` 只负责把 `Action` 派发出去。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 界面动作。绘制与状态机只认这个枚举，不认按键。
///
/// 只放**当前真的接了行为**的动作：先加一个恒为空的 `Action` 分支，等于给按键
/// 装了个没接线的开关——菜单路由（Task 6/9）、队列（Task 11/12）、帮助与登出
/// （Task 9/16）落地时各自往这里加。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    /// `?`：帮助浮层开 / 关。
    Help,
    /// 数字键切屏（下标对应 `Menu::ALL`）。
    Menu(usize),
    Up,
    Down,
    First,
    Last,
    PageUp,
    PageDown,
    Enter,
    Back,
    PlayPause,
    Next,
    Prev,
    SeekBack,
    SeekForward,
    VolumeUp,
    VolumeDown,
    /// `a` / `A`：追加到队尾 / 插到下一首。
    AppendQueue,
    InsertQueue,
    /// `m`：切换播放模式（顺序 / 列表循环 / 单曲 / 随机）。
    CycleMode,
    /// `u`：队列浮层开 / 关。
    QueueOverlay,
    /// 浮层里 `d` / `Shift+D`：删掉选中项 / 清空队列。
    QueueDelete,
    QueueClear,
    /// 历史屏里 `Shift+D`：清空整份播放历史。
    HistoryClear,
    /// 帮助浮层里 `L`：登出（清本地会话回登录屏）。
    Logout,
    /// 登录屏：`c` 粘贴 cookie、`o` 打开二维码图片、`r` 重新扫码。
    LoginCookie,
    LoginOpenPng,
    LoginRetry,
    AddToPlaylist,
    RemoveFromPlaylist,
    Download,
    ToggleLyric,
    /// `Y`：全屏视图（占内容区，播放条与状态行不动）。
    ToggleNowPlaying,
    /// `/`：在当前列表里筛（**不**跳去搜索屏）。
    Filter,
    FilterChar(char),
    FilterBackspace,
    FilterSubmit,
    FilterCancel,
    InputChar(char),
    InputSubmit,
    InputCancel,
    InputBackspace,
    /// 全局热键 `lock`：切桌面歌词的锁定/解锁（界面上没有对应按键）。
    LyricsLock,
    /// 全局热键 `lyrics`：开 / 关桌面歌词窗口——关掉是把窗口**拆了**（不是藏起来），
    /// 状态记在 `desktop-lyrics.json`（界面上没有对应按键）。
    LyricsWindow,
}

/// 按键时的上下文：模态与输入态在这里体现，`resolve` 不再自己猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// 主界面（菜单栏去掉后只剩这一块内容区）。
    Content,
    /// 全屏视图：内容区被它占满，只留「看得见」的那些键。
    NowPlaying,
    /// 搜索输入框正在吃键。
    SearchInput,
    /// `/` 打开的列表筛选框（跟搜索框一样是文本输入，只是 Enter 的含义不同）。
    FilterInput,
    /// `p` 弹出的歌单选择框。
    PlaylistPicker,
    /// 帮助浮层。
    HelpOverlay,
    /// 队列浮层。
    QueueOverlay,
    /// 登录屏（cookie 输入态复用 `SearchInput`）。
    Login,
}

/// 键位映射。`None` = 这个键在当前上下文没有含义（调用方忽略）。
pub fn resolve(context: Context, key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }
    match context {
        Context::FilterInput => match key.code {
            KeyCode::Enter => Some(Action::FilterSubmit),
            KeyCode::Esc => Some(Action::FilterCancel),
            KeyCode::Backspace => Some(Action::FilterBackspace),
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::FilterChar(character))
            }
            _ => None,
        },
        Context::SearchInput => match key.code {
            KeyCode::Enter => Some(Action::InputSubmit),
            KeyCode::Esc => Some(Action::InputCancel),
            KeyCode::Backspace => Some(Action::InputBackspace),
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::InputChar(character))
            }
            _ => None,
        },
        Context::HelpOverlay => match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::F(1) => Some(Action::Help),
            KeyCode::Char('L') => Some(Action::Logout),
            // 表格比浮层长，得能滚：跟队列浮层同一套键（两者可能同时开着，派发时帮助优先）。
            KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
            KeyCode::Char('g') | KeyCode::Home => Some(Action::First),
            KeyCode::Char('G') | KeyCode::End => Some(Action::Last),
            _ => None,
        },
        Context::Login => match key.code {
            KeyCode::Char('c') => Some(Action::LoginCookie),
            KeyCode::Char('o') => Some(Action::LoginOpenPng),
            KeyCode::Char('r') => Some(Action::LoginRetry),
            KeyCode::Char('q') => Some(Action::Quit),
            _ => None,
        },
        Context::QueueOverlay => match key.code {
            KeyCode::Esc | KeyCode::Char('u') => Some(Action::QueueOverlay),
            KeyCode::Enter => Some(Action::Enter),
            KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
            KeyCode::Char('g') | KeyCode::Home => Some(Action::First),
            KeyCode::Char('G') | KeyCode::End => Some(Action::Last),
            KeyCode::Char('d') => Some(Action::QueueDelete),
            KeyCode::Char('D') => Some(Action::QueueClear),
            _ => None,
        },
        Context::PlaylistPicker => match key.code {
            KeyCode::Esc => Some(Action::Back),
            KeyCode::Enter => Some(Action::Enter),
            KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
            _ => None,
        },
        Context::NowPlaying => match key.code {
            KeyCode::Esc => Some(Action::Back),
            KeyCode::Char('Y') => Some(Action::ToggleNowPlaying),
            KeyCode::Char(digit @ '1'..='4') => Some(Action::Menu(digit as usize - '1' as usize)),
            KeyCode::Char('?') | KeyCode::F(1) => Some(Action::Help),
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('L') => Some(Action::Logout),
            KeyCode::Char(' ') => Some(Action::PlayPause),
            KeyCode::Char('n') => Some(Action::Next),
            KeyCode::Char('b') => Some(Action::Prev),
            KeyCode::Left | KeyCode::Char('h') => Some(Action::SeekBack),
            KeyCode::Right | KeyCode::Char('l') => Some(Action::SeekForward),
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('.') => Some(Action::VolumeUp),
            KeyCode::Char('-') | KeyCode::Char(',') => Some(Action::VolumeDown),
            KeyCode::Char('m') => Some(Action::CycleMode),
            KeyCode::Char('u') => Some(Action::QueueOverlay),
            // 列表看不见，就不给「对选中那首动手」的键留门（f/d/p/x/a/A/Enter）。
            _ => None,
        },
        Context::Content => common(key),
    }
}

/// 主界面的键位（`Enter` 的落点由 `state` 按当前屏决定）。
fn common(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        // 数字键切屏——菜单栏去掉后这是唯一的切屏入口，帮助浮层里有这张表。
        KeyCode::Char(digit @ '1'..='4') => Some(Action::Menu(digit as usize - '1' as usize)),
        KeyCode::Char('q') => Some(Action::Quit),
        // 登出：从帮助浮层里挪出来，主界面（含全屏视图）也能按。模态浮层另算——
        // 它们开着时是「一个小编辑器」，仍然吞键（见各自的 match）。
        KeyCode::Char('L') => Some(Action::Logout),
        KeyCode::Char('?') | KeyCode::F(1) => Some(Action::Help),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
        KeyCode::Char('g') | KeyCode::Home => Some(Action::First),
        KeyCode::Char('G') | KeyCode::End => Some(Action::Last),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::Char('u') if ctrl => Some(Action::PageUp),
        KeyCode::Char('d') if ctrl => Some(Action::PageDown),
        KeyCode::Enter => Some(Action::Enter),
        KeyCode::Char(' ') => Some(Action::PlayPause),
        KeyCode::Char('n') => Some(Action::Next),
        KeyCode::Char('b') => Some(Action::Prev),
        KeyCode::Left | KeyCode::Char('h') => Some(Action::SeekBack),
        KeyCode::Right | KeyCode::Char('l') => Some(Action::SeekForward),
        KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('.') => Some(Action::VolumeUp),
        KeyCode::Char('-') | KeyCode::Char(',') => Some(Action::VolumeDown),
        KeyCode::Char('a') => Some(Action::AppendQueue),
        KeyCode::Char('A') => Some(Action::InsertQueue),
        KeyCode::Char('m') => Some(Action::CycleMode),
        KeyCode::Char('p') => Some(Action::AddToPlaylist),
        KeyCode::Char('u') => Some(Action::QueueOverlay),
        KeyCode::Char('D') => Some(Action::HistoryClear),
        KeyCode::Char('x') => Some(Action::RemoveFromPlaylist),
        KeyCode::Char('d') => Some(Action::Download),
        KeyCode::Char('y') => Some(Action::ToggleLyric),
        // 大写 `Y` = 全屏视图（`y` 是底部那一块歌词的开关，两者独立）。
        KeyCode::Char('Y') => Some(Action::ToggleNowPlaying),
        // `/` 筛当前列表；搜索屏有自己的入口（菜单第 3 项）。
        KeyCode::Char('/') => Some(Action::Filter),
        // 可能有上一层（「我的歌单 › 某歌单」）：Esc 一层层退。
        KeyCode::Esc | KeyCode::Backspace => Some(Action::Back),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(context: Context, code: KeyCode) -> Option<Action> {
        resolve(context, key(code))
    }

    /// `y` 是底部那块歌词的开关，`Y` 是全屏视图；全屏里只留「看得见」的键。
    #[test]
    fn uppercase_y_opens_the_fullscreen_view_and_hides_list_keys() {
        assert_eq!(
            press(Context::Content, KeyCode::Char('y')),
            Some(Action::ToggleLyric)
        );
        assert_eq!(
            press(Context::Content, KeyCode::Char('Y')),
            Some(Action::ToggleNowPlaying)
        );

        // 全屏里保留的：播放、切歌、进度、音量、模式、切屏、队列、帮助、退出。
        for (code, action) in [
            (KeyCode::Esc, Action::Back),
            (KeyCode::Char('Y'), Action::ToggleNowPlaying),
            (KeyCode::Char('2'), Action::Menu(1)),
            (KeyCode::Char(' '), Action::PlayPause),
            (KeyCode::Char('n'), Action::Next),
            (KeyCode::Char('b'), Action::Prev),
            (KeyCode::Char('h'), Action::SeekBack),
            (KeyCode::Char('l'), Action::SeekForward),
            (KeyCode::Char('+'), Action::VolumeUp),
            (KeyCode::Char('-'), Action::VolumeDown),
            (KeyCode::Char('m'), Action::CycleMode),
            (KeyCode::Char('u'), Action::QueueOverlay),
            (KeyCode::Char('?'), Action::Help),
            (KeyCode::Char('q'), Action::Quit),
        ] {
            assert_eq!(
                press(Context::NowPlaying, code),
                Some(action),
                "全屏视图里 {code:?} 该是 {action:?}"
            );
        }

        // 列表看不见，就不给「对选中那首动手」的键留门。
        for code in [
            KeyCode::Enter,
            KeyCode::Char('d'),
            KeyCode::Char('p'),
            KeyCode::Char('x'),
            KeyCode::Char('a'),
            KeyCode::Char('A'),
            KeyCode::Char('/'),
            KeyCode::Char('j'),
        ] {
            assert_eq!(press(Context::NowPlaying, code), None, "{code:?} 在全屏里该没绑定");
        }
    }

    /// `/` 是「筛当前列表」，不再跳去搜索屏（搜索屏有自己的入口：菜单第 3 项）。
    #[test]
    fn slash_filters_the_current_list() {
        assert_eq!(
            press(Context::Content, KeyCode::Char('/')),
            Some(Action::Filter)
        );
        assert_eq!(
            press(Context::FilterInput, KeyCode::Char('/')),
            Some(Action::FilterChar('/')),
            "筛选框里再按 / 就是打一个斜杠"
        );
    }

    /// 筛选框跟搜索框一样收字，只是 Enter 的含义不同（确认筛选 vs 发搜索）。
    #[test]
    fn the_filter_box_takes_text_until_enter() {
        assert_eq!(
            press(Context::FilterInput, KeyCode::Char('晴')),
            Some(Action::FilterChar('晴'))
        );
        assert_eq!(
            press(Context::FilterInput, KeyCode::Backspace),
            Some(Action::FilterBackspace)
        );
        assert_eq!(
            press(Context::FilterInput, KeyCode::Enter),
            Some(Action::FilterSubmit)
        );
        assert_eq!(
            press(Context::FilterInput, KeyCode::Esc),
            Some(Action::FilterCancel)
        );
        assert_eq!(
            press(Context::FilterInput, KeyCode::Char('q')),
            Some(Action::FilterChar('q')),
            "输入框里的 q 是打字，不是退出"
        );
        assert_eq!(
            press(Context::FilterInput, KeyCode::Char('j')),
            Some(Action::FilterChar('j')),
            "输入框里 j/k 不该挪高亮"
        );
    }

    #[test]
    fn ctrl_c_quits_from_every_context() {
        for context in [
            Context::Content,
            Context::SearchInput,
            Context::PlaylistPicker,
            Context::QueueOverlay,
            Context::HelpOverlay,
            Context::Login,
        ] {
            let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
            assert_eq!(resolve(context, event), Some(Action::Quit), "{context:?}");
        }
    }

    #[test]
    fn typing_in_the_search_box_never_quits_or_navigates() {
        for character in ['q', '/', 'j', ' ', 'a'] {
            assert_eq!(
                press(Context::SearchInput, KeyCode::Char(character)),
                Some(Action::InputChar(character)),
                "{character} 该原样进输入框"
            );
        }
        assert_eq!(press(Context::SearchInput, KeyCode::Enter), Some(Action::InputSubmit));
        assert_eq!(press(Context::SearchInput, KeyCode::Esc), Some(Action::InputCancel));
        assert_eq!(
            press(Context::SearchInput, KeyCode::Backspace),
            Some(Action::InputBackspace)
        );
    }

    #[test]
    fn the_picker_swallows_everything_it_does_not_use() {
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Esc), Some(Action::Back));
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Enter), Some(Action::Enter));
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Char('j')), Some(Action::Down));
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Char('k')), Some(Action::Up));
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Char('a')), None);
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Char('y')), None);
    }

    /// 登出从帮助浮层里挪出来了：主界面与全屏视图都能按（模态浮层与输入框不抢）。
    #[test]
    fn logout_is_available_from_the_main_screen() {
        assert_eq!(press(Context::Content, KeyCode::Char('L')), Some(Action::Logout));
        assert_eq!(press(Context::NowPlaying, KeyCode::Char('L')), Some(Action::Logout));
        assert_eq!(press(Context::HelpOverlay, KeyCode::Char('L')), Some(Action::Logout));
        // 模态浮层开着时仍然吞键：它们此刻是「正开着的小编辑器」，不往里面塞登出。
        assert_eq!(press(Context::QueueOverlay, KeyCode::Char('L')), None);
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Char('L')), None);
        // 输入态里 `L` 是打字。
        assert_eq!(press(Context::SearchInput, KeyCode::Char('L')), Some(Action::InputChar('L')));
        assert_eq!(press(Context::FilterInput, KeyCode::Char('L')), Some(Action::FilterChar('L')));
        // 小写 `l` 还是快进 10 秒，别跟登出混了。
        assert_eq!(press(Context::Content, KeyCode::Char('l')), Some(Action::SeekForward));
    }

    #[test]
    fn the_main_screen_has_one_key_table() {
        for code in [
            KeyCode::Char('j'),
            KeyCode::Char('G'),
            KeyCode::PageDown,
            KeyCode::Char('n'),
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Char('a'),
            KeyCode::Char('A'),
            KeyCode::Char('m'),
            KeyCode::Char('u'),
            KeyCode::Char('D'),
            KeyCode::Char('p'),
        ] {
            assert!(
                press(Context::Content, code).is_some(),
                "{code:?} 该有绑定"
            );
        }
        assert_eq!(
            press(Context::Content, KeyCode::Tab),
            None,
            "菜单栏没了，Tab 没有第二站可去"
        );
    }

    #[test]
    fn volume_seek_and_page_keys_have_both_spellings() {
        assert_eq!(press(Context::Content, KeyCode::Char('+')), Some(Action::VolumeUp));
        assert_eq!(press(Context::Content, KeyCode::Char('.')), Some(Action::VolumeUp));
        assert_eq!(press(Context::Content, KeyCode::Char('-')), Some(Action::VolumeDown));
        assert_eq!(press(Context::Content, KeyCode::Char(',')), Some(Action::VolumeDown));
        assert_eq!(press(Context::Content, KeyCode::Char('h')), Some(Action::SeekBack));
        assert_eq!(press(Context::Content, KeyCode::Left), Some(Action::SeekBack));
        assert_eq!(press(Context::Content, KeyCode::End), Some(Action::Last));
        let page_up = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(resolve(Context::Content, page_up), Some(Action::PageUp));
    }

    #[test]
    fn digits_are_the_only_way_to_switch_screens() {
        assert_eq!(press(Context::Content, KeyCode::Char('1')), Some(Action::Menu(0)));
        assert_eq!(press(Context::Content, KeyCode::Char('3')), Some(Action::Menu(2)));
        assert_eq!(press(Context::Content, KeyCode::Char('4')), Some(Action::Menu(3)));
        assert_eq!(press(Context::Content, KeyCode::Char('5')), None, "只有四项");
    }

    #[test]
    fn esc_and_backspace_go_back_a_level() {
        assert_eq!(press(Context::Content, KeyCode::Esc), Some(Action::Back));
        assert_eq!(press(Context::Content, KeyCode::Backspace), Some(Action::Back));
    }

    #[test]
    fn help_overlay_opens_and_only_esc_or_question_closes_it() {
        assert_eq!(press(Context::Content, KeyCode::Char('?')), Some(Action::Help));
        assert_eq!(press(Context::Content, KeyCode::F(1)), Some(Action::Help));
        assert_eq!(press(Context::HelpOverlay, KeyCode::Esc), Some(Action::Help));
        assert_eq!(press(Context::HelpOverlay, KeyCode::Char('?')), Some(Action::Help));
        assert_eq!(press(Context::HelpOverlay, KeyCode::Char('L')), Some(Action::Logout));
        assert_eq!(press(Context::HelpOverlay, KeyCode::Char('a')), None, "浮层吞键");
        assert_eq!(press(Context::HelpOverlay, KeyCode::Enter), None, "浮层里的回车也不该穿透");
    }

    /// 帮助浮层用跟队列浮层同一套滚动键（可能两者同时开着，优先给帮助）。
    #[test]
    fn the_help_overlay_scrolls_with_the_queue_keys() {
        for (key, action) in [
            (KeyCode::Char('j'), Action::Down),
            (KeyCode::Down, Action::Down),
            (KeyCode::Char('k'), Action::Up),
            (KeyCode::Up, Action::Up),
            (KeyCode::Char('g'), Action::First),
            (KeyCode::Home, Action::First),
            (KeyCode::Char('G'), Action::Last),
            (KeyCode::End, Action::Last),
        ] {
            assert_eq!(press(Context::HelpOverlay, key), Some(action), "{key:?}");
        }
        assert_eq!(press(Context::HelpOverlay, KeyCode::PageDown), None, "翻页没绑，别悄悄吞语义");
    }

    #[test]
    fn the_login_screen_keeps_its_keys_to_itself() {
        assert_eq!(press(Context::Login, KeyCode::Char('c')), Some(Action::LoginCookie));
        assert_eq!(press(Context::Login, KeyCode::Char('o')), Some(Action::LoginOpenPng));
        assert_eq!(press(Context::Login, KeyCode::Char('r')), Some(Action::LoginRetry));
        assert_eq!(press(Context::Login, KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(press(Context::Login, KeyCode::Char('j')), None, "登录屏没有列表可走");
        assert_eq!(press(Context::Login, KeyCode::Enter), None, "回车只在输入态里提交");
    }

    #[test]
    fn unbound_keys_stay_unbound() {
        assert_eq!(press(Context::Content, KeyCode::Char('z')), None);
        assert_eq!(press(Context::QueueOverlay, KeyCode::Char('u')), Some(Action::QueueOverlay));
        assert_eq!(press(Context::QueueOverlay, KeyCode::Enter), Some(Action::Enter));
        assert_eq!(press(Context::QueueOverlay, KeyCode::Char('d')), Some(Action::QueueDelete));
        assert_eq!(press(Context::QueueOverlay, KeyCode::Char('D')), Some(Action::QueueClear));
        assert_eq!(press(Context::QueueOverlay, KeyCode::Char('p')), None, "浮层吞键");
        assert_eq!(press(Context::PlaylistPicker, KeyCode::Char('?')), None, "选择框里不开帮助");
        assert_eq!(press(Context::Content, KeyCode::Char('z')), None);
        assert_eq!(press(Context::QueueOverlay, KeyCode::Char('z')), None);
    }
}
