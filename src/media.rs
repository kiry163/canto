//! 媒体流：把 CDN 上的文件做成 `Read + Seek`。
//!
//! 为什么需要它：rodio 的解码器要求 `Read + Seek`（要能回头读 header），而 HTTP
//! 响应只能顺序读。CDN 实测支持 `Accept-Ranges: bytes`，所以"往回跳"就重开一次
//! 带 `Range` 的请求，"往前跳"就近读掉（省一次连接）。这样边下边播，不用先落盘。
//!
//! 只有 seek 到**已读过的位置**才会重连；正常顺序播放是一次请求走到底。

use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::Response;

/// 往前 seek 超过这个距离就重连（读掉几 MB 只会白等）。
const FORWARD_SEEK_LIMIT: u64 = 4 << 20;

/// 开流时校验的头部长度。
const SNIFF_LEN: usize = 16;

pub struct MediaStream {
    client: reqwest::blocking::Client,
    url: String,
    response: Option<Response>,
    /// 开流时嗅探用的头部字节：先交给调用方，再接着读网络。
    peek: Vec<u8>,
    peek_pos: usize,
    /// 下一个待交付字节的绝对偏移。
    pos: u64,
    /// 文件总长（`Content-Length`）；HEAD 拿不到时是 `None`。
    len: Option<u64>,
}

/// 认得出来的音频文件头。
///
/// **不能看 `Content-Type`**：实测 FLAC 文件被标成 `audio/x-ogg`，错误页反而标成
/// `audio/mpeg`，这个头完全不可信。404 那种错误由状态码挡住，但 CDN/代理也可能
/// 用 200 回一段错误页正文——那时只有魔数能说明问题（`sniff` 会当场拒掉）。
fn looks_like_audio(head: &[u8]) -> bool {
    head.starts_with(b"fLaC")                     // FLAC
        || head.starts_with(b"OggS")              // Ogg/Vorbis
        || head.starts_with(b"ID3")               // MP3（带标签）
        || head.starts_with(b"RIFF")              // WAV
        || head.starts_with(b"\x1aE\xdf\xa3")    // WebM/Matroska
        || head.get(4..8).is_some_and(|kind| kind == b"ftyp") // MP4/M4A
        || head
            .get(..2)
            .is_some_and(|sync| sync[0] == 0xFF && sync[1] & 0xE0 == 0xE0) // MP3/AAC 帧同步
}

impl MediaStream {
    /// 打开一条流。这里已经发出第一个请求，所以 404/403 会立刻暴露。
    pub fn open(url: &str) -> io::Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(crate::http::UA)
            .connect_timeout(Duration::from_secs(8))
            // 不设总超时：一首歌要连着放几分钟。流中途卡住由 TCP 自身的重传
            // 超时兜底（reqwest 的 blocking 客户端没有单独的读超时）。
            .build()
            .map_err(io_error)?;
        let mut stream = Self {
            client,
            url: url.to_string(),
            response: None,
            peek: Vec::new(),
            peek_pos: 0,
            pos: 0,
            len: None,
        };
        stream.request_from(0)?;
        Ok(stream)
    }

    /// 文件总长（`Content-Length` 或 `Content-Range` 里的总数）。
    pub fn total(&self) -> Option<u64> {
        self.len
    }

    /// 从 `bytes={start}-` 开始取（`start == 0` 时用普通 GET，顺便拿到总长）。
    fn request_from(&mut self, start: u64) -> io::Result<()> {
        self.peek.clear();
        self.peek_pos = 0;
        if self.len.is_some_and(|len| start >= len) {
            // 直接跳到文件尾：不需要真的发请求。
            self.response = None;
            self.pos = start;
            return Ok(());
        }
        let mut request = self.client.get(&self.url);
        if start > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={start}-"));
        }
        let response = request.send().map_err(io_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("CDN 返回 {status}（文件不存在或已过期）"),
            ));
        }
        if start > 0 && status == StatusCode::PARTIAL_CONTENT {
            self.len = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.rsplit('/').next())
                .and_then(|total| total.trim().parse::<u64>().ok())
                .or(self.len);
        } else {
            self.len = response.content_length();
        }
        self.response = Some(response);
        self.pos = start;
        if start == 0 {
            self.sniff(status)?;
        }
        Ok(())
    }

    /// 读头部几个字节目测是不是音频，并把这几个字节存起来供后续 `read` 交付。
    fn sniff(&mut self, status: StatusCode) -> io::Result<()> {
        let mut head = [0_u8; SNIFF_LEN];
        let mut filled = 0;
        while filled < SNIFF_LEN {
            let Some(response) = self.response.as_mut() else {
                break;
            };
            let read = response.read(&mut head[filled..])?;
            if read == 0 {
                self.response = None;
                break;
            }
            filled += read;
        }
        let head = &head[..filled];
        if !looks_like_audio(head) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "返回的不是音频（{}，前 {} 字节 {:02x?}）——可能是 CDN 错误页或占位内容",
                    status,
                    filled,
                    &head[..filled.min(8)]
                ),
            ));
        }
        // 不打印 URL：purl 里带着 vkey，等于把播放凭据写进日志。
        tracing::debug!(
            "开流成功：总长{:?}，头部 {:02x?}",
            self.len,
            &head[..filled.min(4)]
        );
        self.peek = head.to_vec();
        Ok(())
    }

    fn discard(&mut self, mut remaining: u64) -> io::Result<()> {
        let mut scratch = [0_u8; 64 * 1024];
        while remaining > 0 {
            let want = remaining.min(scratch.len() as u64) as usize;
            let read = self.read(&mut scratch[..want])?;
            if read == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "流提前结束"));
            }
            remaining -= read as u64;
        }
        Ok(())
    }
}

fn io_error(error: reqwest::Error) -> io::Error {
    io::Error::other(error)
}

impl Read for MediaStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.peek_pos < self.peek.len() {
            let pending = self.peek.len() - self.peek_pos;
            let take = pending.min(buf.len());
            buf[..take].copy_from_slice(&self.peek[self.peek_pos..self.peek_pos + take]);
            self.peek_pos += take;
            self.pos += take as u64;
            return Ok(take);
        }
        if self.response.is_none() {
            self.request_from(self.pos)?;
        }
        let Some(response) = self.response.as_mut() else {
            return Ok(0); // 已经到文件尾
        };
        let read = response.read(buf)?;
        if read == 0 {
            self.response = None;
        }
        self.pos += read as u64;
        Ok(read)
    }
}

impl Seek for MediaStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(delta) => self.pos.saturating_add_signed(delta),
            SeekFrom::End(delta) => match self.len {
                Some(len) => len.saturating_add_signed(delta),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "总长未知，无法从文件尾定位",
                    ));
                }
            },
        };
        if target == self.pos {
            return Ok(target);
        }
        if target > self.pos && target - self.pos <= FORWARD_SEEK_LIMIT {
            // 往前读掉比重连快，也少一次 CDN 请求。
            self.discard(target - self.pos)?;
            return Ok(self.pos);
        }
        self.request_from(target)?;
        Ok(target)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 测试内容的第 `i` 个字节：开头是 FLAC 魔数，其余可预测。
    /// 返回 (端口, 收到的请求数)。
    fn serve(len: u32) -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("测试服务要能监听");
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut raw = [0_u8; 2048];
                let read = stream.read(&mut raw).unwrap_or(0);
                let request = String::from_utf8_lossy(&raw[..read]).to_string();
                let start = request
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("Range: bytes=")
                            .and_then(|value| value.split('-').next())
                            .and_then(|value| value.trim().parse::<u32>().ok())
                    })
                    .unwrap_or(0);
                let body: Vec<u8> = (start..len)
                    .map(|byte| match byte {
                        0 => b'f',
                        1 => b'L',
                        2 => b'a',
                        3 => b'C',
                        _ => (byte % 251) as u8,
                    })
                    .collect();
                let head = if start == 0 {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{len}\r\nConnection: close\r\n\r\n",
                        body.len(),
                        len - 1
                    )
                };
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        (port, hits)
    }

    #[test]
    fn sequential_reads_do_not_reconnect() {
        let (port, hits) = serve(4096);
        let mut stream =
            MediaStream::open(&format!("http://127.0.0.1:{port}/x.flac")).expect("打开流");
        let mut buf = vec![0_u8; 2048];
        stream.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..4], b"fLaC", "嗅探过的头几个字节要原样交付");
        assert_eq!(buf[2047], (2047 % 251) as u8);
        assert_eq!(stream.stream_position().unwrap(), 2048);
        assert_eq!(hits.load(Ordering::SeqCst), 1, "顺序读不该重连");
    }

    #[test]
    fn backward_seek_refetches_with_range_and_forward_seek_discards() {
        let (port, hits) = serve(4096);
        let mut stream =
            MediaStream::open(&format!("http://127.0.0.1:{port}/x.flac")).expect("打开流");
        let mut buf = [0_u8; 512];
        stream.read_exact(&mut buf).unwrap();

        // 往前跳：读掉即可，不发新请求。
        stream.seek(SeekFrom::Start(1500)).unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "小步往前跳不该重连");
        let mut one = [0_u8; 1];
        stream.read_exact(&mut one).unwrap();
        assert_eq!(one[0], (1500 % 251) as u8, "跳转后必须读到正确位置的字节");

        // 往回跳：必须重开一次 Range 请求，且拿到的是同一个文件。
        let start = stream.seek(SeekFrom::Start(64)).unwrap();
        assert_eq!(start, 64);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "往回跳要重连");
        stream.read_exact(&mut one).unwrap();
        assert_eq!(one[0], 64, "跳回 64 字节处要读到该位置的字节");

        // 从文件尾定位（解码器用它算 byte_len）。
        let end = stream.seek(SeekFrom::End(-16)).unwrap();
        assert_eq!(end, 4096 - 16);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "文件尾不该真的再发请求");
    }

    /// 错误页不能当音频：CDN 对不存在的文件会回 64 字节 JSON（实测多为 404，
    /// 但同样的请求过一会儿又可能 200），代理也可能插一页自己的说明。
    #[test]
    fn error_pages_are_not_mistaken_for_audio() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut raw = [0_u8; 1024];
                let _ = stream.read(&mut raw);
                let body = br#"{"errorcode":-46628,"errormsg":"file not exist"}"#;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\n\r\n");
                let _ = stream.write_all(body);
            }
        });
        let error = match MediaStream::open(&format!("http://127.0.0.1:{port}/fake.mp3")) {
            Ok(_) => panic!("错误页必须被挡下来，不能当音频播"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("不是音频"),
            "错误信息要说清是什么：{error}"
        );
    }

    #[test]
    fn missing_files_surface_immediately() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut raw = [0_u8; 1024];
                let _ = stream.read(&mut raw);
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        });
        let error = match MediaStream::open(&format!("http://127.0.0.1:{port}/gone.mp3")) {
            Ok(_) => panic!("404 必须当场报错"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("404"),
            "错误信息要带状态码：{error}"
        );
    }
}
