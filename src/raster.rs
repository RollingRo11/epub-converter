//! SVG -> PNG with resvg. Used for equations, SVG figures, inline pictures and
//! the cover, so that everything renders identically on every Kindle.

use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, anyhow};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, fontdb};

fn fontdb() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        // Generic families that exist on macOS, Linux and Windows respectively.
        for serif in [
            "Times New Roman",
            "DejaVu Serif",
            "Liberation Serif",
            "Georgia",
        ] {
            if db
                .faces()
                .any(|f| f.families.iter().any(|(n, _)| n == serif))
            {
                db.set_serif_family(serif);
                break;
            }
        }
        for sans in ["Helvetica", "Arial", "DejaVu Sans", "Liberation Sans"] {
            if db
                .faces()
                .any(|f| f.families.iter().any(|(n, _)| n == sans))
            {
                db.set_sans_serif_family(sans);
                break;
            }
        }
        for mono in [
            "Menlo",
            "Courier New",
            "DejaVu Sans Mono",
            "Liberation Mono",
        ] {
            if db
                .faces()
                .any(|f| f.families.iter().any(|(n, _)| n == mono))
            {
                db.set_monospace_family(mono);
                break;
            }
        }
        Arc::new(db)
    })
    .clone()
}

pub fn parse(svg: &[u8]) -> Result<usvg::Tree> {
    let opt = usvg::Options {
        fontdb: fontdb(),
        ..Default::default()
    };
    usvg::Tree::from_data(svg, &opt).map_err(|e| anyhow!("SVG parse error: {e}"))
}

/// Render `tree` scaled by `scale` into a new pixmap.
pub fn render(tree: &usvg::Tree, scale: f32, white_background: bool) -> Result<Pixmap> {
    let size = tree.size();
    let w = (size.width() * scale).ceil().max(1.0) as u32;
    let h = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = Pixmap::new(w, h).context("image too large to rasterise")?;
    if white_background {
        pixmap.fill(resvg::tiny_skia::Color::WHITE);
    }
    resvg::render(
        tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

#[derive(Clone, Copy)]
pub enum PngMode {
    /// Luminance + alpha: equations (black ink on a transparent page).
    GrayAlpha,
    /// Opaque colour: figures on white.
    Rgb,
}

pub fn encode(pixmap: &Pixmap, mode: PngMode) -> Result<Vec<u8>> {
    let (w, h) = (pixmap.width(), pixmap.height());
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        match mode {
            PngMode::GrayAlpha => {
                let y =
                    (0.299 * c.red() as f32 + 0.587 * c.green() as f32 + 0.114 * c.blue() as f32)
                        .round() as u8;
                data.push(y);
                data.push(c.alpha());
            }
            PngMode::Rgb => {
                // Composite onto white in case the source left anything transparent.
                let a = c.alpha() as u32;
                for v in [c.red(), c.green(), c.blue()] {
                    data.push(((v as u32 * a + 255 * (255 - a)) / 255) as u8);
                }
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(match mode {
            PngMode::GrayAlpha => png::ColorType::GrayscaleAlpha,
            PngMode::Rgb => png::ColorType::Rgb,
        });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let mut writer = enc.write_header()?;
        writer.write_image_data(&data)?;
    }
    Ok(out)
}

/// Decode a PNG to 8-bit RGB, composited onto white.
pub fn decode_png_rgb(data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("PNG too large")?];
    let info = reader.next_frame(&mut buf)?;
    let buf = &buf[..info.buffer_size()];
    let on_white = |v: u8, a: u8| ((v as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
    let rgb = match info.color_type {
        png::ColorType::Rgb => buf.to_vec(),
        png::ColorType::Rgba => buf
            .chunks(4)
            .flat_map(|p| {
                [
                    on_white(p[0], p[3]),
                    on_white(p[1], p[3]),
                    on_white(p[2], p[3]),
                ]
            })
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks(2)
            .flat_map(|p| [on_white(p[0], p[1]); 3])
            .collect(),
        png::ColorType::Indexed => anyhow::bail!("unexpanded palette PNG"),
    };
    Ok((info.width, info.height, rgb))
}

/// RGB on white from a rendered pixmap.
pub fn pixmap_rgb(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity((pixmap.width() * pixmap.height() * 3) as usize);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        let a = c.alpha() as u32;
        for v in [c.red(), c.green(), c.blue()] {
            out.push(((v as u32 * a + 255 * (255 - a)) / 255) as u8);
        }
    }
    out
}

/// Grayscale for e-ink, darkening light colours so they don't vanish.
///
/// Plain grayscale turns pale yellow or light blue into almost-white. Here a
/// pixel is darkened in proportion to how colourful it is (`strength`), so
/// white and grays are untouched while tints stay visible; `gamma` > 1 then
/// darkens all mid and light tones a little.
pub fn eink_gray(rgb: &[u8], strength: f32, gamma: f32) -> Vec<u8> {
    rgb.chunks(3)
        .map(|p| {
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            let luma = 0.299 * r + 0.587 * g + 0.114 * b;
            let chroma = (r.max(g).max(b) - r.min(g).min(b)) / 255.0;
            let v = (luma / 255.0) * (1.0 - strength * chroma);
            (v.clamp(0.0, 1.0).powf(gamma) * 255.0).round() as u8
        })
        .collect()
}

pub fn encode_gray(w: u32, h: u32, gray: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        enc.write_header()?.write_image_data(gray)?;
    }
    Ok(out)
}
