# canto 桌面歌词（macOS）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** macOS 上播放时在桌面之上浮一条歌词（当前句 + 译文 + 下一句），不抢焦点、默认鼠标穿透、`ctrl+shift+l` 解锁后可拖动，位置重启后还在。

**Architecture:** 平台无关那半（取词、`State::advance` → `Payload`、位置状态、`desktop-lyrics.json`）已在 Windows 那轮做完，一行不改。本次加两样：`DesktopLyrics::sync` 每帧调一次 `Imp::pump()`（新增的接口位），以及 `#[cfg(target_os = "macos")] mod imp` —— 一个无边框透明 `NSPanel` + 三个透明 `NSTextField`，**主线程**每帧把 AppKit 事件队列拉空派发（没有 `NSApplicationMain`，也没有第二个进程）。

**Tech Stack:** Rust edition 2024、`objc2 0.6` + `objc2-foundation 0.3` + `objc2-app-kit 0.3`（**在 macOS 上它们已经在依赖图里**——`global-hotkey` 拖进来的，`Cargo.lock` 里现成，这次只是把传递依赖写成直接依赖，**不新增下载**）。不引窗口库（winit/tao 没有「泵一次」的 API）、不引 core-graphics/foundation 之外的绑定。

**Spec:** `docs/superpowers/specs/2026-10-13-desktop-lyrics-macos-design.md`

## Global Constraints

- **平台**：只加 `#[cfg(target_os = "macos")] mod imp`。Windows 那份实现**一行不改**；`#[cfg(not(windows))]` 那份是 Linux 的最终形态（只加一个空 `pump`）。
- **状态机不动**：`State`/`Frame`/`Payload`/`Center`/`primary_screen`/`clamp_center`/`hidden_payload` 语义与状态文件名 `desktop-lyrics.json` 全部沿用；`overlay.rs` 平台无关那半唯一的改动是 `sync` 里多一句 `self.imp.pump()`。
- **更新节奏**：仍然只在 payload 变化时 `push`（逐帧零绘制、零分配）；但 `pump` **每帧都跑**，与 payload 是否变化无关——锁定态的拖动与重绘全靠它。
- **主线程**：`install` 第一句是 `MainThreadMarker::new()`，拿不到就降级返回错误。`install` 只在 `tui::run` 里调；`App::new` 与测试一律 `none()`（不在测试里真建窗口）。
- **坐标系**：状态文件存的是**左上原点**的中心点；进 AppKit 按主屏高换算，出来再换回去（`y_appkit = 屏高 − y_topleft`）。
- **配置与文案**：`[desktop_lyrics]` 九个键不动、解析与校验都在 `config.rs`；失败文案沿用「歌词窗口起不来」，细节进 `canto.log`（短句约定见 `docs/design.md` 状态行那节）。
- **观感常数**（真机对着 Windows 调）：次要行字号 = `font_size × 3/5`、内边距 8 点、描边 −3.0（正文）/−2.0（次要行）、浮层 `level = 3`。
- 每个 commit 都要三平台编得过：CI = Linux `clippy --all-targets --locked -- -D warnings` + ubuntu/windows/macos `cargo test --locked`。

## Review Focus

（spec 没铺开写、但真会踩的点，各挂到拥有它的任务上）

1. **`pump` 必须每帧都调**：拖动过程中 payload 是不变的，只在变化时泵会把拖动冻住。（Task 3 E2E 第 5 条专门验这个）
2. **`Retained<T>` 不是 `repr(transparent)`**：对象指针一律 `&*retained`，写成 `&retained` 会段错误（探针里踩过）。所有拼 `NSDictionary` 的地方都走 `any()` 助手。（Task 2 代码）
3. **坐标换算写反** → 窗口跑到屏幕外或上下颠倒。（Task 2 E2E 第 1 条看位置、Task 3 的往返）
4. **隐藏用 `orderOut` 而不是销毁**：位置与锁定态要留着，再显示时 `orderFrontRegardless` 回来。（Task 2 代码 + E2E 第 2 条）
5. **建窗口 / 激活策略失败** → 一句通知 + 日志，播放与终端界面照常。（Task 2 失败路径）

## 文件结构

| 文件 | 责任 |
|---|---|
| `src/overlay.rs` | ① `sync` 每帧泵 + 三份 `Imp` 各加 `pump`；② macOS `mod imp`（窗口、三行文字、事件泵、拖动回传） |
| `Cargo.toml` | 新增 `[target.'cfg(target_os = "macos")'.dependencies]`：`objc2` / `objc2-foundation` / `objc2-app-kit` |
| `README.md`、`docs/design.md` | 功能表、配置注释、已知限制、实现说明里的「桌面歌词只有 Windows」 |
| `docs/superpowers/specs/2026-10-12-desktop-lyrics-design.md` | 「其它平台是空实现」那两处已经过期 |
| `docs/superpowers/specs/2026-10-13-desktop-lyrics-macos-design.md` | §7 验收清单勾选；实测与预期不符的地方就改 |

---

# Task 1: 接口处改动——`Imp::pump` 与每帧泵

**Files:**
- Modify: `src/overlay.rs`（`DesktopLyrics::sync`；Windows `mod imp`、`#[cfg(not(windows))] mod imp` 各加一个空 `pump`；文件末尾 `#[cfg(test)] mod tests`）

**Interfaces:**
- Produces: `Imp::pump(&mut self)`（三份实现：macOS 真干活、Windows 空、Linux 空），由 `DesktopLyrics::sync` 每帧调用
- Consumes: 现有 `Frame` / `Payload` / `State::advance`（签名不变）

**这个任务没有像样的单测**：`pump` 的行为在平台实现里，`none()` 那份是空的——从外面观察不到「调没调」。**它的验收是编译 + 既有测试全绿**，那条「每帧都要泵」的不变式由 Task 3 的 E2E 第 5 条钉住（拖动时 payload 不变）。别为了写测试往生产代码里塞计数器。

- [ ] **Step 1: 给 `sync` 加每帧泵**

```rust
    /// 每帧调一次：先泵平台那边的事件，再有变化就把新 payload 推给窗口。
    ///
    /// 泵必须每帧都跑（payload 没变也要）：macOS 的拖动与重绘全靠它，而拖动过程中 payload
    /// 恰恰是不变的——只在变化时泵会把拖动冻住。
    pub fn sync(&mut self, frame: Frame<'_>, config: &DesktopLyricsConfig) {
        self.imp.pump();
        if let Some(payload) = self.state.advance(frame, config) {
            self.imp.push(payload);
        }
    }
```

- [ ] **Step 2: 三份实现各加一个 `pump`**

Windows（`#[cfg(target_os = "windows")] mod imp`，`impl Imp` 里，挨着 `push`）：

```rust
        /// Windows 的事件由窗口线程自己跑消息循环，主循环这边不用泵。
        pub(super) fn pump(&mut self) {}
```

非 Windows（`#[cfg(not(target_os = "windows"))] mod imp`，挨着空 `push`）：

```rust
        /// 不建窗口，没什么可泵的。
        pub(super) fn pump(&mut self) {}
```

- [ ] **Step 3: 既有测试必须全绿（含 `a_none_handle_syncs_nothing`）**

Run: `cargo test --locked overlay` 与 `cargo test --locked`
Expected: 全 PASS。`a_none_handle_syncs_nothing` 现在会顺带走一遍空 `pump`——如果它断言的是「句柄为空时 `sync`/`toggle_lock` 都是空操作」，那正是我们要的语义，不用改；若它断言了别的行为再看。

- [ ] **Step 4: clippy**

Run: `cargo clippy --all-targets --locked -- -D warnings`
Expected: 无输出（干净）。`pump(&mut self)` 在空实现上会触发 `clippy::unused_self`？不会——`unused_self` 只对 `&self` 且未使用才报，`&mut self` 不受影响。

- [ ] **Step 5: 提交**

```bash
git add src/overlay.rs
git commit -m "refactor(overlay): Imp::pump —— 每帧泵一次平台事件"
```

---

# Task 2: macOS imp——窗口、文字、每帧泵

**Files:**
- Modify: `Cargo.toml`（新增 macOS 直接依赖段）
- Modify: `src/overlay.rs`（新增 `#[cfg(target_os = "macos")] mod imp`，位置放在 Windows 那份与「其它平台」那份之间）

**Interfaces:**
- Consumes: `Payload`（`visible`/`current`/`next`/`translation`/`locked`）、`DesktopLyricsConfig`、`crate::config::Rgb`
- Produces:
  - `pub(super) type Started = (Imp, Option<Receiver<(i32, i32)>>, Option<String>);`（三份实现同名同形；本任务位置那项返回 `None`）
  - `Imp::none() -> Imp`、`Imp::push(&mut self, payload: Payload)`、`Imp::pump(&mut self)`（Task 1 已把调用点接好）
  - `pub(super) fn install(config: &DesktopLyricsConfig, center: (i32, i32)) -> Started`

- [ ] **Step 1: `Cargo.toml` 加段**

放在 `[target.'cfg(unix)'.dependencies]`（`libc`）之后、`[target.'cfg(windows)'.dependencies]` 之前：

```toml
[target.'cfg(target_os = "macos")'.dependencies]
# 桌面歌词的覆盖层：一个无边框浮层窗口 + 三个透明 label。三件都**已经在依赖图里**
# （`global-hotkey` 拉的），这里只是把传递依赖写成直接依赖，不新增下载。
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = [
    "NSString",
    "NSDictionary",
    "NSArray",
    "NSAttributedString",
    "NSGeometry",
    "NSValue",
    "NSDate",
    "NSRunLoop",
] }
objc2-app-kit = { version = "0.3", features = [
    "NSApplication",
    "NSWindow",
    "NSPanel",
    "NSView",
    "NSTextField",
    "NSScreen",
    "NSColor",
    "NSFont",
    "NSAttributedString",
    "NSEvent",
] }
```

- [ ] **Step 2: 写 macOS 实现**

```rust
/// macOS：无边框 `NSPanel` + 三个透明 `NSTextField`，主线程每帧手动泵 AppKit 事件。
///
/// 三件事值得记住：
/// - 窗口必须在**主线程**建（AppKit 的硬要求）：`install` 第一句就是 `MainThreadMarker::new()`，
///   拿不到就降级报错。`objc2` 的类型系统也替我们守着这条（`sharedApplication`、`alloc` 都要标记）。
/// - 没有 `NSApplicationMain`，事件得自己拉：`sync` 每帧调 `pump`。CFRunLoop 泵不出 AppKit 的
///   事件（实测），`NSApp.run()` 会阻塞到退出，塞不进「画帧 + 读键盘」。
/// - 状态文件存的是**左上原点**的中心点（与 Windows / 旧数据同一口径），进出 AppKit 按主屏高换算。
#[cfg(target_os = "macos")]
mod imp {
    use std::sync::mpsc::Receiver;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont,
        NSFontAttributeName, NSForegroundColorAttributeName, NSEventMask, NSPanel, NSScreen,
        NSStrokeColorAttributeName, NSStrokeWidthAttributeName, NSTextField, NSView,
        NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_foundation::{
        NSAttributedString, NSDate, NSDefaultRunLoopMode, NSDictionary, NSNumber, NSPoint, NSRect,
        NSSize, NSString,
    };

    use crate::config::Rgb;

    use super::{DesktopLyricsConfig, Payload};

    /// 次要行（译文 / 下一句）与正文的字号比：与 Windows 的 `small_size` 同一比例。
    const SMALL_RATIO: f64 = 3.0 / 5.0;
    /// 文字四周的留白（Windows 是 8 逻辑像素 × DPI；macOS 直接用点）。
    const PAD: f64 = 8.0;
    /// 描边粗细：**负值 = 填充 + 描边一次画完**（Windows 是八方向各画一遍再合成）。
    /// 观感常数，真机对着 Windows 调。
    const OUTLINE_MAIN: f64 = -3.0;
    const OUTLINE_SMALL: f64 = -2.0;
    /// 浮层高度：桌面之上的浮动层（`CGWindowList` 里能看到 layer=3）。
    const LEVEL: isize = 3;

    pub(super) type Started = (Imp, Option<Receiver<(i32, i32)>>, Option<String>);

    pub(super) struct Imp {
        /// `None` = 没有窗口（关掉功能 / 建不出来 / `App::new` 与测试）。
        inner: Option<Inner>,
    }

    struct Inner {
        panel: Retained<NSPanel>,
        /// 三行文本控件：当前句 / 译文 / 下一句。
        current: Retained<NSTextField>,
        translation: Retained<NSTextField>,
        next: Retained<NSTextField>,
        /// 配置在进程里是固定的，建窗口时拿一份（与 Windows 的 `Shared` 同一个理由）。
        config: DesktopLyricsConfig,
        /// 主屏高：左上原点 ↔ AppKit 左下原点换算用。
        screen_height: f64,
        /// 现在是 `orderOut` 状态吗（避免每帧重复调）。
        hidden: bool,
    }

    impl Imp {
        pub(super) fn none() -> Self {
            Self { inner: None }
        }

        /// 把事件队列拉空并派发。每帧都跑，哪怕 payload 没变。
        pub(super) fn pump(&mut self) {
            let Some(inner) = &mut self.inner else {
                return;
            };
            inner.pump();
        }

        pub(super) fn push(&mut self, payload: Payload) {
            let Some(inner) = &mut self.inner else {
                return;
            };
            inner.push(&payload);
        }
    }

    impl Inner {
        fn pump(&mut self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let app = NSApplication::sharedApplication(mtm);
            loop {
                // SAFETY: 主线程；`distantPast` = 队列空就立刻返回，不阻塞界面。
                let event = unsafe {
                    app.nextEventMatchingMask_untilDate_inMode_dequeue(
                        NSEventMask::Any,
                        Some(&NSDate::distantPast()),
                        NSDefaultRunLoopMode,
                        true,
                    )
                };
                let Some(event) = event else { break };
                app.sendEvent(&event);
            }
        }

        /// 一帧 payload → 窗口。内容没变化时 `State::advance` 根本不会调到这里。
        fn push(&mut self, payload: &Payload) {
            // 隐藏：`orderOut` 而不是销毁（位置与锁定态留着），下次 `orderFrontRegardless` 回来。
            if !payload.visible {
                for field in [&self.current, &self.translation, &self.next] {
                    field.setHidden(true);
                }
                if !self.hidden {
                    self.panel.orderOut(None);
                    self.hidden = true;
                }
                return;
            }

            let main_size = f64::from(self.config.font_size);
            let small_size = (main_size * SMALL_RATIO).max(8.0);
            let (color, next_color, outline) = (self.config.color, self.config.next_color, self.config.outline);
            set_line(&self.current, &payload.current, main_size, color, outline, &self.config, OUTLINE_MAIN);
            set_line(
                &self.translation,
                payload.translation.as_deref().unwrap_or(""),
                small_size,
                next_color,
                outline,
                &self.config,
                OUTLINE_SMALL,
            );
            set_line(&self.next, &payload.next, small_size, next_color, outline, &self.config, OUTLINE_SMALL);

            // 量尺寸：宽度取最宽一行、高度是各行之和，再加内边距（与 Windows 同一算法）。
            let rows: Vec<&Retained<NSTextField>> = [&self.current, &self.translation, &self.next]
                .into_iter()
                .filter(|field| !field.isHidden())
                .collect();
            let width = rows.iter().map(|field| field.frame().size.width).fold(0.0_f64, f64::max) + PAD * 2.0;
            let height = rows.iter().map(|field| field.frame().size.height).sum::<f64>() + PAD * 2.0;

            // 位置以**窗口自己**为准：解锁态的拖动是系统直接改窗口，缓存里的中心点会是旧的。
            // 每次重画都重新采纳一次——拖动途中换行不会把窗口拽回旧位置，也不用指望「拖动结束」事件。
            let frame = self.panel.frame();
            let center = (
                (frame.origin.x + frame.size.width / 2.0).round() as i32,
                (self.screen_height - (frame.origin.y + frame.size.height / 2.0)).round() as i32,
            );
            self.panel.setAlphaValue(f64::from(self.config.opacity) / 100.0);
            self.panel.setIgnoresMouseEvents(!payload.locked);
            self.panel.setFrame_display(
                NSRect::new(
                    NSPoint::new(
                        f64::from(center.0) - width / 2.0,
                        self.screen_height - f64::from(center.1) - height / 2.0,
                    ),
                    NSSize::new(width, height),
                ),
                true,
            );

            // 每行水平居中，从上往下摆（AppKit 是左下原点：y 从顶边往下减）。
            let mut y = height - PAD;
            for field in &rows {
                let size = field.frame().size;
                y -= size.height;
                field.setFrame(NSRect::new(
                    NSPoint::new(PAD + (width - PAD * 2.0 - size.width) / 2.0, y),
                    size,
                ));
            }
            // 从 `orderOut` 回来（第一次显示也走这里）；`orderFrontRegardless` 不激活自己。
            self.panel.orderFrontRegardless();
            self.hidden = false;
        }
    }

    /// 还没有真 payload 时先进去的那一帧：窗口建出来先藏着。
    fn hidden_payload() -> Payload {
        Payload { visible: false, current: String::new(), next: String::new(), translation: None, locked: true }
    }

    /// `Retained<T>` **不是** `repr(transparent)`：`&retained` 不是对象指针（直接当 `AnyObject`
    /// 用会段错误，探针里踩过），必须 `&*retained` 再转。
    unsafe fn any<T>(object: &T) -> &AnyObject {
        unsafe { &*(object as *const T as *const AnyObject) }
    }

    /// `Rgb`（0-255）→ `NSColor`。
    fn ns_color(color: Rgb) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(color.r) / 255.0,
            f64::from(color.g) / 255.0,
            f64::from(color.b) / 255.0,
            1.0,
        )
    }

    /// 配置里的字体面认不出来（macOS 上默认那串就是 Windows 字体名）就回退系统字体。
    fn font_for(name: &str, size: f64) -> Retained<NSFont> {
        NSFont::fontWithName_size(&NSString::from_str(name), size)
            .unwrap_or_else(|| NSFont::systemFontOfSize(size))
    }

    /// 给一行设文本与样式。空文本 = 这行不画（`setHidden`），也就不参与量尺寸。
    fn set_line(
        field: &NSTextField,
        text: &str,
        size: f64,
        color: Rgb,
        outline: Rgb,
        config: &DesktopLyricsConfig,
        stroke_width: f64,
    ) {
        field.setHidden(text.is_empty());
        if text.is_empty() {
            return;
        }
        let font = font_for(&config.font, size);
        let text_color = ns_color(color);
        let outline_color = ns_color(outline);
        let stroke = NSNumber::numberWithDouble(stroke_width);
        // SAFETY: 属性名是 AppKit 导出的常量，值的类型与属性名要求的一致。
        let attributes = NSDictionary::from_slices(
            &[
                unsafe { NSFontAttributeName },
                unsafe { NSForegroundColorAttributeName },
                unsafe { NSStrokeColorAttributeName },
                unsafe { NSStrokeWidthAttributeName },
            ],
            &[
                unsafe { any(&*font) },
                unsafe { any(&*text_color) },
                unsafe { any(&*outline_color) },
                unsafe { any(&*stroke) },
            ],
        );
        // SAFETY: 属性键都是 AppKit 的常量，字典是我们刚建的。
        let attributed = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attributes),
            )
        };
        field.setAttributedStringValue(&attributed);
        field.sizeToFit();
    }

    /// 建那个浮层窗口：无边框、透明、不抢焦点、不进 Dock、各 space 都在场。
    fn make_panel(mtm: MainThreadMarker) -> Retained<NSPanel> {
        // SAFETY: 主线程；样式掩码与 backing 都是常量，先给个 1×1 的框，位置随后由 `push` 摆。
        let panel = unsafe {
            NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
                NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        panel.setTitle(&NSString::from_str("canto lyrics"));
        panel.setLevel(LEVEL);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setOpaque(false);
        // SAFETY: `clearColor` 是常驻单例。
        unsafe { panel.setBackgroundColor(Some(&NSColor::clearColor())) };
        panel.setHasShadow(false);
        // 启动一律锁定：点不着（锁定态刻意不持久化）。
        panel.setIgnoresMouseEvents(true);
        // 解锁后拖动交给系统自己拖（见 Task 3）。
        panel.setMovableByWindowBackground(true);
        panel
    }

    /// 一行文本控件：透明、只读、不可选。
    fn make_label(mtm: MainThreadMarker) -> Retained<NSTextField> {
        let field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        field.setBezeled(false);
        field.setDrawsBackground(false);
        field.setEditable(false);
        field.setSelectable(false);
        field.setHidden(true);
        field
    }

    pub(super) fn install(config: &DesktopLyricsConfig, center: (i32, i32)) -> Started {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("桌面歌词要在主线程建窗口");
            return (Imp::none(), None, Some("歌词窗口起不来".to_string()));
        };
        // 附属进程：不进 Dock、不抢别人的焦点（返回 `false` = 这次调用没生效）。
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: 主线程、启动早期。
        if !unsafe { app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) } {
            tracing::warn!("设置 Accessory 激活策略失败");
            return (Imp::none(), None, Some("歌词窗口起不来".to_string()));
        }
        let screen_height = NSScreen::mainScreen(mtm).map_or(0.0, |screen| screen.frame().size.height);

        let panel = make_panel(mtm);
        let (current, translation, next) = (make_label(mtm), make_label(mtm), make_label(mtm));
        // SAFETY: 主线程建视图，内容视图装三行 label。
        let view = unsafe {
            NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
            )
        };
        view.addSubview(&current);
        view.addSubview(&translation);
        view.addSubview(&next);
        panel.setContentView(Some(&*view));

        let mut inner = Inner {
            panel,
            current,
            translation,
            next,
            config: config.clone(),
            screen_height,
            hidden: false,
        };
        // 先把窗口摆到记下来的中心点（`push` 之后会按窗口自己的矩形重算，这里只是给个起点）。
        inner.panel.setFrame_display(
            NSRect::new(
                NSPoint::new(
                    f64::from(center.0) - 50.0,
                    screen_height - f64::from(center.1) - 20.0,
                ),
                NSSize::new(100.0, 40.0),
            ),
            false,
        );
        // 第一帧就是「隐藏」：窗口建出来是空的，等第一次真 payload。
        inner.push(&hidden_payload());
        // 安装记录在这儿打（只有真建了窗口的平台才该说这句）。
        tracing::info!(font = %config.font, size = config.font_size, "桌面歌词窗口已就绪");
        (Imp { inner: Some(inner) }, None, None)
    }
}
```

- [ ] **Step 3: 编译与门禁**

Run: `cargo build --locked && cargo test --locked && cargo clippy --all-targets --locked -- -D warnings`
Expected: 全过。注意 `Cargo.lock` **不应该**变化（三件都已在图里）——`git diff --stat Cargo.lock` 要是空的；若变了，说明版本没对齐（`objc2 0.6.x` / `objc2-*-0.3.x`，照 `Cargo.lock` 里现有的写）。

- [ ] **Step 4: 真机验收（能看见、能更新、能隐藏）**

准备工作：`cargo build`（debug），然后在 tuios 里起真二进制（窗口按任务命名）：

```sh
tuios new-window -s canto-dev canto-lyrics-mac --cwd /Users/kiry/code/canto --no-focus -- target/debug/canto --log debug
tuios wait-for window-output -s canto-dev -w canto-lyrics-mac --pattern '我的收藏' --timeout 20000
```

逐条走：

| # | 操作 | 期望 |
|---|---|---|
| 1 | 界面里选中一首有歌词的歌回车播放（等 2–3 秒） | 桌面底部居中浮出三行：当前句（大、白、带黑描边）、译文、下一句（小、灰） |
| 2 | `screencapture -x /tmp/lyrics-mac.png` 然后看图 | 中文没变成方框、描边干净、三行水平居中、位置在屏幕下方居中；整层略透明（`opacity = 90`） |
| 3 | 在 TUI 里按 `Space` 暂停 | `hide_when_paused` 默认 `false` → 窗口留着（这一步确认默认行为）；把配置改成 `true` 重启后再暂停 → 窗口消失，继续播放又回来 |
| 4 | 看 `~/.config/canto/canto.log` | 有 `桌面歌词窗口已就绪`（带 font/size），没有 `建歌词窗口失败` |
| 5 | 在 TUI 里按 `q` 退出 | 窗口立刻消失（进程退出）；`pgrep -f target/debug/canto` 为空 |
| 6 | 把配置改成 `enabled = false` 再起一次 | 界面上没有任何浮层；进程里也不该有那个窗口 |
| 7 | 检查窗口属性（可选，探针） | `/tmp/lyricsprobe windows <canto 的 pid>` 里能看到一条 `layer=3`、`alpha≈0.90`、bounds 与看到的框一致 |

> 探针是设计阶段那两个一次性程序的合体（`/tmp/lyricsprobe`，spec §8 有记录）。若它已经不在了，
> 第 7 条可以跳过——第 1/2 条的可视证据才是关键的。

- [ ] **Step 5: 提交**

```bash
git add Cargo.toml src/overlay.rs
git commit -m "feat(overlay): macOS 覆盖层(无边框 NSPanel + 透明 label + 主线程每帧泵 AppKit)"
```

---

# Task 3: 拖动与位置回传

**Files:**
- Modify: `src/overlay.rs`（macOS `mod imp`：`Inner` 加两个字段、`install` 建通道并返回接收端、`pump` 读窗口矩形回传）

**Interfaces:**
- Consumes: Task 2 的 `Inner { panel, screen_height, .. }`、`Imp::pump(&mut self)` 每帧被 `sync` 调用
- Produces: `install` 的第二个返回值从 `None` 变成 `Some(Receiver<(i32, i32)>)`；`desktop-lyrics.json` 由 `tui/state.rs` 既有的循环落盘（那段代码不改）
- 依赖的既有行为：`tui/state.rs` 每 tick 排空 `positions` 通道、`overlay::save_center` 写文件、`overlay::load_center` 启动时读回并夹取

- [ ] **Step 1: `Inner` 加字段（顺带把 import 补上）**

`use std::sync::mpsc::Receiver;` → `use std::sync::mpsc::{Receiver, Sender, channel};`（`Sender` 与 `channel` 到这一步才用得上；Task 2 只引进 `Receiver` 就是为了不在 `-D warnings` 下报 unused import）。

```rust
    struct Inner {
        panel: Retained<NSPanel>,
        current: Retained<NSTextField>,
        translation: Retained<NSTextField>,
        next: Retained<NSTextField>,
        config: DesktopLyricsConfig,
        /// 拖动结束后的新中心点回传给主线程（落盘在 `tui/state.rs`）。
        positions: Sender<(i32, i32)>,
        /// 上次报出去的中心点（左上原点）：变了才回传（与 Windows 逐 draw 采纳窗口自身矩形同一口径）。
        reported: (i32, i32),
        screen_height: f64,
        hidden: bool,
    }
```

- [ ] **Step 2: `install` 建通道**

`install` 里，`let panel = make_panel(mtm);` 之前加：

```rust
        let (positions, places) = channel::<(i32, i32)>();
```

`Inner { ... }` 里补两个字段（`reported` 初值就是记下来的中心点）：

```rust
            positions,
            reported: center,
```

最后一行返回值改成把它交出去：

```rust
        (Imp { inner: Some(inner) }, Some(places), None)
```

- [ ] **Step 3: `pump` 里读窗口矩形并回传**

`fn pump(&mut self)` 的循环之后（同函数内）追加：

```rust
            // 拖动：解锁态窗口被系统拖走，位置以**窗口自己**为准。每帧读一次真实矩形，
            // 跟上次报过的比——变了才回传（不指望「拖动结束」事件：Windows 那边踩过
            // `WM_EXITSIZEMOVE` 进不了消息队列的坑，改成逐 draw 采纳）。
            let frame = self.panel.frame();
            let center = (
                (frame.origin.x + frame.size.width / 2.0).round() as i32,
                (self.screen_height - (frame.origin.y + frame.size.height / 2.0)).round() as i32,
            );
            if center != self.reported {
                self.reported = center;
                // 通道那头没了（主线程收摊）就算了，不影响界面。
                let _ = self.positions.send(center);
            }
```

- [ ] **Step 4: 门禁**

Run: `cargo build --locked && cargo test --locked && cargo clippy --all-targets --locked -- -D warnings`
Expected: 全过。

- [ ] **Step 5: 真机验收（解锁 → 拖 → 落盘 → 重启回原位）**

窗口还开着（Task 2 那个 `canto-lyrics-mac` 窗口；没有就按 Task 2 Step 4 重开，并让它播一首有歌词的歌）：

```sh
# 1. 解锁：往系统里合成 ctrl+shift+l（37 = l，与 canto 的 [hotkeys] 默认组合一致）
osascript -e 'tell application "System Events" to key code 37 using {control down, shift down}'
# 2. 看一眼解锁成败：无歌词/没内容时窗口会显示「拖动我（再按一次 ctrl+shift+l 锁定）」
screencapture -x /tmp/lyrics-unlocked.png
```

| # | 操作 | 期望 |
|---|---|---|
| 1 | 注入 `ctrl+shift+l` 后用探针的 `windows <pid>` 拿窗口 bounds（左上原点） | 拿到中心点坐标 |
| 2 | 用探针合成拖动：`/tmp/lyricsprobe drag <cx> <cy> 150 -80 12`（往右上拖） | 窗口跟手移动，canto 的前台状态不变（`osascript -e 'tell application "System Events" to get name of first process whose frontmost is true'` 还是原来的 app） |
| 3 | 读 `~/.config/canto/desktop-lyrics.json` | `cx`/`cy` 约等于拖动后的中心点（差值应在个位数内） |
| 4 | 再注入一次 `ctrl+shift+l`（锁回去），然后退出 canto 重新起 | 歌词窗口出现在**拖到的地方**，不是原来的底部居中 |
| 5 | **（关键）** 拖动时 payload 不变（同一句歌词） | 拖动全程跟手 = 证明 `pump` 每帧都跑；如果第 2 步窗口纹丝不动而第 4 步的位置却变了，说明泵漏了帧（回到 Task 1 Step 1） |
| 6 | 整层透明与描边在真实桌面背景上看一眼（拖到浅色/深色壁纸上） | 黑描边保证任何背景上都能看清（与 Windows 的八方向描边同一目的） |

- [ ] **Step 6: 提交**

```bash
git add src/overlay.rs
git commit -m "feat(overlay): macOS 拖动与位置回传(解锁后交给系统拖，中心点落盘)"
```

---

# Task 4: 文档同步与整轮收尾

**Files:**
- Modify: `README.md`（功能表、平台说明、配置注释、已知限制）
- Modify: `docs/design.md`（「桌面歌词（Windows，实测 2026-10-12）」一节 + 平台总表那行 + 已知限制）
- Modify: `docs/superpowers/specs/2026-10-12-desktop-lyrics-design.md`（两处已过期的「其它平台是空实现」）
- Modify: `docs/superpowers/specs/2026-10-13-desktop-lyrics-macos-design.md`（§7 验收结果；实测与预期不符的地方就改）

**Interfaces:**
- Consumes: Task 2/3 已合并的行为
- Produces: 文档与实现一致；spec §7 的清单有实测结论

- [ ] **Step 1: README**

- 功能表那行：`桌面歌词（Windows，默认开）` → `桌面歌词（Windows / macOS，默认开）`。
- 平台那节：`macOS / Linux / Windows 三平台都能跑（**全局快捷键只在 Windows / macOS 生效**…）`
  → 补一句桌面歌词的平台范围（Windows / macOS）。
- 配置段那段的注释：`# 桌面歌词（Windows）：…` → 去掉「（Windows）」，改成三平台通用的说法；
  并加一句「默认字体面是 Windows 的中文字体名，macOS 上认不出来会自动回退系统字体，想指定就写
  macOS 上有的字体名（如 `PingFang SC`）」。
- 「已知限制」里那条 `**桌面歌词只有 Windows 有**：…` **删掉**，换成 macOS 的已知限制：
  只认主屏（多显示器不做）、解锁态在有歌词时没有额外视觉标记（`UNLOCKED_HINT` 只在没内容时出现）。

- [ ] **Step 2: `docs/design.md`**

- 那一节的标题：`## 桌面歌词（Windows，实测 2026-10-12）` → `## 桌面歌词（Windows / macOS，实测 2026-10-12 / 2026-10-13）`。
- 标题下那段「它分成两半…窗口那一半在这台机器上只能靠探针 + 真机看」补一句：两边的窗口实现各自 `cfg`，
  状态机共用。
- 那个「做法 / 为什么」表：加一列或补几行 macOS 的对照（窗口类型、文字渲染、事件来源、
  拖动、坐标系换算、失败降级），**照 spec 的决策记录写**，别另创说法：
  - 窗口：无边框 `NSPanel`（`NonactivatingPanel`）+ `level = Floating` + `CanJoinAllSpaces`；
  - 文字：透明 `NSTextField` + `NSAttributedString`，描边用负 `strokeWidth` 一次画完；
  - 事件：**主线程每帧手动泵**（`nextEventMatchingMask…dequeue:` + `sendEvent:`）——CFRunLoop
    泵不出 AppKit 事件、`NSApp.run()` 会阻塞（与全局快捷键那节同一个套路）；
  - 拖动：`isMovableByWindowBackground`，每帧读窗口矩形，变了就回传（不指望「拖动结束」事件）；
  - 坐标：状态文件仍然是左上原点，进 AppKit 按主屏高换算。
- 平台总表那行 `桌面歌词 | 非 Windows：不建窗口…` → 改成 `非 Windows：不建窗口（Linux）` +
  `macOS：NSPanel 浮层 + 主线程每帧泵 AppKit（见「桌面歌词」）`。
- 「已知限制」里若还有「桌面歌词只有 Windows」，同样改掉。

- [ ] **Step 3: 两份 spec**

- `2026-10-12-desktop-lyrics-design.md`：§5 模块表里「其它平台：空实现（`none()` + 两个空方法）」
  与 §7 退化表里的「非 Windows：同上（配置段整体忽略…）」都**已经过期**——各加一句
  「macOS 见 `2026-10-13-desktop-lyrics-macos-design.md`；`#[cfg(not(any(windows, macos)))]` 那份才是
  空实现」。历史决策记录不动（那份 spec 明确写了自己只做 Windows）。
- `2026-10-13-desktop-lyrics-macos-design.md`：§7 表格每一行填上实测结论（哪条过、哪条没验）；
  §9 若有条目被实测推翻（比如全屏之上其实不行）就改掉，并把结论写进 `docs/design.md`。

- [ ] **Step 4: 整轮收尾验证**

| # | 检查 | 手段 |
|---|---|---|
| 1 | 三平台门禁 | `cargo test --locked` + `cargo clippy --all-targets --locked -- -D warnings`（本机 macOS）；CI 三个平台 |
| 2 | `enabled = false` 零成本 | 起一次，`pgrep` + 探针 `windows` 里没有那条 `layer=3` 的窗口 |
| 3 | 全屏应用之上 / 切 space | 真机：把某个 app 切全屏，看歌词还在不在；切 space 回来看还在不在（`CanJoinAllSpaces` + `FullScreenAuxiliary`）。不行就把结论写进 spec §9 与 README 已知限制 |
| 4 | 长时间跑的稳定性 | 让它播 10 分钟以上，看 `canto.log` 有没有异常、窗口有没有自己消失/残影、CPU 没有明显抬升（探针里泵 150ms 一次是 0.0%） |
| 5 | 退出清理 | `q` 退出后窗口消失、进程退出、`desktop-lyrics.json` 是最后一个位置 |

- [ ] **Step 5: 提交**

```bash
git add README.md docs/design.md docs/superpowers/specs/
git commit -m "docs: 桌面歌词(macOS 覆盖层)"
```

---

## 自查（写完计划后逐条对 spec）

- **spec §1 接口与改动面** → Task 1（`sync` 每帧泵 + 三份 `pump`）+ Task 2（`install`/`none`/`push`）。
- **spec §2 窗口** → Task 2 Step 2（`make_panel` 的每一项都有）：style mask / level / collectionBehavior /
  opaque+clear+no shadow / ignoresMouseEvents 随锁定 (在 `push` 里) / movableByWindowBackground /
  orderFrontRegardless / Accessory / orderOut / 尺寸随文本 / 坐标换算。
- **spec §3 文字与配色** → Task 2 Step 2（`set_line`/`font_for`/`ns_color`；`SMALL_RATIO`/`PAD`/描边常数）。
- **spec §4 事件泵与拖动** → Task 2（泵）+ Task 3（矩形回传）。
- **spec §5 与 Windows 对齐** → 状态机未动；配置未动；Task 4 Step 3 同步文档。
- **spec §6 退化** → Task 2（主线程标记 / 激活策略 / 建窗口三条失败路径都返回 `Some("歌词窗口起不来")`）。
- **spec §7 验证** → Task 2 Step 4（1/2/5/7 与 3 的一半）+ Task 3 Step 5（3/4/5）+ Task 4 Step 4（3/4/5）。
- **spec §8 探针** → 不再需要新探针；Task 2/3 复用 `/tmp/lyricsprobe` 的 `windows`/`drag`。
- **spec §9 风险** → Task 4 Step 4 第 3/4 条把「全屏之上」与稳定性落到实测。

**类型一致性**：`Imp::none()/push(&mut self, Payload)/pump(&mut self)`、`install(&DesktopLyricsConfig, (i32, i32)) -> Started`、
`Started = (Imp, Option<Receiver<(i32,i32)>>, Option<String>)` 三个任务里写法一致；`Inner` 的字段在 Task 3 Step 1
里加的两个（`positions`/`reported`）在 Task 2 的代码里**没有**，只在 Task 3 出现——按任务顺序会先编译不过，
这是有意的：Task 3 Step 1/2/3 一起才是那一步的完整改动，别只做一半就提交。
