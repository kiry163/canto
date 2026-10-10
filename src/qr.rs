//! 把二维码 PNG 变成终端里能画的字符块。
//!
//! 一个字符格画上下两个像素（`▀` 上半、`▄` 下半、`█` 两个都是模块），所以
//! 111×111 的二维码占 111 列 × 56 行。
//!
//! 默认按**深色终端**来画：模块留空（用终端的深色背景），背景画成亮块，
//! 这样扫出来是「黑模块 + 白底」。浅色终端会反相，所以 PNG 一律另存一份。
//! 装不下时画一个扫不出来的残缺码没有意义——尺寸判断交给调用方（登录屏），
//! 这里只负责「把像素变成字符」与「落盘」。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 把 PNG 变成一行行字符（每行一个字符串，上下两像素合一格）。
pub fn to_lines(png: &[u8]) -> Result<Vec<String>> {
    let image = decode(png)?;
    let (width, height) = image.dimensions();
    let mut lines = Vec::with_capacity((height / 2) as usize);
    for y in (0..height).step_by(2) {
        let mut line = String::with_capacity(width as usize);
        for x in 0..width {
            let top = is_module(&image, x, y);
            let bottom = y + 1 < height && is_module(&image, x, y + 1);
            line.push(match (top, bottom) {
                (true, true) => ' ',
                (true, false) => '▄',
                (false, true) => '▀',
                (false, false) => '█',
            });
        }
        lines.push(line);
    }
    Ok(lines)
}

/// 二维码的像素尺寸（宽, 高）。登录屏用它在画不下时说清「需要多大」。
pub fn dimensions(png: &[u8]) -> Result<(u32, u32)> {
    Ok(decode(png)?.dimensions())
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
    Ok(image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .context("二维码不是 PNG")?
        .to_luma8())
}

/// 该像素是不是二维码的「模块」（深色）。
fn is_module(image: &image::GrayImage, x: u32, y: u32) -> bool {
    image.get_pixel(x, y).0[0] < 128
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张 4×4 的 PNG：左上 2×2 是模块（黑），其余是底色（白）。
    fn sample_png() -> Vec<u8> {
        let mut image = image::GrayImage::from_pixel(4, 4, image::Luma([255]));
        for y in 0..2 {
            for x in 0..2 {
                image.put_pixel(x, y, image::Luma([0]));
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

    #[test]
    fn renders_one_character_per_two_pixel_rows() {
        let png = sample_png();
        let lines = to_lines(&png).expect("渲染");
        assert_eq!(lines.len(), 2, "4 像素高 = 2 行");
        assert_eq!(lines[0].chars().count(), 4, "每行 4 列");
        assert_eq!(dimensions(&png).expect("读尺寸"), (4, 4));
        assert!(lines[0].starts_with("  █"), "左上两个像素是模块：{:?}", lines[0]);
    }

    #[test]
    fn rejects_non_png_payloads() {
        let error = to_lines(b"<html>not a png</html>").expect_err("非 PNG 要报错");
        assert!(error.to_string().contains("PNG"), "实际：{error}");
    }

    #[test]
    fn saves_the_png_beside_the_session() {
        let dir = std::env::temp_dir().join(format!("canto-qr-test-{}", std::process::id()));
        let path = dir.join("login-qr.png");
        save_png(&sample_png(), &path).expect("落盘");
        assert!(path.exists(), "终端画不下时靠这个文件");
        assert_eq!(image::open(&path).expect("能读回").to_luma8().dimensions(), (4, 4));
        assert!(
            save_png(b"not a png", &dir.join("bad.png")).is_err(),
            "错误页不该被存成二维码图片"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
