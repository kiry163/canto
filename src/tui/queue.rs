//! 播放队列与播放模式——纯逻辑，不碰 UI、网络与音频设备。
//!
//! 「一首播完往哪走」与「用户按 `n`/`b` 往哪走」是**两件事**：单曲循环时按 `n`
//! 仍然要前进（那是显式意图），而播完要重播当前这首。所以这里分成
//! `advance` / `next_manual` / `prev_manual` 三个入口，各自有单测钉住。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::Song;

/// 播放模式。`m` 按这个顺序循环。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Sequential,
    LoopAll,
    RepeatOne,
    Shuffle,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Sequential => Mode::LoopAll,
            Mode::LoopAll => Mode::RepeatOne,
            Mode::RepeatOne => Mode::Shuffle,
            Mode::Shuffle => Mode::Sequential,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Sequential => "顺序",
            Mode::LoopAll => "列表循环",
            Mode::RepeatOne => "单曲",
            Mode::Shuffle => "随机",
        }
    }
}

/// 「接下来往哪走」。`Play(index)` 是队列下标，不是洗牌序里的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Play(usize),
    Stop,
}

#[derive(Debug, Clone)]
pub(super) struct Queue {
    items: Vec<Song>,
    index: usize,
    origin: String,
    mode: Mode,
    /// 随机模式下这一轮的顺序（`0..len` 的一个排列）。
    shuffle: Vec<usize>,
    /// 游标在 `shuffle` 里的位置。
    shuffle_pos: usize,
}

impl Queue {
    pub fn new(items: Vec<Song>, index: usize, origin: impl Into<String>) -> Self {
        let mut queue = Self {
            index: index.min(items.len().saturating_sub(1)),
            items,
            origin: origin.into(),
            mode: Mode::Sequential,
            shuffle: Vec::new(),
            shuffle_pos: 0,
        };
        queue.reshuffle();
        queue
    }

    pub fn items(&self) -> &[Song] {
        &self.items
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        if mode == Mode::Shuffle {
            // 进随机模式就重洗一轮，免得沿用上一轮的顺序。
            self.reshuffle();
        }
    }

    pub fn cycle_mode(&mut self) -> Mode {
        self.set_mode(self.mode.next());
        self.mode
    }

    pub fn push_back(&mut self, song: Song) {
        let at = self.items.len();
        self.items.push(song);
        // 新项排在整轮的最后，不抢当前这首的位置。
        self.shuffle.push(at);
    }

    pub fn insert_next(&mut self, song: Song) {
        let at = (self.index + 1).min(self.items.len());
        self.items.insert(at, song);
        for slot in &mut self.shuffle {
            if *slot >= at {
                *slot += 1;
            }
        }
        let position = (self.shuffle_pos + 1).min(self.shuffle.len());
        self.shuffle.insert(position, at);
    }

    /// 删第 `at` 项。`Some(step)` = 这次删的是当前项，调用方按 `step` 继续播。
    pub fn remove(&mut self, at: usize) -> Option<Step> {
        if self.is_empty() {
            return Some(Step::Stop);
        }
        if at >= self.items.len() {
            return None;
        }
        self.items.remove(at);
        for slot in &mut self.shuffle {
            if *slot > at {
                *slot -= 1;
            }
        }
        if let Some(position) = self.shuffle.iter().position(|slot| *slot == at) {
            self.shuffle.remove(position);
            if position < self.shuffle_pos {
                self.shuffle_pos = self.shuffle_pos.saturating_sub(1);
            }
        }
        if at < self.index {
            self.index -= 1;
            return None;
        }
        if at > self.index {
            return None;
        }
        // 删的是当前项：下标不动，已经指向原来的下一首。
        if self.index < self.items.len() {
            return Some(Step::Play(self.index));
        }
        self.index = self.items.len().saturating_sub(1);
        Some(self.recover_after_end())
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.index = 0;
        self.shuffle.clear();
        self.shuffle_pos = 0;
    }

    /// 浮层里 `Enter` 跳播。
    pub fn jump(&mut self, at: usize) {
        if at < self.items.len() {
            self.index = at;
            self.sync_shuffle_pos();
        }
    }

    /// 一首播完往哪走。
    pub fn advance(&mut self) -> Step {
        if self.is_empty() {
            return Step::Stop;
        }
        match self.mode {
            Mode::Sequential => {
                if self.index + 1 < self.items.len() {
                    self.index += 1;
                    Step::Play(self.index)
                } else {
                    Step::Stop
                }
            }
            Mode::LoopAll => {
                self.index = (self.index + 1) % self.items.len();
                Step::Play(self.index)
            }
            Mode::RepeatOne => Step::Play(self.index),
            Mode::Shuffle => {
                if self.shuffle_pos + 1 < self.shuffle.len() {
                    self.shuffle_pos += 1;
                } else {
                    // 一轮播完重洗：否则「随机」会在固定的几首之间来回跳。
                    self.reshuffle();
                    // 新的一轮从表头走，但别让当前这首又排第一——听感上像没换歌。
                    if self.shuffle.len() > 1 && self.shuffle[0] == self.index {
                        self.shuffle.swap(0, 1);
                    }
                    self.shuffle_pos = 0;
                }
                self.index = self.shuffle[self.shuffle_pos];
                Step::Play(self.index)
            }
        }
    }

    /// 显式 `n`：单曲循环也前进，因为按 `n` 就是「我要换一首」。
    pub fn next_manual(&mut self) -> Step {
        match self.mode {
            Mode::RepeatOne | Mode::Sequential => {
                if self.index + 1 < self.items.len() {
                    self.index += 1;
                    Step::Play(self.index)
                } else {
                    Step::Stop
                }
            }
            Mode::LoopAll | Mode::Shuffle => self.advance(),
        }
    }

    /// 显式 `b`：沿「来时的那条顺序」后退。
    pub fn prev_manual(&mut self) -> Step {
        if self.is_empty() {
            return Step::Stop;
        }
        if self.mode == Mode::Shuffle {
            if self.shuffle_pos == 0 {
                return Step::Stop;
            }
            self.shuffle_pos -= 1;
            self.index = self.shuffle[self.shuffle_pos];
            return Step::Play(self.index);
        }
        if self.index == 0 {
            if self.mode == Mode::LoopAll {
                self.index = self.items.len() - 1;
                return Step::Play(self.index);
            }
            return Step::Stop;
        }
        self.index -= 1;
        Step::Play(self.index)
    }

    /// 洗牌用的小 PRNG（xorshift64*）：只为打乱顺序，跟安全性无关。
    fn reshuffle(&mut self) {
        self.reshuffle_seeded(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15),
        );
    }

    fn reshuffle_seeded(&mut self, seed: u64) {
        let len = self.items.len();
        self.shuffle = (0..len).collect();
        let mut state = seed | 1;
        for position in (1..len).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let swap = (state % (position as u64 + 1)) as usize;
            self.shuffle.swap(position, swap);
        }
        // 新表和旧表没关系，游标必须重新对齐——否则一轮里会有歌播两遍、有歌被跳过。
        self.sync_shuffle_pos();
    }

    /// 让游标指回当前项在洗牌表里的位置（`jump` / 新建之后都要对齐）。
    fn sync_shuffle_pos(&mut self) {
        self.shuffle_pos = self
            .shuffle
            .iter()
            .position(|slot| *slot == self.index)
            .unwrap_or(0);
    }

    /// 已经走到队尾（或当前项后面没东西了）之后按模式恢复。
    fn recover_after_end(&mut self) -> Step {
        if self.is_empty() {
            return Step::Stop;
        }
        match self.mode {
            Mode::LoopAll => {
                self.index = 0;
                Step::Play(0)
            }
            Mode::Shuffle => {
                self.reshuffle();
                self.shuffle_pos = 0;
                self.index = self.shuffle[0];
                Step::Play(self.index)
            }
            Mode::Sequential | Mode::RepeatOne => Step::Stop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileSizes, Song};

    fn song(mid: &str) -> Song {
        Song {
            mid: mid.to_string(),
            song_id: None,
            media_mid: None,
            title: format!("歌 {mid}"),
            singers: "歌手".to_string(),
            album_mid: String::new(),
            album_name: String::new(),
            duration_secs: 200,
            sizes: FileSizes::default(),
        }
    }

    fn queue_of(count: usize, mode: Mode) -> Queue {
        let items = (0..count).map(|index| song(&format!("m{index}"))).collect();
        let mut queue = Queue::new(items, 0, "嗯");
        queue.set_mode(mode);
        queue
    }

    #[test]
    fn sequential_stops_at_the_end() {
        let mut queue = queue_of(3, Mode::Sequential);
        assert_eq!(queue.advance(), Step::Play(1));
        assert_eq!(queue.advance(), Step::Play(2));
        assert_eq!(queue.advance(), Step::Stop, "队尾要停，不回头");
    }

    #[test]
    fn loop_all_wraps_around() {
        let mut queue = queue_of(2, Mode::LoopAll);
        assert_eq!(queue.advance(), Step::Play(1));
        assert_eq!(queue.advance(), Step::Play(0));
    }

    #[test]
    fn repeat_one_replays_but_n_still_moves_on() {
        let mut queue = queue_of(3, Mode::RepeatOne);
        assert_eq!(queue.advance(), Step::Play(0), "播完重播当前");
        assert_eq!(queue.next_manual(), Step::Play(1), "n 是显式意图");
        assert_eq!(queue.advance(), Step::Play(1));
    }

    #[test]
    fn shuffle_walks_every_item_once_per_round() {
        let mut queue = queue_of(5, Mode::Shuffle);
        queue.reshuffle_seeded(42);
        // 站到这一轮的起点上（表头）。当前项恰好排在表尾时，第一次 advance 会直接
        // 跨过轮边界重洗——那是另一回事，这里要看的是「一轮之内的覆盖」。
        let start = queue.shuffle[0];
        queue.jump(start);
        let mut seen = vec![start];
        for _ in 1..5 {
            let Step::Play(index) = queue.advance() else {
                panic!("一轮之内不该停");
            };
            seen.push(index);
        }
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2, 3, 4], "一轮里每首恰好一次");

        // 再走一步就是新的一轮：重洗后接着放，不停下，也不会把刚放过的这首又排第一。
        let just_played = queue.index();
        let Step::Play(next) = queue.advance() else {
            panic!("一轮播完重洗，不该停");
        };
        assert_ne!(next, just_played, "新一轮的第一首不该又是刚放过的这首");
    }

    #[test]
    fn prev_walks_back_in_the_same_order_it_came() {
        let mut sequential = queue_of(3, Mode::Sequential);
        assert_eq!(sequential.prev_manual(), Step::Stop, "第一首之前没有上一首");
        sequential.jump(2);
        assert_eq!(sequential.prev_manual(), Step::Play(1));

        let mut looping = queue_of(3, Mode::LoopAll);
        assert_eq!(
            looping.prev_manual(),
            Step::Play(2),
            "列表循环里 b 从第一首回到最后一首"
        );

        let mut shuffled = queue_of(4, Mode::Shuffle);
        shuffled.reshuffle_seeded(7);
        let first = shuffled.index();
        let Step::Play(second) = shuffled.advance() else {
            panic!("随机模式应前进");
        };
        assert_eq!(shuffled.index(), second);
        assert_eq!(shuffled.prev_manual(), Step::Play(first), "b 沿洗牌序后退");
    }

    #[test]
    fn appending_and_inserting_change_the_queue_not_the_source_list() {
        let mut queue = queue_of(2, Mode::Sequential);
        queue.push_back(song("tail"));
        assert_eq!(queue.items().len(), 3);
        assert_eq!(queue.items()[2].mid, "tail");
        queue.insert_next(song("next"));
        assert_eq!(queue.items()[1].mid, "next", "A 插到当前项后面");
        assert_eq!(queue.index(), 0, "插队不动当前项");
    }

    #[test]
    fn removing_the_current_item_plays_its_successor() {
        let mut queue = queue_of(3, Mode::Sequential);
        queue.jump(1);
        assert_eq!(queue.remove(1), Some(Step::Play(1)), "删当前项 → 播原 index+1");
        assert_eq!(queue.items()[1].mid, "m2");
    }

    #[test]
    fn removing_the_last_item_stops_unless_looping() {
        let mut sequential = queue_of(2, Mode::Sequential);
        sequential.jump(1);
        assert_eq!(sequential.remove(1), Some(Step::Stop));

        let mut looping = queue_of(2, Mode::LoopAll);
        looping.jump(1);
        assert_eq!(looping.remove(1), Some(Step::Play(0)));
    }

    #[test]
    fn removing_a_song_before_the_current_one_keeps_playing_the_same_song() {
        let mut queue = queue_of(3, Mode::Sequential);
        queue.jump(2);
        assert_eq!(queue.remove(0), None, "删的不是当前项，播放不用动");
        assert_eq!(queue.items()[queue.index()].mid, "m2");
        assert_eq!(queue.index(), 1, "下标要跟着前移");
    }

    #[test]
    fn clearing_an_empty_queue_is_a_no_op_and_current_is_none() {
        let mut queue = queue_of(1, Mode::Sequential);
        queue.clear();
        assert!(queue.is_empty());
        assert!(queue.items().is_empty());
        assert_eq!(queue.advance(), Step::Stop);
        assert_eq!(queue.remove(0), Some(Step::Stop), "空队列删除也是停");
    }
}
