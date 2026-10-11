//! 把二维码 PNG 变成终端里能画的字符块。
//!
//! `ptqrshow` 给的是一张 **1-bit PNG**：符号本体（33×33 模块）每模块 3 像素、四周 2 模块
//! 静区，合计 111×111。逐像素往屏幕上搬等于把 9 份重复的像素都画出来（要 112×61 的终端），
//! 所以这里先把图**还原成模块网格**再画：一个模块占一个字符格宽、上下两个模块拼一格
//! （`▀` / `▄` / `█` / 空），33 模块的码只占 37 列 × 19 行——普通终端就装得下。
//! 终端够大时按 `scale` 把每模块放大成 `scale`×`scale` 像素，画大一点更好扫。
//!
//! 默认按**深色终端**来画：模块留空（用终端的深色背景），背景画成亮块，
//! 这样扫出来是「黑模块 + 白底」。浅色终端会反相，所以 PNG 一律另存一份。
//! 装不下时画一个扫不出来的残缺码没有意义——尺寸判断交给调用方（登录屏），
//! 这里只负责「把像素变成字符」与「落盘」。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// QR 规格允许的模块数（version 1..40，恒为奇数）。
const MIN_MODULES: u32 = 21;
const MAX_MODULES: u32 = 177;

/// 还原出来的码体：`true` = 深色（模块）。含静区，静区那一圈是 `false`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modules {
    rows: Vec<Vec<bool>>,
}

impl Modules {
    /// 单位（模块）数：宽 × 高。
    pub fn dims(&self) -> (usize, usize) {
        let width = self.rows.first().map_or(0, Vec::len);
        (width, self.rows.len())
    }

    /// 第 `row` 行第 `column` 列是不是深色。
    fn get(&self, column: usize, row: usize) -> bool {
        self.rows
            .get(row)
            .and_then(|line| line.get(column))
            .copied()
            .unwrap_or(false)
    }
}

/// 把 PNG 还原成模块网格。
///
/// 认不出「整数个像素/模块」（图被缩放过、带了抗锯齿）时退回**逐像素**网格：那是最坏
/// 情况的保底，画出来跟以前一样大，但至少画得出。
pub fn modules(png: &[u8]) -> Result<Modules> {
    let image = decode(png)?;
    if let Some((unit, x0, y0, count)) = grid_of(&image) {
        // 静区也照原样带上；原图没留静区就自己补一个模块，手机才好对焦。
        let quiet = (x0 / unit).max(1) as i64;
        let size = count as usize + quiet as usize * 2;
        let base_x = x0 as i64 - quiet * unit as i64;
        let base_y = y0 as i64 - quiet * unit as i64;
        let half = (unit / 2) as i64;
        let mut rows = Vec::with_capacity(size);
        for row in 0..size {
            let y = base_y + row as i64 * unit as i64 + half;
            rows.push(
                (0..size)
                    .map(|column| {
                        let x = base_x + column as i64 * unit as i64 + half;
                        sample(&image, x, y)
                    })
                    .collect(),
            );
        }
        return Ok(Modules { rows });
    }
    let (width, height) = image.dimensions();
    let mut rows = Vec::with_capacity(height as usize);
    for y in 0..height {
        rows.push((0..width).map(|x| is_module(&image, x, y)).collect());
    }
    Ok(Modules { rows })
}

/// 按 `scale` 像素/模块把网格画成字符行（一个字符格 = 1 像素宽 × 2 像素高）。
///
/// `scale` 取奇数时模块跨半格对齐不了，所以调用方只给 1 与偶数。
pub fn render(modules: &Modules, scale: u16) -> Vec<String> {
    let scale = scale.max(1) as usize;
    let (width, height) = modules.dims();
    let pixel_rows = height * scale;
    let mut lines = Vec::with_capacity(pixel_rows.div_ceil(2));
    for row in (0..pixel_rows).step_by(2) {
        let mut line = String::with_capacity(width * scale);
        for column in 0..width {
            let top = pixel(modules, column, row, scale);
            let bottom = pixel(modules, column, row + 1, scale);
            let character = match (top, bottom) {
                (true, true) => ' ',
                (true, false) => '▄',
                (false, true) => '▀',
                (false, false) => '█',
            };
            line.extend(std::iter::repeat_n(character, scale));
        }
        lines.push(line);
    }
    lines
}

/// 把 PNG 落到 `path`（终端画不下时靠它）。
pub fn save_png(png: &[u8], path: &Path) -> Result<PathBuf> {
    // 先当 PNG 解一遍：别把错误页存成「二维码图片」。
    decode(png)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建二维码目录失败：{}", parent.display()))?;
    }
    std::fs::write(path, png).with_context(|| format!("写二维码文件失败：{}", path.display()))?;
    Ok(path.to_path_buf())
}

/// 用系统默认查看器打开文件——把二维码递给手机扫的最后一条路。
pub fn open_with_system(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .with_context(|| format!("调 {program} 打开 {} 失败", path.display()))
        .map(|_| ())
}

fn decode(png: &[u8]) -> Result<image::GrayImage> {
    Ok(
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .context("二维码不是 PNG")?
            .to_luma8(),
    )
}

/// 该像素是不是二维码的「模块」（深色）。
fn is_module(image: &image::GrayImage, x: u32, y: u32) -> bool {
    image.get_pixel(x, y).0[0] < 128
}

/// 越界当底色（补出来的静区就落在画面外）。
fn sample(image: &image::GrayImage, x: i64, y: i64) -> bool {
    let (width, height) = image.dimensions();
    x >= 0
        && y >= 0
        && (x as u32) < width
        && (y as u32) < height
        && is_module(image, x as u32, y as u32)
}

/// 认出「每模块多少像素、码体从哪开始、一共多少模块」；不是干净的等比缩放就是 `None`。
fn grid_of(image: &image::GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = image.dimensions();
    let (x0, y0, x1, y1) = dark_bbox(image)?;
    let (core_width, core_height) = (x1 - x0 + 1, y1 - y0 + 1);
    if core_width != core_height {
        return None;
    }
    let unit = min_dark_run(image, x0, y0, x1, y1);
    if unit == 0 || core_width % unit != 0 {
        return None;
    }
    let count = core_width / unit;
    if !(MIN_MODULES..=MAX_MODULES).contains(&count) || count.is_multiple_of(2) {
        return None;
    }
    // 四边静区也得是同样的整数个模块——不一致就说明这张图不是原样缩放的。
    let (left, top) = (x0, y0);
    let (right, bottom) = (width - 1 - x1, height - 1 - y1);
    if left != top || right != bottom || left % unit != 0 || right % unit != 0 || left != right {
        return None;
    }
    Some((unit, x0, y0, count))
}

/// 深色像素的包围盒（静区不算深色，所以这就是码体本身）。
fn dark_bbox(image: &image::GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = image.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    let mut seen = false;
    for y in 0..height {
        for x in 0..width {
            if is_module(image, x, y) {
                seen = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    seen.then_some((x0, y0, x1, y1))
}

/// 码体内最短的连续深色游程 = 一个模块的像素数：时序线必然是 1 模块宽的黑白交替。
fn min_dark_run(image: &image::GrayImage, x0: u32, y0: u32, x1: u32, y1: u32) -> u32 {
    let mut shortest = u32::MAX;
    for y in y0..=y1 {
        let mut run = 0;
        for x in x0..=x1 {
            if is_module(image, x, y) {
                run += 1;
            } else {
                if run > 0 {
                    shortest = shortest.min(run);
                }
                run = 0;
            }
        }
        if run > 0 {
            shortest = shortest.min(run);
        }
    }
    if shortest == u32::MAX { 0 } else { shortest }
}

fn pixel(modules: &Modules, column: usize, row: usize, scale: usize) -> bool {
    modules.get(column, row / scale)
}

#[cfg(test)]
pub(crate) mod test_support {
    /// 造一张「模块边长 `unit` 像素、四周 `quiet` 模块静区」的 PNG；`grid` 里 `true`
    /// 是深色模块。
    pub(crate) fn png(grid: &[Vec<bool>], unit: u32, quiet: u32) -> Vec<u8> {
        let count = grid.len() as u32;
        let size = (count + quiet * 2) * unit;
        let mut image = image::GrayImage::from_pixel(size, size, image::Luma([255]));
        for (row, line) in grid.iter().enumerate() {
            for (column, dark) in line.iter().enumerate() {
                if !*dark {
                    continue;
                }
                for dy in 0..unit {
                    for dx in 0..unit {
                        let x = (quiet + column as u32) * unit + dx;
                        let y = (quiet + row as u32) * unit + dy;
                        image.put_pixel(x, y, image::Luma([0]));
                    }
                }
            }
        }
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("编码测试 PNG");
        bytes
    }

    /// `count`×`count` 的棋盘格：每个深色模块都孤单，便于核对采样位置。
    pub(crate) fn checkerboard(count: usize) -> Vec<Vec<bool>> {
        (0..count)
            .map(|row| (0..count).map(|column| (row + column) % 2 == 0).collect())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modules_of(grid: &[Vec<bool>], unit: u32, quiet: u32) -> Modules {
        modules(&test_support::png(grid, unit, quiet)).expect("还原模块网格")
    }

    /// 旧版逐像素的 4×4 样例：左上 2×2 是模块（黑），其余底色（白）。
    fn sample_png() -> Vec<u8> {
        let grid = vec![
            vec![true, true, false, false],
            vec![true, true, false, false],
            vec![false; 4],
            vec![false; 4],
        ];
        test_support::png(&grid, 1, 0)
    }

    #[test]
    fn restores_the_module_grid_from_a_scaled_png() {
        let grid = test_support::checkerboard(21);
        let restored = modules_of(&grid, 3, 2);
        assert_eq!(restored.dims(), (25, 25), "21 模块 + 四周 2 模块静区");
        let quiet = restored.get(0, 0);
        assert!(!quiet, "静区不该是模块");
        for (row, line) in grid.iter().enumerate() {
            for (column, dark) in line.iter().enumerate() {
                assert_eq!(
                    restored.get(column + 2, row + 2),
                    *dark,
                    "第 {row} 行第 {column} 列采错"
                );
            }
        }
    }

    #[test]
    fn renders_two_module_rows_per_character_cell() {
        let restored = modules_of(&test_support::checkerboard(21), 1, 2);
        let lines = render(&restored, 1);
        assert_eq!(lines.len(), 13, "25 像素高 = 13 行");
        assert_eq!(lines[0].chars().count(), 25, "每行 25 列");
        assert_eq!(lines[0], "█".repeat(25), "头两行都是静区：整行亮块");
        assert!(
            lines[1].starts_with("██▄▀▄▀"),
            "静区是亮块、深色模块留空：{:?}",
            lines[1]
        );
    }

    #[test]
    fn scale_repeats_every_module() {
        let restored = modules_of(&test_support::checkerboard(21), 3, 2);
        let lines = render(&restored, 2);
        assert_eq!(lines.len(), 25, "每模块 2 像素：25 模块 = 25 行");
        assert_eq!(lines[0].chars().count(), 50, "横向也放大一倍");
        assert!(
            lines[2].starts_with("████  ██"),
            "每格重复两遍：{:?}",
            lines[2]
        );
    }

    #[test]
    fn falls_back_to_pixels_when_modules_are_not_integral() {
        // 4×4 的黑块不可能是 21..177 个模块之一，只能逐像素画。
        let restored = modules(&sample_png()).expect("还原");
        assert_eq!(restored.dims(), (4, 4));
        let lines = render(&restored, 1);
        assert_eq!(lines.len(), 2, "4 像素高 = 2 行");
        assert_eq!(lines[0].chars().count(), 4, "每行 4 列");
        assert!(
            lines[0].starts_with("  █"),
            "左上两个像素是模块：{:?}",
            lines[0]
        );
    }

    #[test]
    fn rejects_non_png_payloads() {
        let error = modules(b"<html>not a png</html>").expect_err("非 PNG 要报错");
        assert!(error.to_string().contains("PNG"), "实际：{error}");
    }

    #[test]
    fn saves_the_png_beside_the_session() {
        let dir = std::env::temp_dir().join(format!("canto-qr-test-{}", std::process::id()));
        let path = dir.join("login-qr.png");
        save_png(&sample_png(), &path).expect("落盘");
        assert!(path.exists(), "终端画不下时靠这个文件");
        assert_eq!(
            image::open(&path).expect("能读回").to_luma8().dimensions(),
            (4, 4)
        );
        assert!(
            save_png(b"not a png", &dir.join("bad.png")).is_err(),
            "错误页不该被存成二维码图片"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
