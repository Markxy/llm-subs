//! The application icon, drawn at runtime so no image assets are needed.

pub struct IconImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A dark navy rounded tile, a little lighter towards its edges, with two subtitle-like
/// bars, anti-aliased by supersampling.
pub fn render(size: u32) -> IconImage {
    const SUBSAMPLES: u32 = 4;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let s = size as f32;
    let tile_radius = s * 0.22;
    let bar_height = s * 0.11;
    let bar_radius = bar_height / 2.0;
    let top_bar = Rect { x0: s * 0.24, y0: s * 0.50, x1: s * 0.76, y1: s * 0.50 + bar_height };
    let bottom_bar = Rect { x0: s * 0.16, y0: s * 0.67, x1: s * 0.84, y1: s * 0.67 + bar_height };
    let tile = Rect { x0: s * 0.04, y0: s * 0.04, x1: s * 0.96, y1: s * 0.96 };
    let center = s / 2.0;
    let corner_distance = (tile.x1 - center) * std::f32::consts::SQRT_2;

    for y in 0..size {
        for x in 0..size {
            let mut tile_cover = 0.0;
            let mut bar_cover = 0.0;
            for sy in 0..SUBSAMPLES {
                for sx in 0..SUBSAMPLES {
                    let px = x as f32 + (sx as f32 + 0.5) / SUBSAMPLES as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SUBSAMPLES as f32;
                    if rounded_rect_contains(&tile, tile_radius, px, py) {
                        tile_cover += 1.0;
                    }
                    if rounded_rect_contains(&top_bar, bar_radius, px, py) || rounded_rect_contains(&bottom_bar, bar_radius, px, py) {
                        bar_cover += 1.0;
                    }
                }
            }
            let samples = (SUBSAMPLES * SUBSAMPLES) as f32;
            let tile_cover = tile_cover / samples;
            let bar_cover = bar_cover / samples;
            let (dx, dy) = (x as f32 + 0.5 - center, y as f32 + 0.5 - center);
            let edge = ((dx * dx + dy * dy).sqrt() / corner_distance).powf(1.8);
            let tile = [lerp(18.0, 52.0, edge), lerp(34.0, 80.0, edge), lerp(76.0, 142.0, edge)];
            let [r, g, b] = tile.map(|channel| lerp(channel, 242.0, bar_cover) as u8);
            rgba.extend_from_slice(&[r, g, b, (tile_cover * 255.0) as u8]);
        }
    }
    IconImage { rgba, width: size, height: size }
}

/// Saves the icon as a PNG file, for places that want an image file instead of pixels.
pub fn write_png(icon: &IconImage, path: &std::path::Path) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, icon.width, icon.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&icon.rgba)?;
    Ok(())
}

struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

fn rounded_rect_contains(rect: &Rect, radius: f32, x: f32, y: f32) -> bool {
    if x < rect.x0 || x > rect.x1 || y < rect.y0 || y > rect.y1 {
        return false;
    }
    let cx = x.clamp(rect.x0 + radius, rect.x1 - radius);
    let cy = y.clamp(rect.y0 + radius, rect.y1 - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_opaque_center_and_transparent_corner() {
        let icon = render(32);
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
        assert_eq!(icon.rgba[3], 0);
        let center = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(icon.rgba[center + 3], 255);
    }
}
