//! 本地歌单：使用者自己从本地歌曲挑出来组织的列表。
//!
//! 跟「在线歌单」（QQ 那边）完全分开：那份数据在服务端，这份落
//! `~/.config/canto/local_playlists.json`——**使用者自己建的东西，丢了就没了**，所以
//! 它跟历史一样放数据目录，而不是跟曲库索引那样放缓存目录。
//!
//! 条目只存 `mid` + 标题与歌手的**快照**：文件被移走或改名后条目还在（渲染成「（已丢失）」），
//! 不会因为一次扫描就把歌单清空。

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::Song;

/// 落盘格式版本。字段含义变了就 +1（老文件按空处理，不做迁移）。
pub const VERSION: u32 = 2;

/// 固定那张本地歌单的名字。
///
/// **不能改名、不能删**：`p` 的选择框里永远得有个地方可放歌，不然「加进本地歌单」这件事
/// 在没建过任何歌单的人那儿是无路可走的。按名字认它（名字固定 = 不用往文件里加标记、不用
/// 动版本号），所以这个名字是保留的：`create` / `rename` 都不许撞上它。
pub const DEFAULT_NAME: &str = "默认";

/// 存进歌单的一条：`mid`（`local:<路径>`）+ 标题 / 歌手 / 专辑 / 时长的**快照**。
///
/// 快照要够画出一行：文件被删了那句「（已丢失）」照样得写得出专辑与时长（曲库那边查不到了）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSong {
    pub mid: String,
    pub title: String,
    pub singers: String,
    pub album: String,
    /// 秒；0 = 不知道。
    pub duration_secs: u64,
}

/// 一张本地歌单。条目按加入顺序，不支持手动排序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPlaylist {
    pub name: String,
    pub songs: Vec<StoredSong>,
}

/// 名字不合法：空（去空白之后）或与别的歌单重名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    Duplicate,
    /// 撞上固定那张「默认」（它的名字是保留的；改名与删它都不许）。
    Reserved,
}

impl std::fmt::Display for NameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            NameError::Empty => "名字不能是空的",
            NameError::Duplicate => "这个名字已经有过了",
            NameError::Reserved => "「默认」这个名字是留着的",
        })
    }
}

/// `local_playlists.json` 的全部内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    pub playlists: Vec<LocalPlaylist>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: VERSION,
            playlists: Vec::new(),
        }
    }
}

impl Store {
    /// 读回来。文件缺失 / 坏了 / 版本不符一律按**空**处理：歌单是使用者自己建的东西，
    /// 宁可当没有，也不要带着半份数据接着写（下一次 `save` 会把半份变成全份）。
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(store) if store.version == VERSION => store,
            Ok(store) => {
                tracing::warn!(
                    "本地歌单版本是 {}（当前 {VERSION}），按空处理",
                    store.version
                );
                Self::default()
            }
            Err(error) => {
                tracing::warn!("本地歌单读取失败（按空处理）：{error:#}");
                Self::default()
            }
        }
    }

    /// 落盘（临时文件 + rename，中途失败不留坏文件）。
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).context("序列化本地歌单失败")?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text).with_context(|| format!("写 {} 失败", temp.display()))?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("把 {} 换成 {} 失败", temp.display(), path.display()))?;
        Ok(())
    }

    /// 保证有那张固定的「默认」：没有就插到**第一张**（别的歌单一个不动）。
    /// 返回是不是刚补的——调用方拿它决定要不要立刻落盘。老文件（版本 2、没有这张）也照做：
    /// 歌单是使用者自己建的东西，不升版本号、不清空。
    pub fn ensure_default(&mut self) -> bool {
        if self.has_default() {
            return false;
        }
        self.playlists.insert(
            0,
            LocalPlaylist {
                name: DEFAULT_NAME.to_string(),
                songs: Vec::new(),
            },
        );
        true
    }

    /// 有那张「默认」吗（按名字认）。
    pub fn has_default(&self) -> bool {
        self.playlists.iter().any(|playlist| playlist.name == DEFAULT_NAME)
    }

    /// 第 `index` 张是不是固定那张「默认」。
    pub fn is_default(&self, index: usize) -> bool {
        self.playlists
            .get(index)
            .is_some_and(|playlist| playlist.name == DEFAULT_NAME)
    }

    /// 新建一张，返回它在列表里的下标。名字去首尾空白，空 / 重名 / 撞上「默认」都拒绝。
    pub fn create(&mut self, name: &str) -> std::result::Result<usize, NameError> {
        let name = self.check_name(name, None)?;
        self.playlists.push(LocalPlaylist {
            name,
            songs: Vec::new(),
        });
        Ok(self.playlists.len() - 1)
    }

    /// 改名（同样校验；改成自己原来的名字不算重名）。固定那张「默认」不许改名。
    pub fn rename(&mut self, index: usize, name: &str) -> std::result::Result<(), NameError> {
        if self.is_default(index) {
            return Err(NameError::Reserved);
        }
        let name = self.check_name(name, Some(index))?;
        if let Some(playlist) = self.playlists.get_mut(index) {
            playlist.name = name;
        }
        Ok(())
    }

    /// 加一首：同一 `mid` 只留一份，顺序就是加入顺序。
    pub fn add(&mut self, index: usize, song: &Song) {
        let Some(playlist) = self.playlists.get_mut(index) else {
            return;
        };
        if playlist.songs.iter().any(|stored| stored.mid == song.mid) {
            return;
        }
        playlist.songs.push(StoredSong {
            mid: song.mid.clone(),
            title: song.title.clone(),
            singers: song.singers.clone(),
            album: song.album_name.clone(),
            duration_secs: song.duration_secs,
        });
    }

    /// 从第 `index` 张歌单里移出第 `song` 条。
    pub fn remove(&mut self, index: usize, song: usize) {
        let Some(playlist) = self.playlists.get_mut(index) else {
            return;
        };
        if song < playlist.songs.len() {
            playlist.songs.remove(song);
        }
    }

    /// 删掉整张歌单。固定那张「默认」删不掉（返回 `false`）。
    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.playlists.len() || self.is_default(index) {
            return false;
        }
        self.playlists.remove(index);
        true
    }

    /// 名字校验：去首尾空白 → 非空 → 不撞「默认」这个保留名 → 不与**别的**歌单重名
    /// （`ignore` 是正在改名的那张）。
    fn check_name(
        &self,
        name: &str,
        ignore: Option<usize>,
    ) -> std::result::Result<String, NameError> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(NameError::Empty);
        }
        if name == DEFAULT_NAME
            && !ignore.is_some_and(|index| self.is_default(index))
        {
            return Err(NameError::Reserved);
        }
        let clash = self
            .playlists
            .iter()
            .enumerate()
            .any(|(index, playlist)| Some(index) != ignore && playlist.name == name);
        if clash {
            return Err(NameError::Duplicate);
        }
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("canto-playlists-{}-{}", std::process::id(), name));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("建目录");
        dir.join("local_playlists.json")
    }

    /// 一首本地曲目（借 `entry_to_song` 造，跟生产代码同一条映射）。
    fn local_song(path: &std::path::Path, title: &str) -> crate::model::Song {
        crate::library::entry_to_song(&crate::library::Entry {
            path: path.to_path_buf(),
            mtime: 0,
            size: 0,
            title: title.to_string(),
            artist: "张宇".to_string(),
            album: "消息".to_string(),
            duration_secs: 270,
            lyric: None,
        })
    }

    /// 快照要够画出一行：专辑与时长都存下来，读回来还在（文件删了也能写「（已丢失）」那一行）。
    #[test]
    fn a_stored_song_keeps_the_album_and_duration() {
        let path = std::env::temp_dir().join("canto-playlists-snapshot/曲终人散.flac");
        let song = local_song(&path, "曲终人散");
        assert_eq!(song.album_name, "消息", "前提：借来的那首有专辑");
        let mut store = Store::default();
        let index = store.create("通勤").expect("建歌单");
        store.add(index, &song);

        let stored = &store.playlists[0].songs[0];
        assert_eq!(
            (stored.album.as_str(), stored.duration_secs),
            ("消息", 270),
            "专辑与时长都要跟着走"
        );

        let text = serde_json::to_string(&store).expect("序列化");
        let back: Store = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(back.playlists[0].songs[0].duration_secs, 270);
        assert_eq!(back.playlists[0].songs[0].album, "消息");
    }

    /// 老版本（只有 mid / 标题 / 歌手）：版本号对不上 → 整份按空处理，不是崩。
    #[test]
    fn an_old_playlist_file_is_treated_as_empty() {
        let text = r#"{"version":1,"playlists":[{"name":"旧的","songs":[{"mid":"local:C:/a.flac","title":"旧","singers":"人"}]}]}"#;
        let store: std::result::Result<Store, _> = serde_json::from_str(text);
        assert!(
            store.map(|store| store.version == VERSION).unwrap_or(true),
            "老版本号读得出来，但不等于当前版本"
        );
        assert_eq!(VERSION, 2);
    }

    /// 固定那张「默认」：没有就补在**第一张**，别的歌单一个不动；已经有了就不动手
    /// （老文件里没有它也一样补，不升版本号、不丢数据）。
    #[test]
    fn the_default_playlist_is_added_once_at_the_front() {
        let mut store = Store::default();
        assert!(store.ensure_default(), "本来没有，要补一张");
        assert_eq!(store.playlists.len(), 1);
        assert_eq!(store.playlists[0].name, DEFAULT_NAME);
        assert!(store.playlists[0].songs.is_empty());
        assert!(!store.ensure_default(), "已经有了就不再动手");
        assert_eq!(store.playlists.len(), 1);

        // 老文件：只有使用者自己建的一张 → 补在第一张，它自己留在后面。
        let mut old = Store::default();
        old.create("通勤").expect("建歌单");
        assert!(old.ensure_default());
        assert_eq!(
            old.playlists.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec![DEFAULT_NAME, "通勤"],
            "补在第一张，别的原地不动"
        );
    }

    /// 「默认」这张：不能改名、不能删；名字也是保留的（新建 / 改名都不许撞上它）。
    #[test]
    fn the_default_playlist_cannot_be_renamed_or_deleted() {
        let mut store = Store::default();
        store.ensure_default();
        let index = store.create("通勤").expect("建歌单");

        assert_eq!(store.rename(0, "改个名"), Err(NameError::Reserved));
        assert_eq!(store.playlists[0].name, DEFAULT_NAME, "名字没动");
        assert!(!store.delete(0), "删不掉");
        assert_eq!(store.playlists.len(), 2, "两张都还在");

        assert_eq!(store.create(DEFAULT_NAME), Err(NameError::Reserved), "名字留着");
        assert_eq!(store.rename(index, DEFAULT_NAME), Err(NameError::Reserved));
        assert_eq!(store.rename(index, "通勤早"), Ok(()), "别的照旧");
        assert!(store.delete(index), "别的删得掉");
        assert_eq!(store.playlists.len(), 1);
        assert!(store.has_default());
    }

    #[test]
    fn create_rename_and_persist_round_trip() {
        let path = tmp_file("round-trip");
        let mut store = Store::default();
        let index = store.create("通勤").expect("建歌单");
        store.add(
            index,
            &local_song(
                std::path::Path::new(r"C:\m\张宇 - 曲终人散.mp3"),
                "曲终人散",
            ),
        );
        store.rename(index, "通勤路上").expect("改名");
        store.save(&path).expect("落盘");

        let back = Store::load(&path);
        assert_eq!(back.playlists.len(), 1);
        assert_eq!(back.playlists[0].name, "通勤路上");
        assert_eq!(
            back.playlists[0].songs[0].mid,
            r"local:C:\m\张宇 - 曲终人散.mp3"
        );
        assert_eq!(back.playlists[0].songs[0].title, "曲终人散", "标题快照留着");
        assert_eq!(back.playlists[0].songs[0].singers, "张宇");
    }

    #[test]
    fn blank_and_duplicate_names_are_rejected() {
        let mut store = Store::default();
        store.create("通勤").expect("建歌单");
        assert_eq!(store.create("通勤"), Err(NameError::Duplicate));
        assert_eq!(
            store.create("  通勤  "),
            Err(NameError::Duplicate),
            "先去掉首尾空白再比"
        );
        assert_eq!(store.create("   "), Err(NameError::Empty));
        let other = store.create("另一个").expect("第二个歌单");
        assert_eq!(store.rename(other, ""), Err(NameError::Empty));
        assert_eq!(store.rename(other, "通勤"), Err(NameError::Duplicate));
    }

    #[test]
    fn add_keeps_join_order_and_skips_duplicates() {
        let mut store = Store::default();
        let index = store.create("通勤").expect("建歌单");
        let a = local_song(std::path::Path::new("/m/a.mp3"), "甲");
        let b = local_song(std::path::Path::new("/m/b.mp3"), "乙");
        store.add(index, &a);
        store.add(index, &b);
        store.add(index, &a);
        assert_eq!(
            store.playlists[index].songs.len(),
            2,
            "同一首加两次只留一份"
        );
        assert_eq!(store.playlists[0].songs[1].mid, b.mid, "顺序就是加入顺序");
    }

    #[test]
    fn remove_and_delete_shrink_the_store() {
        let mut store = Store::default();
        let index = store.create("通勤").expect("建歌单");
        store.add(index, &local_song(std::path::Path::new("/m/a.mp3"), "甲"));
        store.remove(index, 0);
        assert_eq!(store.playlists[index].songs.len(), 0, "移出一首");
        store.delete(index);
        assert!(store.playlists.is_empty(), "删整张歌单");
    }

    #[test]
    fn missing_files_do_not_erase_the_playlist() {
        let path = tmp_file("missing");
        let mut store = Store::default();
        let index = store.create("通勤").expect("建歌单");
        store.add(
            index,
            &local_song(std::path::Path::new("/m/gone.mp3"), "走远了"),
        );
        store.save(&path).expect("落盘");
        // 文件已经不在磁盘上：条目仍然读得回来、名字还留着。
        let back = Store::load(&path);
        assert_eq!(back.playlists[0].songs.len(), 1);
        assert_eq!(
            back.playlists[0].songs[0].title, "走远了",
            "标题快照不依赖文件在不在"
        );
    }

    #[test]
    fn a_missing_or_broken_file_loads_as_empty() {
        let path = tmp_file("broken");
        assert!(Store::load(&path).playlists.is_empty(), "文件不存在 = 空");
        std::fs::write(&path, "{ 半个 json").expect("写坏文件");
        assert!(Store::load(&path).playlists.is_empty(), "坏了按空处理");
    }
}
