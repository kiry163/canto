//! 把二维码 PNG 画到终端。
//!
//! 一个字符格画上下两个像素（`▀` 上半、`▄` 下半、`█` 两个都是模块），所以
//! 111×111 的二维码占 111 列 × 56 行。
//!
//! 默认按**深色终端**来画：模块留空（用终端的深色背景），背景画成亮块，
//! 这样扫出来是"黑模块 + 白底"。浅色终端会反相，所以 PNG 文件一律另存一份，
//! 终端太窄/太矮时也直接给文件路径——画一个扫不出来的残缺码没有意义。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 画到终端，并把 PNG 存到 `png_path`；返回 PNG 路径。
pub fn show(png: &[u8], png_path: &Path) -> Result<Option<PathBuf>> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .context("二维码不是 PNG")?
        .to_luma8();
    let (width, height) = image.dimensions();
    std::fs::create_dir_all(png_path.parent().unwrap_or(Path::new(".")))
        .with_context(|| format!("创建二维码目录失败：{}", png_path.display()))?;
    std::fs::write(png_path, png)
        .with_context(|| format!("写二维码文件失败：{}", png_path.display()))?;

    let (columns, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    // 上下两像素合一行，另留几行给提示文字。
    let needed_rows = height / 2 + 4;
    if u32::from(columns) < width + 1 || u32::from(rows) < needed_rows {
        return Ok(Some(png_path.to_path_buf()));
    }

    let mut out = String::new();
    for y in (0..height).step_by(2) {
        for x in 0..width {
            let top = is_module(&image, x, y);
            let bottom = y + 1 < height && is_module(&image, x, y + 1);
            out.push(match (top, bottom) {
                (true, true) => ' ',
                (true, false) => '▄',
                (false, true) => '▀',
                (false, false) => '█',
            });
        }
        out.push('\n');
    }
    print!("{out}");
    Ok(Some(png_path.to_path_buf()))
}

/// 该像素是不是二维码的"模块"（深色）。
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
    fn renders_modules_as_blanks_on_a_dark_terminal() {
        let dir = std::env::temp_dir().join(format!("canto-qr-test-{}", std::process::id()));
        let path = dir.join("qr.png");
        // 终端尺寸在测试环境里不可控，所以只断言文件落盘与像素判定这两件硬事实。
        show(&sample_png(), &path).expect("渲染不该失败");
        assert!(path.exists(), "PNG 一定要留一份，终端画不下时靠它");
        let image = image::open(&path).expect("图片能读回").to_luma8();
        assert!(is_module(&image, 0, 0));
        assert!(!is_module(&image, 3, 3));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_non_png_payloads() {
        let path = std::env::temp_dir().join("canto-qr-bad.png");
        let error = show(b"<html>not a png</html>", &path).expect_err("非 PNG 要报错");
        assert!(error.to_string().contains("PNG"), "实际：{error}");
    }
}
