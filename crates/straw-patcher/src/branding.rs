//! Studio branding: the logo shown in the window and the window icon.

use egui::ColorImage;

/// The logo artwork, embedded in the executable at build time.
const LOGO_PNG: &[u8] = include_bytes!("../assets/InnovLogo.png");

/// Decodes the logo into an egui image.
pub fn logo_image() -> ColorImage {
    let decoder = png::Decoder::new(std::io::Cursor::new(LOGO_PNG));
    let mut reader = decoder.read_info().expect("the embedded logo is a valid PNG");
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .expect("the embedded logo could be decoded");
    let raw = &buffer[..info.buffer_size()];

    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => 3,
    };

    // The artwork sits on a white canvas. Making that transparent lets the
    // logo blend with the window background instead of showing a white box.
    let mut pixels = Vec::with_capacity((info.width * info.height * 4) as usize);
    for pixel in raw.chunks_exact(channels as usize) {
        let (r, g, b, mut a) = match info.color_type {
            png::ColorType::Rgb => (pixel[0], pixel[1], pixel[2], 255),
            png::ColorType::Rgba => (pixel[0], pixel[1], pixel[2], pixel[3]),
            png::ColorType::Grayscale => (pixel[0], pixel[0], pixel[0], 255),
            png::ColorType::GrayscaleAlpha => (pixel[0], pixel[0], pixel[0], pixel[1]),
            png::ColorType::Indexed => (pixel[0], pixel[1], pixel[2], 255),
        };
        if info.color_type != png::ColorType::Rgba && is_near_white(r, g, b) {
            a = 0;
        }
        pixels.extend_from_slice(&[r, g, b, a]);
    }

    ColorImage::from_rgba_unmultiplied([info.width as usize, info.height as usize], &pixels)
}

/// Builds the icon data used for the application window.
pub fn window_icon() -> egui::IconData {
    let image = logo_image();
    let raw = image.as_raw();
    let mut rgba = Vec::with_capacity(raw.len());
    for pixel in raw.chunks_exact(4) {
        // egui expects premultiplied alpha.
        let alpha = pixel[3] as f32 / 255.0;
        rgba.push((pixel[0] as f32 * alpha) as u8);
        rgba.push((pixel[1] as f32 * alpha) as u8);
        rgba.push((pixel[2] as f32 * alpha) as u8);
        rgba.push(pixel[3]);
    }
    egui::IconData {
        rgba,
        width: image.width() as u32,
        height: image.height() as u32,
    }
}

/// True for colours that are effectively the white background.
fn is_near_white(r: u8, g: u8, b: u8) -> bool {
    let min = r.min(g).min(b);
    let max = r.max(g).max(b);
    min >= 250 && max - min <= 5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_decodes_with_transparent_background() {
        let image = logo_image();
        assert_eq!(image.width(), 256);
        assert_eq!(image.height(), 256);
        let raw = image.as_raw();
        let at = |x: usize, y: usize| {
            let offset = (y * image.width() + x) * 4;
            (raw[offset], raw[offset + 1], raw[offset + 2], raw[offset + 3])
        };
        // The corners belong to the background and must be transparent.
        assert_eq!(at(0, 0).3, 0, "the corner should be transparent");
        assert_eq!(at(255, 255).3, 0, "the corner should be transparent");
        // The petals are opaque and keep their colours. The centre of the
        // canvas is a gap between the petals, so sample the upper petal.
        let (r, g, b, a) = at(122, 61);
        assert!(a > 0, "the artwork should be opaque");
        assert!(r > 200 && g < 200 && b > 120 && b < 200, "unexpected colour {r},{g},{b}");
    }

    #[test]
    fn window_icon_is_premultiplied() {
        let icon = window_icon();
        assert_eq!(icon.width, 256);
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        // Fully transparent pixels stay transparent.
        assert_eq!(icon.rgba[3], 0);
    }
}
