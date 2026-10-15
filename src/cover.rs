//! A plain typographic cover (1600x2560, Kindle's recommended size).

use anyhow::Result;

use crate::raster::{self, PngMode};
use crate::tree::esc;

const W: f64 = 1600.0;
const H: f64 = 2560.0;
const MARGIN: f64 = 150.0;

/// Greedy word wrap using an average glyph width estimate.
fn wrap(text: &str, font_px: f64, avg_em: f64) -> Vec<String> {
    let max_chars = ((W - 2.0 * MARGIN) / (font_px * avg_em)).floor().max(4.0) as usize;
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max_chars {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

pub fn render(title: &str, authors: &[String], footer: &str) -> Result<Vec<u8>> {
    // Largest title size that fits in eight lines.
    let mut size = 120.0;
    let mut lines = wrap(title, size, 0.52);
    while lines.len() > 8 && size > 50.0 {
        size -= 8.0;
        lines = wrap(title, size, 0.52);
    }

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
<rect width="100%" height="100%" fill="#ffffff"/>
<rect x="{MARGIN}" y="300" width="{bar}" height="14" fill="#b31b1b"/>
"##,
        bar = W - 2.0 * MARGIN
    );
    let mut y = 300.0 + 90.0 + size;
    for l in &lines {
        svg.push_str(&format!(
            r##"<text x="{MARGIN}" y="{y:.0}" font-family="serif" font-size="{size:.0}" font-weight="bold" fill="#111111">{}</text>
"##,
            esc(l)
        ));
        y += size * 1.22;
    }

    let author_text = match authors.len() {
        0 => String::new(),
        1..=6 => authors.join(", "),
        _ => format!("{}, et al.", authors[..6].join(", ")),
    };
    let mut ay = y + 120.0;
    for l in wrap(&author_text, 56.0, 0.5).iter().take(6) {
        svg.push_str(&format!(
            r##"<text x="{MARGIN}" y="{ay:.0}" font-family="serif" font-size="56" fill="#444444">{}</text>
"##,
            esc(l)
        ));
        ay += 72.0;
    }

    svg.push_str(&format!(
        r##"<rect x="{MARGIN}" y="{ry}" width="{bar}" height="4" fill="#111111"/>
<text x="{MARGIN}" y="{fy}" font-family="sans-serif" font-size="46" fill="#111111">{}</text>
</svg>"##,
        esc(footer),
        ry = H - 330.0,
        fy = H - 240.0,
        bar = W - 2.0 * MARGIN
    ));

    let tree = raster::parse(svg.as_bytes())?;
    let pixmap = raster::render(&tree, 1.0, true)?;
    raster::encode(&pixmap, PngMode::Rgb)
}
