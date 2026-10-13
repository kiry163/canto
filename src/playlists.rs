//! 本地歌单：使用者自己从本地曲库挑出来组织的列表。
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
pub const VERSION: u32 = 1;

/// 存进歌单的一条：`mid`（`local:<路径>`）+ 标题与歌手的**快照**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSong {
    pub mid: String,
    pub title: String,
    pub singers: String,
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
}

impl std::fmt::Display for NameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            NameError::Empty => "名字不能是空的",
            NameError::Duplicate => "这个名字已经有过了",
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

    /// 新建一张，返回它在列表里的下标。名字去首尾空白，空 / 重名拒绝。
    pub fn create(&mut self, name: &str) -> std::result::Result<usize, NameError> {
        let name = self.check_name(name, None)?;
        self.playlists.push(LocalPlaylist {
            name,
            songs: Vec::new(),
        });
        Ok(self.playlists.len() - 1)
    }

    /// 改名（同样校验；改成自己原来的名字不算重名）。
    pub fn rename(&mut self, index: usize, name: &str) -> std::result::Result<(), NameError> {
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

    /// 删掉整张歌单。
    pub fn delete(&mut self, index: usize) {
        if index < self.playlists.len() {
            self.playlists.remove(index);
        }
    }

    /// 名字校验：去首尾空白 → 非空 → 不与**别的**歌单重名（`ignore` 是正在改名的那张）。
    fn check_name(
        &self,
        name: &str,
        ignore: Option<usize>,
    ) -> std::result::Result<String, NameError> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(NameError::Empty);
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
        })
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
