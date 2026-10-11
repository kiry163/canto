//! 播放采样的采集通道：把音源包一层，采样原样透传给播放器，顺带把第 0 声道写进
//! 一份无锁环形缓冲，供频谱分析用。
//!
//! # 为什么无锁
//!
//! 音频线程每个采样写一次、界面线程每帧读一次。用互斥锁会让两边互相卡住——界面卡
//! 一下不算事，音频线程卡一下就是爆音。f32 没有原子版本，所以存它的位模式。
//!
//! # 为什么抽稀
//!
//! 窗口按**时间**（约 46ms）定义，不是按点数：192kHz 的母带文件若照收 2048 个点，
//! 窗口只有 10.7ms，低频分辨率从 21Hz/格退到 94Hz/格。源采样率高于基准就抽稀到
//! 基准量级，并且先做一次 N 点滑动平均——不做低通直接抽稀会把 Nyquist 以上的内容
//! 折叠进可闻段，显示上就是高频段一片噪。
//!
//! # 为什么只取第 0 声道
//!
//! 立体声的两个声道交错排列，全塞进一个窗口等于把两条不同的波形混在一起，频谱是乱的
//! （6 声道的 FLAC 同理）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, SampleRate};

use crate::spectrum::WINDOW;

/// 分析的基准采样率。源比它高就抽稀到这个量级。
const BASE_RATE: u32 = 44_100;

/// 环形缓冲里的采样数（= 一个分析窗口）。
const SLOTS: usize = WINDOW;

/// 音频线程写、界面线程读的一小段波形。
#[derive(Debug, Clone)]
pub struct Levels {
    samples: Arc<Vec<AtomicU32>>,
    cursor: Arc<AtomicUsize>,
    /// 抽稀后的有效采样率（换歌时会变：不同来源的文件采样率未必相同）。
    sample_rate: Arc<AtomicU32>,
}

impl Default for Levels {
    fn default() -> Self {
        Self::new()
    }
}

impl Levels {
    pub fn new() -> Self {
        let mut samples = Vec::with_capacity(SLOTS);
        samples.resize_with(SLOTS, || AtomicU32::new(0.0f32.to_bits()));
        Self {
            samples: Arc::new(samples),
            cursor: Arc::new(AtomicUsize::new(0)),
            sample_rate: Arc::new(AtomicU32::new(0)),
        }
    }

    /// 追加一个（抽稀后的、单声道的）采样。
    fn push(&self, sample: f32) {
        let index = self.cursor.fetch_add(1, Ordering::Relaxed) % SLOTS;
        self.samples[index].store(sample.to_bits(), Ordering::Relaxed);
    }

    fn set_sample_rate(&self, sample_rate: u32) {
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
    }

    /// 把整个窗口按时间顺序（最旧 → 最新）拷进 `out`，返回有效采样率。
    ///
    /// 复用调用方的缓冲：逐帧零分配。
    pub fn read_into(&self, out: &mut Vec<f32>) -> u32 {
        let cursor = self.cursor.load(Ordering::Relaxed);
        out.clear();
        out.extend((0..SLOTS).map(|offset| {
            let index = (cursor + offset) % SLOTS;
            f32::from_bits(self.samples[index].load(Ordering::Relaxed))
        }));
        self.sample_rate.load(Ordering::Relaxed)
    }

    /// 清零。切歌时调用，否则会留着上一首的波形。
    pub fn clear(&self) {
        for sample in self.samples.iter() {
            sample.store(0.0f32.to_bits(), Ordering::Relaxed);
        }
        self.cursor.store(0, Ordering::Relaxed);
    }
}

/// 把音源包起来：采样透传，同时把第 0 声道按抽稀倍率记进 [`Levels`]。
pub struct LevelMeter<S> {
    inner: S,
    levels: Levels,
    channels: u16,
    /// 当前轮到第几声道（只有 0 会被记下来）。
    channel: u16,
    /// 抽稀倍率（1 = 原样收）。
    factor: u16,
    sum: f32,
    counted: u16,
}

impl<S> LevelMeter<S>
where
    S: Source,
{
    pub fn new(inner: S, levels: Levels) -> Self {
        let channels = inner.channels().get();
        let rate = inner.sample_rate().get();
        // 基准采样率以下的源原样收（factor = 1，不做平均）
        let factor = (rate / BASE_RATE).clamp(1, u16::MAX as u32) as u16;
        levels.set_sample_rate(rate / u32::from(factor));
        Self {
            inner,
            levels,
            channels: channels.max(1),
            channel: 0,
            factor,
            sum: 0.0,
            counted: 0,
        }
    }

    /// 记一个第 0 声道的采样：按倍率累加，凑满一格才算一个（倍率 1 时即原样）。
    fn record(&mut self, sample: f32) {
        self.sum += sample;
        self.counted += 1;
        if self.counted == self.factor {
            self.levels.push(self.sum / f32::from(self.factor));
            self.sum = 0.0;
            self.counted = 0;
        }
    }
}

impl<S> Iterator for LevelMeter<S>
where
    S: Source,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;

        // 只把第 0 声道写进窗口，保证窗口里是一条连续的波形
        if self.channel == 0 {
            self.record(sample);
        }
        self.channel = (self.channel + 1) % self.channels;

        Some(sample)
    }
}

impl<S> Source for LevelMeter<S>
where
    S: Source,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// 必须转发，否则进度条与方向键跳转会完全失效。
    ///
    /// `Source::try_seek` 的**默认实现直接返回 `NotSupported`**：包装层漏掉它时，
    /// rodio 会认为整条音频不可跳转，而播放本身完全正常——极难察觉是这个包装层
    /// 吞掉的。`levels.rs` 的单测专门钉住这一条。
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只实现必要的转发，用来验证包装层没有把 `Source` 的那几个方法吞掉。
    struct Fake {
        samples: Vec<f32>,
        index: usize,
        rate: u32,
        seeks: Arc<std::sync::Mutex<Vec<Duration>>>,
    }

    impl Iterator for Fake {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            let sample = self.samples.get(self.index).copied();
            self.index += 1;
            sample
        }
    }

    impl Source for Fake {
        fn current_span_len(&self) -> Option<usize> {
            Some(7)
        }

        fn channels(&self) -> ChannelCount {
            ChannelCount::new(2).expect("2 声道")
        }

        fn sample_rate(&self) -> SampleRate {
            SampleRate::new(self.rate).expect("采样率")
        }

        fn total_duration(&self) -> Option<Duration> {
            Some(Duration::from_secs(9))
        }

        fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
            self.seeks.lock().expect("锁").push(position);
            Ok(())
        }
    }

    /// 一个假音源：`rate` 决定抽稀倍率，`samples` 是逐声道的交错采样。
    fn fake(samples: Vec<f32>, rate: u32) -> (Fake, Arc<std::sync::Mutex<Vec<Duration>>>) {
        let seeks = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Fake {
                samples,
                index: 0,
                rate,
                seeks: seeks.clone(),
            },
            seeks,
        )
    }

    #[test]
    fn the_wrapper_forwards_every_source_method() {
        // 这一条是整个设计里最容易踩的坑：`Source::try_seek` 的**默认实现直接返回
        // NotSupported**，包装层漏掉转发，rodio 就认为整条音频不可跳转——进度条与
        // 方向键全部静默失效，而播放本身完全正常。
        let (inner, seeks) = fake(vec![0.0; 8], 44_100);
        let mut meter = LevelMeter::new(inner, Levels::new());

        assert_eq!(meter.current_span_len(), Some(7));
        assert_eq!(meter.channels().get(), 2);
        assert_eq!(meter.sample_rate().get(), 44_100);
        assert_eq!(meter.total_duration(), Some(Duration::from_secs(9)));

        meter
            .try_seek(Duration::from_millis(500))
            .expect("seek 必须被转发到内层");
        assert_eq!(
            seeks.lock().expect("锁").as_slice(),
            [Duration::from_millis(500)]
        );
    }

    #[test]
    fn samples_pass_through_untouched() {
        let (inner, _seeks) = fake(vec![0.25, -0.5, 1.0], 44_100);
        let meter = LevelMeter::new(inner, Levels::new());
        assert_eq!(meter.collect::<Vec<f32>>(), vec![0.25, -0.5, 1.0]);
    }

    #[test]
    fn only_the_first_channel_is_recorded() {
        // 两个声道交错：第 0 声道全是 1.0、第 1 声道全是 0.0
        let samples: Vec<f32> = (0..SLOTS * 2)
            .map(|index| if index % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let levels = Levels::new();
        let (inner, _seeks) = fake(samples, 44_100);
        let mut meter = LevelMeter::new(inner, levels.clone());
        for _ in 0..SLOTS * 2 {
            meter.next().expect("有采样");
        }

        let mut window = Vec::new();
        levels.read_into(&mut window);
        assert!(
            window.iter().all(|&sample| sample == 1.0),
            "窗口里应当是第 0 声道的波形"
        );
    }

    #[test]
    fn a_high_rate_source_is_decimated() {
        // 96kHz 的源抽稀 2 倍：有效采样率变 48kHz，窗口里是两个采样的平均。
        //
        // 采样数要算对：填满窗口需要「窗口 × 声道数 × 抽稀倍率」个采样
        // （立体声每 2 个采样才出一个第 0 声道，再每 2 个才凑成一格）。
        //
        // 源是立体声、每帧两个采样同值，于是**第 0 声道**是 0 与 1 交替
        // （写成「按 index 交替」会让第 0 声道恒为 0——那是两条不同的波形）。
        let total = SLOTS * 2 * 2;
        let samples: Vec<f32> = (0..total)
            .map(|index| if (index / 2) % 2 == 0 { 0.0 } else { 1.0 })
            .collect();
        let levels = Levels::new();
        let (inner, _seeks) = fake(samples, 96_000);
        let mut meter = LevelMeter::new(inner, levels.clone());
        assert_eq!(meter.factor, 2);
        for _ in 0..total {
            meter.next().expect("有采样");
        }

        let mut window = Vec::new();
        let rate = levels.read_into(&mut window);
        assert_eq!(rate, 48_000, "有效采样率该是抽稀后的");
        assert_eq!(window.len(), SLOTS);
        assert!(
            window.iter().all(|&sample| (sample - 0.5).abs() < 1e-6),
            "0 与 1 交替的源抽稀后应当处处是 0.5，实际首个是 {}",
            window[0]
        );
    }

    #[test]
    fn a_base_rate_source_is_not_decimated() {
        let (inner, _seeks) = fake(vec![0.0; 4], 44_100);
        let meter = LevelMeter::new(inner, Levels::new());
        assert_eq!(meter.factor, 1);
        assert_eq!(meter.sample_rate().get(), 44_100);
    }

    #[test]
    fn clear_wipes_the_window() {
        let levels = Levels::new();
        levels.push(1.0);
        levels.clear();
        let mut window = Vec::new();
        levels.read_into(&mut window);
        assert!(window.iter().all(|&sample| sample == 0.0));
    }
}
