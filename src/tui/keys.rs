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
    Up,
    Down,
    First,
    Last,
    PageUp,
    PageDown,
    FocusNext,
    FocusPrev,
    Enter,
    Back,
    PlayPause,
    Next,
    Prev,
    SeekBack,
    SeekForward,
    VolumeUp,
    VolumeDown,
    AddToPlaylist,
    RemoveFromPlaylist,
    Favorite,
    Download,
    ToggleLyric,
    Search,
    InputChar(char),
    InputSubmit,
    InputCancel,
    InputBackspace,
}

/// 按键时的上下文：模态与输入态在这里体现，`resolve` 不再自己猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// 焦点在左栏（歌单）。
    Menu,
    /// 焦点在右栏（曲目）。
    Content,
    /// 搜索输入框正在吃键。
    SearchInput,
    /// `a` 弹出的歌单选择框。
    PlaylistPicker,
}

/// 键位映射。`None` = 这个键在当前上下文没有含义（调用方忽略）。
pub fn resolve(context: Context, key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }
    match context {
        Context::SearchInput => match key.code {
            KeyCode::Enter => Some(Action::InputSubmit),
            KeyCode::Esc => Some(Action::InputCancel),
            KeyCode::Backspace => Some(Action::InputBackspace),
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::InputChar(character))
            }
            _ => None,
        },
        Context::PlaylistPicker => match key.code {
            KeyCode::Esc => Some(Action::Back),
            KeyCode::Enter => Some(Action::Enter),
            KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
            _ => None,
        },
        Context::Menu | Context::Content => common(key, context),
    }
}

/// 左栏 / 右栏共用的键位（焦点差异只有 `Tab` 与 `Enter` 的落点，都在 `state` 里判）。
fn common(key: KeyEvent, context: Context) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Tab => Some(Action::FocusNext),
        KeyCode::BackTab => Some(Action::FocusPrev),
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
        KeyCode::Char('a') => Some(Action::AddToPlaylist),
        KeyCode::Char('x') => Some(Action::RemoveFromPlaylist),
        KeyCode::Char('f') => Some(Action::Favorite),
        KeyCode::Char('d') => Some(Action::Download),
        KeyCode::Char('y') => Some(Action::ToggleLyric),
        KeyCode::Char('/') => Some(Action::Search),
        // 左栏是最外层，Esc 没有「上级」可回；右栏的返回语义等嵌套层（Task 6）再接。
        _ => {
            let _ = context;
            None
        }
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

    #[test]
    fn ctrl_c_quits_from_every_context() {
        for context in [
            Context::Menu,
            Context::Content,
            Context::SearchInput,
            Context::PlaylistPicker,
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

    #[test]
    fn the_two_lists_share_one_key_table() {
        for context in [Context::Menu, Context::Content] {
            assert_eq!(press(context, KeyCode::Char('j')), Some(Action::Down));
            assert_eq!(press(context, KeyCode::Char('G')), Some(Action::Last));
            assert_eq!(press(context, KeyCode::PageDown), Some(Action::PageDown));
            assert_eq!(press(context, KeyCode::Char('n')), Some(Action::Next));
            assert_eq!(press(context, KeyCode::Char(' ')), Some(Action::PlayPause));
            assert_eq!(press(context, KeyCode::Enter), Some(Action::Enter));
            assert_eq!(press(context, KeyCode::Char('a')), Some(Action::AddToPlaylist));
            assert_eq!(press(context, KeyCode::Tab), Some(Action::FocusNext));
        }
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
    fn unbound_keys_stay_unbound() {
        assert_eq!(press(Context::Content, KeyCode::Char('z')), None);
        assert_eq!(press(Context::Content, KeyCode::Esc), None, "阶段 2 还没有可返回的层级");
        assert_eq!(press(Context::Content, KeyCode::Char('m')), None, "播放模式随队列一起接");
        assert_eq!(press(Context::Menu, KeyCode::Char('?')), None, "帮助浮层在 Task 9");
    }
}
