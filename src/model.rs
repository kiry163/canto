//! canto 自己的数据模型。
//!
//! 只保留列表展示与播放取流要用的字段：接口里其它字段一律不落地，
//! 免得变成"看起来有用、实际没人读"的包袱。

use std::time::Duration;

/// 音质档位。声明顺序就是高低顺序，便于比较（`want >= Quality::Flac`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    Low128,
    High320,
    Flac,
    Hires,
}

impl Quality {
    /// 展示与 `--quality` 用的短名。
    pub fn label(self) -> &'static str {
        match self {
            Quality::Low128 => "128k",
            Quality::High320 => "320k",
            Quality::Flac => "无损",
            Quality::Hires => "Hi-Res",
        }
    }

    /// 列表里那一列用的标签（按文件类型叫：`FLAC` / `320k` / …）。
    ///
    /// 跟 [`Quality::label`] 的展示名（「无损」「Hi-Res」）不是一套：状态行报的是
    /// 音质档位，列表里那一列报的是「这份文件是什么」。
    pub fn file_label(self) -> &'static str {
        match self {
            Quality::Low128 => "128k",
            Quality::High320 => "320k",
            Quality::Flac => "FLAC",
            Quality::Hires => "Hi-Res",
        }
    }

    /// 配置文件里写的**规范名**：`parse()` 认得的一堆写法里挑一个当正牌。
    ///
    /// 写回 `config.toml` 与报错提示都用它，免得「无损」这种展示名又得特判回 `flac`。
    pub fn token(self) -> &'static str {
        match self {
            Quality::Low128 => "128",
            Quality::High320 => "320",
            Quality::Flac => "flac",
            Quality::Hires => "hires",
        }
    }

    /// 四档的说明（配置模板里逐行写清楚：名字、体积量级），顺序从低到高。
    pub fn options() -> [(&'static str, &'static str); 4] {
        [
            ("128", "128k MP3     约 3–5 MB/首       最小"),
            ("320", "320k MP3     约 8–12 MB/首      默认"),
            ("flac", "无损 FLAC    约 25–35 MB/首"),
            ("hires", "Hi-Res       单曲可到 ~150 MB"),
        ]
    }

    /// 解析 `--quality`。接受 `128`/`128k`、`320`/`320k`、`flac`/`lossless`/`无损`、
    /// `hires`/`hi-res`/`母带`。
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "128" | "128k" | "low" => Some(Quality::Low128),
            "320" | "320k" | "high" => Some(Quality::High320),
            "flac" | "lossless" | "无损" => Some(Quality::Flac),
            "hires" | "hi-res" | "hi_res" | "master" | "母带" => Some(Quality::Hires),
            _ => None,
        }
    }
}

/// 一首歌。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    /// 歌曲 mid，接口主键。
    pub mid: String,
    /// 数字 id，取 vkey 要用。
    pub song_id: Option<u64>,
    /// `file.media_mid`，取 vkey 要用。
    pub media_mid: Option<String>,
    pub title: String,
    /// 歌手，多个用「、」连接。
    pub singers: String,
    pub album_mid: String,
    pub album_name: String,
    pub duration_secs: u64,
    /// 各档位文件大小（字节）。0 = 该档位没有。
    pub sizes: FileSizes,
}

/// 各档位文件大小；同时用来判断最高可用音质。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileSizes {
    pub mp3_128: u64,
    pub mp3_320: u64,
    pub flac: u64,
    pub hires: u64,
}

impl FileSizes {
    /// 按这个下限，**实际会去取**哪一档：（档位，接口报的体积）。
    ///
    /// 列表里那一列显示这个，而不是接口报的最高档位——配置成 320 的时候还列着
    /// `FLAC 31MB`，只会让人以为它要下无损。
    ///
    /// 优先挑「真的报了体积」的档位（接口对某些档位不报大小，见 README 已知限制）；
    /// 一档都没报就退回下限本身、体积未知。
    pub fn at_or_below(&self, want: Quality) -> (Quality, Option<u64>) {
        [
            (Quality::Hires, self.hires),
            (Quality::Flac, self.flac),
            (Quality::High320, self.mp3_320),
            (Quality::Low128, self.mp3_128),
        ]
        .into_iter()
        .find(|(quality, size)| *quality <= want && *size > 0)
        .map_or((want, None), |(quality, size)| (quality, Some(size)))
    }
}

/// 一行歌词。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    /// 这一行开始的时间（已应用 LRC 的 `offset`）。
    pub at: Duration,
    pub text: String,
    /// 翻译（按时间戳与正文配对）。
    pub translation: Option<String>,
}

/// 一首歌的歌词。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lyric {
    pub lines: Vec<LyricLine>,
}

impl Lyric {
    /// 写回标准 LRC（时间戳已换算成绝对值，`offset` 也应用过了）。
    ///
    /// 下载落盘与歌词缓存都走这一份——读的时候又是 [`Lyric::parse`]，
    /// 所以「写出去再读回来」正好是个闭环，不需要第二套序列化。
    /// `translation = true` 时只输出翻译行（写 `<mid>.trans.lrc` 用）。
    pub fn to_lrc(&self, translation: bool) -> String {
        let mut out = String::new();
        for line in &self.lines {
            let text = if translation {
                line.translation.as_deref()
            } else {
                Some(line.text.as_str())
            };
            let Some(text) = text else { continue };
            let seconds = line.at.as_secs();
            out.push_str(&format!(
                "[{:02}:{:02}.{:02}]{text}\n",
                seconds / 60,
                seconds % 60,
                line.at.subsec_millis() / 10
            ));
        }
        out
    }
}

impl Lyric {
    /// 解析 LRC（正文 + 可选翻译，都是同一套 `[mm:ss.xx]文本` 格式）。
    ///
    /// 处理这几件实测里会碰到的事：
    /// - `[ti:]`/`[ar:]`/`[al:]`/`[by:]` 这类元信息行丢掉；
    /// - `[offset:±ms]` 会整体平移时间（正值表示歌词要**早**出现）；
    /// - 一行可以挂多个时间戳（`[00:01.00][00:30.00]同样一句`）；
    /// - 翻译按时间戳配对，配不上的丢掉。
    pub fn parse(lrc: &str, translation: Option<&str>) -> Self {
        // 正文与翻译各带自己的 `offset`，所以先用各自的 offset 换成绝对时间再配对，
        // 否则两边的 offset 不一致时会整段错位。
        let translations = translation
            .map(|text| {
                let offset = parse_offset(text);
                parse_lines(text)
                    .into_iter()
                    .map(|(at, text)| (shift(at, offset), text))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let offset = parse_offset(lrc);
        let mut lines = parse_lines(lrc)
            .into_iter()
            .map(|(at, text)| {
                let at = shift(at, offset);
                let translated = translations
                    .iter()
                    .find(|(other, _)| *other == at)
                    .map(|(_, text)| text.clone());
                LyricLine {
                    at,
                    text,
                    translation: translated,
                }
            })
            .collect::<Vec<_>>();
        lines.sort_by_key(|line| line.at);
        Self { lines }
    }

    /// 当前时间应该高亮哪一行：最后一个 `at <= position` 的行。
    ///
    /// 第一行之前返回 `None`（前奏阶段不亮任何行）。
    pub fn line_at(&self, position: Duration) -> Option<usize> {
        let index = self
            .lines
            .partition_point(|line| line.at <= position)
            .checked_sub(1)?;
        Some(index)
    }
}

/// 解析成 `(时间, 文本)` 列表（保留重复时间戳，排序交给调用方）。
fn parse_lines(lrc: &str) -> Vec<(Duration, String)> {
    let mut parsed = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        // 一行可能挂着多个 `[..]`：把所有时间戳摘出来；认不出的标签（`[ti:]`、
        // `[kana:]`）跳过继续扫，别因为它把同一行的真歌词一起丢掉。
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some((tag, after)) = stripped.split_once(']') else {
                break;
            };
            rest = after.trim_start();
            if let Some(at) = parse_timestamp(tag) {
                stamps.push(at);
            }
        }
        let text = rest.trim();
        if text.is_empty() || stamps.is_empty() {
            continue;
        }
        for at in stamps {
            parsed.push((at, text.to_string()));
        }
    }
    parsed
}

/// `mm:ss.xx` / `mm:ss.xxx` / `mm:ss`，也容忍 `hh:mm:ss.xx`。
///
/// `[ti:天黑黑]` 这类元信息会走到最后一步解析失败返回 `None`（"天黑黑" 不是数字），
/// 所以不需要单独维护一份标签名清单。
fn parse_timestamp(tag: &str) -> Option<Duration> {
    let mut parts = tag.split(':').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let last = parts.pop()?;
    let (seconds, milliseconds) = last
        .split_once('.')
        .map_or((last, 0), |(seconds, fraction)| {
            let digits = fraction.chars().take(3).collect::<String>();
            let milliseconds = format!("{digits:0<3}").parse::<u64>().unwrap_or(0);
            (seconds, milliseconds)
        });
    let seconds: u64 = seconds.parse().ok()?;
    let minutes: u64 = parts.pop()?.parse().ok()?;
    let hours: u64 = match parts.pop() {
        Some(hours) => hours.parse().ok()?,
        None => 0,
    };
    Some(Duration::from_millis(
        ((hours * 60 + minutes) * 60 + seconds) * 1000 + milliseconds,
    ))
}

/// LRC 的 `[offset:±毫秒]`：正值让歌词提前出现。
fn parse_offset(lrc: &str) -> i64 {
    for raw in lrc.lines() {
        let tag = raw.trim();
        let Some(inner) = tag
            .strip_prefix("[offset:")
            .and_then(|rest| rest.strip_suffix(']'))
        else {
            continue;
        };
        if let Ok(value) = inner.trim().parse::<i64>() {
            return value;
        }
    }
    0
}

fn shift(at: Duration, offset_ms: i64) -> Duration {
    if offset_ms >= 0 {
        at.saturating_sub(Duration::from_millis(offset_ms as u64))
    } else {
        at + Duration::from_millis(offset_ms.unsigned_abs())
    }
}

/// 一个歌单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    /// 详情接口要的 `disstid`（= 服务端的 `tid`）。
    pub id: String,
    /// 写入接口要的目录号（`dirid`）；「我喜欢」固定 201，未知时 0。
    pub dir_id: u64,
    pub name: String,
    pub song_count: u64,
    pub cover_url: Option<String>,
    /// 「我喜欢」不是普通歌单：读取走另一个接口，界面上也要单独一行。
    pub is_favorites: bool,
}

impl Playlist {
    /// 「我喜欢」的伪 id：命令行里可以直接 `canto tracks favorites`。
    pub const FAVORITES_ID: &'static str = "favorites";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 列表那一列：跟着配置的下限走，标签与体积必须同一档。
    #[test]
    fn at_or_below_picks_the_tier_we_would_actually_fetch() {
        let sizes = FileSizes {
            mp3_128: 4_000_000,
            mp3_320: 9_000_000,
            flac: 30_000_000,
            hires: 150_000_000,
        };
        assert_eq!(
            sizes.at_or_below(Quality::Hires),
            (Quality::Hires, Some(150_000_000))
        );
        assert_eq!(
            sizes.at_or_below(Quality::Flac),
            (Quality::Flac, Some(30_000_000))
        );
        assert_eq!(
            sizes.at_or_below(Quality::High320),
            (Quality::High320, Some(9_000_000)),
            "配置成 320 就不该再显示无损"
        );
        assert_eq!(
            sizes.at_or_below(Quality::Low128),
            (Quality::Low128, Some(4_000_000))
        );
    }

    /// 接口没报的档位不算数（跳过去看更低一档）；全都没报就退回下限、体积未知。
    #[test]
    fn at_or_below_skips_tiers_without_a_size() {
        let partial = FileSizes {
            mp3_128: 4_000_000,
            mp3_320: 0,
            flac: 0,
            hires: 0,
        };
        assert_eq!(
            partial.at_or_below(Quality::Flac),
            (Quality::Low128, Some(4_000_000)),
            "无损没报体积，实际会退到 128k"
        );

        let none = FileSizes::default();
        assert_eq!(none.at_or_below(Quality::High320), (Quality::High320, None));
    }

    /// 实测抓到的开头（2026-10-09，天黑黑）：元信息 + 正文混在一起。
    const REAL_LRC: &str = "[ti:天黑黑]\n\
        [ar:孙燕姿]\n\
        [al:孙燕姿 同名专辑]\n\
        [by:]\n\
        [offset:0]\n\
        [00:00.00]天黑黑 - 孙燕姿\n\
        [00:03.11]词：廖莹如/吴依铮\n\
        [00:06.22]曲：李偲菘\n";

    /// 档位与体积必须出自同一档：标着 FLAC、大小却是 Hi-Res 那份就是错位。
    #[test]
    fn the_reported_tier_and_size_come_from_the_same_tier() {
        let lossless = FileSizes {
            mp3_128: 3_500_000,
            mp3_320: 9_000_000,
            flac: 25_000_000,
            hires: 0,
        };
        assert_eq!(
            lossless.at_or_below(Quality::Hires),
            (Quality::Flac, Some(25_000_000)),
            "要 Hi-Res 但接口没报它的体积 → 退到 FLAC，标签与大小都是 FLAC 那份"
        );

        let hires = FileSizes {
            flac: 25_000_000,
            hires: 150_000_000,
            ..Default::default()
        };
        assert_eq!(
            hires.at_or_below(Quality::Hires),
            (Quality::Hires, Some(150_000_000))
        );

        let low = FileSizes {
            mp3_128: 3_500_000,
            ..Default::default()
        };
        assert_eq!(
            low.at_or_below(Quality::Hires),
            (Quality::Low128, Some(3_500_000))
        );

        let unknown = FileSizes::default();
        assert_eq!(
            unknown.at_or_below(Quality::Hires),
            (Quality::Hires, None),
            "接口一档都没报：退回下限本身，体积未知"
        );
    }

    #[test]
    fn parses_a_real_lrc() {
        let lyric = Lyric::parse(REAL_LRC, None);
        assert_eq!(
            lyric.lines.len(),
            3,
            "元信息行不能进正文：{:?}",
            lyric.lines
        );
        assert_eq!(lyric.lines[0].at, Duration::ZERO);
        assert_eq!(lyric.lines[0].text, "天黑黑 - 孙燕姿");
        assert_eq!(lyric.lines[1].at, Duration::from_millis(3110));
        assert_eq!(lyric.lines[1].text, "词：廖莹如/吴依铮");
        assert_eq!(lyric.lines[2].at, Duration::from_millis(6220));
    }

    #[test]
    fn handles_multiple_timestamps_and_offset() {
        // offset 正值表示歌词要提前出现（500ms）。
        let lyric = Lyric::parse("[offset:500]\n[00:01.00][00:31.50]副歌\n", None);
        assert_eq!(lyric.lines.len(), 2, "一行挂两个时间戳要变成两行");
        assert_eq!(lyric.lines[0].at, Duration::from_millis(500));
        assert_eq!(lyric.lines[1].at, Duration::from_millis(31_000));
        assert_eq!(lyric.lines[0].text, "副歌");

        // 负 offset 往后推。
        let lyric = Lyric::parse("[offset:-250]\n[00:02.00]x\n", None);
        assert_eq!(lyric.lines[0].at, Duration::from_millis(2250));
    }

    #[test]
    fn merges_translation_by_timestamp() {
        let lyric = Lyric::parse(
            "[00:05.00]hello\n[00:09.00]world\n",
            Some("[00:05.00]你好\n[00:07.00]配不上的翻译\n"),
        );
        assert_eq!(lyric.lines[0].translation.as_deref(), Some("你好"));
        assert_eq!(
            lyric.lines[1].translation, None,
            "时间戳配不上的翻译要丢掉，不能错位贴到别的行"
        );
    }

    #[test]
    fn line_at_picks_the_last_line_at_or_before_the_position() {
        let lyric = Lyric::parse("[00:03.00]a\n[00:06.00]b\n[00:09.00]c\n", None);
        assert_eq!(
            lyric.line_at(Duration::from_millis(1000)),
            None,
            "前奏阶段不亮"
        );
        assert_eq!(lyric.line_at(Duration::from_millis(3000)), Some(0));
        assert_eq!(lyric.line_at(Duration::from_millis(5900)), Some(0));
        assert_eq!(lyric.line_at(Duration::from_millis(6000)), Some(1));
        assert_eq!(lyric.line_at(Duration::from_millis(600_000)), Some(2));
    }

    #[test]
    fn translation_uses_its_own_offset() {
        // 正文 offset=500（提前 0.5s），翻译 offset=0：绝对时间对不上就别贴。
        let lyric = Lyric::parse(
            "[offset:500]
[00:01.00]a
",
            Some(
                "[offset:0]
[00:01.00]译错
",
            ),
        );
        assert_eq!(lyric.lines[0].at, Duration::from_millis(500));
        assert_eq!(lyric.lines[0].translation, None);

        // 两边都按自己的 offset 换算后相同 → 贴上。
        let lyric = Lyric::parse(
            "[offset:500]
[00:01.00]a
",
            Some(
                "[offset:500]
[00:01.00]对
",
            ),
        );
        assert_eq!(lyric.lines[0].translation.as_deref(), Some("对"));
    }

    #[test]
    fn tag_lines_like_kana_do_not_become_lyrics() {
        // QQ 的翻译文件里带 `[kana:1し1きょく…]` 这种整行标签，不能当歌词。
        let lyric = Lyric::parse(
            "[kana:1し1きょく1へん][00:01.00]真的词
",
            Some(
                "[kana:1し1きょく]
",
            ),
        );
        assert_eq!(lyric.lines.len(), 1);
        assert_eq!(lyric.lines[0].text, "真的词");
    }

    #[test]
    fn empty_or_tag_only_content_yields_no_lines() {
        assert!(Lyric::parse("", None).lines.is_empty());
        assert!(
            Lyric::parse("[ti:只有标题]\n[ar:歌手]\n", None)
                .lines
                .is_empty()
        );
    }
}
