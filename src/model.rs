//! canto 自己的数据模型。
//!
//! 只保留列表展示与播放取流要用的字段：接口里其它字段一律不落地，
//! 免得变成"看起来有用、实际没人读"的包袱。

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
    /// 最高可用档位标签，用于列表里一眼看出有没有无损。
    pub fn best_label(&self) -> &'static str {
        if self.hires > 0 {
            "Hi-Res"
        } else if self.flac > 0 {
            "FLAC"
        } else if self.mp3_320 > 0 {
            "320k"
        } else if self.mp3_128 > 0 {
            "128k"
        } else {
            "—"
        }
    }
}

impl Song {
    /// `mm:ss`，列表展示用。
    pub fn duration_display(&self) -> String {
        format!(
            "{:02}:{:02}",
            self.duration_secs / 60,
            self.duration_secs % 60
        )
    }
}

/// 一个歌单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
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
