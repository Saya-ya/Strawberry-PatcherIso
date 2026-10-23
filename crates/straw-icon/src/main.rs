//! Generates the multi-resolution Windows icon from the studio logo.
//!
//! The logo is a square RGB PNG; the white background is turned transparent
//! and the image is rescaled into the sizes Windows expects for an icon
//! (16, 24, 32, 48, 64 and 256 pixels).
//!
//! Run it from the repository root:
//!
//! ```text
//! cargo run -p straw-icon -- dist/InnovLogo.png dist/icon.ico
//! ```

use std::{env, fs, path::Path};

use anyhow::{bail, Context, Result};
use ico::{IconDir, IconDirEntry, IconImage};

/// Sizes Windows expects in a proper multi-resolution icon.
const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let source = args
        .next()
        .context("usage: straw-icon <logo.png> <icon.ico>")?;
    let target = args
        .next()
        .context("usage: straw-icon <logo.png> <icon.ico>")?;

    let source = Path::new(&source);
    let target = Path::new(&target);

    let (width, height, pixels) = load_logo(source)?;
    if width != height {
        bail!("the logo must be square, got {width}x{height}");
    }

    let mut dir = IconDir::new(ico::ResourceType::Icon);
    for size in SIZES {
        let rgba = resample_nearest(&pixels, width, height, size);
        let image = IconImage::from_rgba_data(size, size, rgba);
        let entry: IconDirEntry = IconDirEntry::encode_as_png(&image)
            .context("failed to encode the icon entry")?;
        dir.add_entry(entry);
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let file = fs::File::create(target)
        .with_context(|| format!("failed to create {}", target.display()))?;
    dir.write(file)
        .with_context(|| format!("failed to write {}", target.display()))?;

    println!(
        "icon written to {} ({}x{})",
        target.display(),
        width,
        height
    );
    Ok(())
}

/// Reads the logo and converts its white background into transparency.
fn load_logo(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let file = fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder
        .read_info()
        .with_context(|| format!("failed to decode {}", path.display()))?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .with_context(|| format!("failed to read the PNG frame of {}", path.display()))?;
    let raw = &buffer[..info.buffer_size()];

    let width = info.width;
    let height = info.height;
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => bail!("indexed PNGs are not supported"),
    };

    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for pixel in raw.chunks_exact(channels as usize) {
        let (r, g, b, mut a) = match info.color_type {
            png::ColorType::Rgb => (pixel[0], pixel[1], pixel[2], 255),
            png::ColorType::Rgba => (pixel[0], pixel[1], pixel[2], pixel[3]),
            png::ColorType::Grayscale => (pixel[0], pixel[0], pixel[0], 255),
            png::ColorType::GrayscaleAlpha => (pixel[0], pixel[0], pixel[0], pixel[1]),
            png::ColorType::Indexed => unreachable!("handled above"),
        };
        // The source art sits on a white canvas. Making that transparent keeps
        // the icon readable on any taskbar or desktop background.
        if info.color_type != png::ColorType::Rgba && is_near_white(r, g, b) {
            a = 0;
        }
        rgba.extend_from_slice(&[r, g, b, a]);
    }

    Ok((width, height, rgba))
}

/// True for colours that are effectively the white background.
fn is_near_white(r: u8, g: u8, b: u8) -> bool {
    let min = r.min(g).min(b);
    let max = r.max(g).max(b);
    min >= 250 && max - min <= 5
}

/// Scales the RGBA buffer to `size` using nearest-neighbour sampling.
///
/// The logo is flat art with hard edges, so nearest-neighbour keeps the
/// shapes crisp instead of blurring them.
fn resample_nearest(pixels: &[u8], width: u32, height: u32, size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        let src_y = (y * height) / size;
        for x in 0..size {
            let src_x = (x * width) / size;
            let offset = ((src_y * width + src_x) * 4) as usize;
            out.extend_from_slice(&pixels[offset..offset + 4]);
        }
    }
    out
}
