//! Turns the converter's slots into `<img>` elements and image files.
//!
//! Equation sizing follows md2kindle: the image is sized in `em` (1 MathJax ex
//! = 0.5 em) so equations scale with the reader's font, inline equations get a
//! `vertical-align` that puts their baseline on the text baseline, and the PNG
//! is rasterised at a higher density so it stays sharp on 300 ppi screens.
//!
//! Rows of an aligned equation group share one set of column widths, so when
//! each row is its own (centred) image the `=` signs still line up.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::convert::{Align, Book, EqTable, Slot, SvgLabel};
use crate::math::{MathSvg, UNITS_PER_EX};
use crate::raster::{self, PngMode};
use crate::tree::{El, Node};

const EM_PER_EX: f64 = 0.5;
/// MathJax units per em of the surrounding text.
pub const UNITS_PER_TEXT_EM: f64 = UNITS_PER_EX / EM_PER_EX;
/// Pixels per MathJax ex in the PNG (md2kindle uses 22; a bit more for crispness).
const PX_PER_EX: f64 = 28.0;
const MAX_FIGURE_PX: f32 = 2400.0;

pub struct Asset {
    pub path: String,
    pub media_type: &'static str,
    pub data: Vec<u8>,
}

pub struct Placed<'a> {
    pub x: f64,
    pub baseline: f64,
    pub m: &'a MathSvg,
}

pub struct MathImage {
    pub png: Vec<u8>,
    pub width_em: f64,
    pub height_em: f64,
    /// Depth below the baseline, in em (positive).
    pub depth_em: f64,
}

pub fn units_to_em(u: f64) -> f64 {
    u / UNITS_PER_EX * EM_PER_EX
}

/// Compose MathJax SVGs into one PNG. Coordinates are MathJax units, y down.
pub fn compose(
    parts: &[Placed],
    width: f64,
    height: f64,
    baseline: f64,
) -> Result<MathImage, String> {
    let scale = PX_PER_EX / UNITS_PER_EX;
    let wpx = (width * scale).ceil().max(1.0);
    let hpx = (height * scale).ceil().max(1.0);
    let (w, h) = (wpx / scale, hpx / scale);

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{wpx}" height="{hpx}" viewBox="0 0 {w:.3} {h:.3}">"#
    );
    for p in parts {
        let src = &p.m.svg;
        let open_end = src.find('>').ok_or("bad svg")? + 1;
        let close = src.rfind("</svg>").ok_or("bad svg")?;
        let inner = src[open_end..close].replace("currentColor", "#000000");
        let ch = p.m.ascent + p.m.descent;
        svg.push_str(&format!(
            r#"<svg x="{:.3}" y="{:.3}" width="{:.3}" height="{:.3}" viewBox="{:.3} {:.3} {:.3} {:.3}" overflow="visible">{inner}</svg>"#,
            p.x,
            p.baseline - p.m.ascent,
            p.m.width,
            ch.max(1.0),
            p.m.min_x,
            -p.m.ascent,
            p.m.width,
            ch.max(1.0),
        ));
    }
    svg.push_str("</svg>");

    let tree = raster::parse(svg.as_bytes()).map_err(|e| e.to_string())?;
    let pixmap = raster::render(&tree, 1.0, false).map_err(|e| e.to_string())?;
    let png = raster::encode(&pixmap, PngMode::GrayAlpha).map_err(|e| e.to_string())?;
    Ok(MathImage {
        png,
        width_em: units_to_em(w),
        height_em: units_to_em(h),
        depth_em: units_to_em(h - baseline),
    })
}

pub fn single(m: &MathSvg) -> Result<MathImage, String> {
    compose(
        &[Placed {
            x: 0.0,
            baseline: m.ascent,
            m,
        }],
        m.width,
        m.ascent + m.descent,
        m.ascent,
    )
}

/// One equation, with empty space added above or below so that its baseline
/// sits where `vertical-align: middle` will put it: `half_x_em` (half the
/// reader font's x-height) below the image's centre.
pub fn padded_for_middle(m: &MathSvg, half_x_em: f64) -> Result<MathImage, String> {
    let two_h = 2.0 * half_x_em * UNITS_PER_TEXT_EM;
    let (mut a, mut d) = (m.ascent, m.descent);
    if a - d > two_h {
        d = a - two_h;
    } else {
        a = d + two_h;
    }
    compose(
        &[Placed {
            x: 0.0,
            baseline: a,
            m,
        }],
        m.width,
        a + d,
        a,
    )
}

/// A display equation with its number drawn into the same image, on the
/// equation's baseline. Space for the number is reserved on both sides so the
/// equation itself stays centred. `num_width` lets a group of rows reserve the
/// same space so they still line up.
pub fn with_number(eq: &MathSvg, num: &MathSvg, num_width: f64) -> Result<MathImage, String> {
    let gap = 1.5 * UNITS_PER_TEXT_EM;
    let side = num_width.max(num.width) + gap;
    let a = eq.ascent.max(num.ascent);
    let d = eq.descent.max(num.descent);
    let width = side + eq.width + side;
    compose(
        &[
            Placed {
                x: side,
                baseline: a,
                m: eq,
            },
            Placed {
                x: width - num.width,
                baseline: a,
                m: num,
            },
        ],
        width,
        a + d,
        a,
    )
}

/// Rows `rows` of an equation table as one image. With `number`, the
/// equation number (and the width to reserve for it, so a group's rows stay
/// the same width) is drawn into the image as `with_number` does.
pub fn eq_rows(
    t: &EqTable,
    rows: std::ops::Range<usize>,
    math: &[Result<MathSvg, String>],
    number: Option<(&MathSvg, f64)>,
) -> Result<MathImage, String> {
    let get = |job: Option<usize>| -> Result<Option<&MathSvg>, String> {
        match job {
            None => Ok(None),
            Some(j) => math[j].as_ref().map(Some).map_err(|e| e.clone()),
        }
    };
    // Column widths over the whole table, so separate row images align.
    let ncols = t.aligns.len();
    let mut colw = vec![0.0f64; ncols];
    for row in &t.rows {
        for (j, cell) in row.iter().enumerate() {
            if let Some(m) = get(*cell)? {
                colw[j] = colw[j].max(m.width);
            }
        }
    }
    let mut colx = vec![0.0f64; ncols];
    let mut x = 0.0;
    for j in 0..ncols {
        if j > 0 {
            x += t.gaps[j] * 1000.0;
        }
        colx[j] = x;
        x += colw[j];
    }
    let width = x.max(1.0);

    let stacked = rows.len() > 1;
    let mut parts = Vec::new();
    let mut y = 0.0;
    let mut first_baseline = None;
    for (k, r) in rows.clone().enumerate() {
        let cells: Vec<(usize, &MathSvg)> = t.rows[r]
            .iter()
            .enumerate()
            .filter_map(|(j, c)| get(*c).transpose().map(|m| m.map(|m| (j, m))))
            .collect::<Result<_, _>>()?;
        let mut asc = cells.iter().map(|(_, m)| m.ascent).fold(0.0, f64::max);
        let mut desc = cells.iter().map(|(_, m)| m.descent).fold(0.0, f64::max);
        if stacked {
            asc = asc.max(750.0);
            desc = desc.max(250.0);
            if k > 0 {
                y += 300.0;
            }
        }
        let baseline = y + asc;
        first_baseline.get_or_insert(baseline);
        for (j, m) in cells {
            let x = match t.aligns[j] {
                Align::Left => colx[j],
                Align::Right => colx[j] + colw[j] - m.width,
                Align::Center => colx[j] + (colw[j] - m.width) / 2.0,
            };
            parts.push(Placed { x, baseline, m });
        }
        y = baseline + desc;
    }
    let first_baseline = first_baseline.unwrap_or(y);
    let Some((num, num_width)) = number else {
        return compose(&parts, width, y.max(1.0), first_baseline);
    };

    // One row: the number sits on its baseline (as in LaTeX). Several rows
    // sharing one number: centred vertically.
    let gap = 1.5 * UNITS_PER_TEXT_EM;
    let side = num_width.max(num.width) + gap;
    let num_baseline = if stacked {
        y / 2.0 + (num.ascent - num.descent) / 2.0
    } else {
        first_baseline
    };
    let shift = (num.ascent - num_baseline).max(0.0);
    for p in &mut parts {
        p.x += side;
        p.baseline += shift;
    }
    let total = width + 2.0 * side;
    parts.push(Placed {
        x: total - num.width,
        baseline: num_baseline + shift,
        m: num,
    });
    let height = (y + shift).max(num_baseline + shift + num.descent);
    compose(&parts, total, height.max(1.0), first_baseline + shift)
}

fn math_img(img: &MathImage, path: &str, alt: &str, display: bool) -> Node {
    let style = if display {
        format!("width:{:.3}em", img.width_em)
    } else {
        format!(
            "width:{:.3}em;height:{:.3}em;vertical-align:-{:.3}em",
            img.width_em, img.height_em, img.depth_em
        )
    };
    El::new("img")
        .attr("src", path)
        .attr("alt", alt)
        .class(if display {
            "ltx_math_img ltx_math_display"
        } else {
            "ltx_math_img"
        })
        .attr("style", style)
        .into()
}

fn tex_fallback(tex: &str) -> Node {
    El::new("code")
        .class("ltx_tex_fallback")
        .text(tex.to_string())
        .into()
}

fn sniff(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(b"\x89PNG") {
        return Some(("image/png", "png"));
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(("image/jpeg", "jpg"));
    }
    if data.starts_with(b"GIF8") {
        return Some(("image/gif", "gif"));
    }
    if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some(("image/webp", "webp"));
    }
    is_svg(data).then_some(("image/svg+xml", "svg"))
}

/// True if the document's root element is <svg> (an HTML error page is not).
fn is_svg(data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&data[..data.len().min(64 * 1024)]);
    let mut s = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(rest) = s.strip_prefix("<?") {
            let Some(end) = rest.find("?>") else {
                return false;
            };
            s = rest[end + 2..].trim_start();
        } else if let Some(rest) = s.strip_prefix("<!--") {
            let Some(end) = rest.find("-->") else {
                return false;
            };
            s = rest[end + 3..].trim_start();
        } else if s.starts_with("<!DOCTYPE") || s.starts_with("<!doctype") {
            if !s[9..].trim_start().to_ascii_lowercase().starts_with("svg") {
                return false;
            }
            // Skip the doctype, including any internal subset.
            let end = match (s.find('['), s.find('>')) {
                (Some(b), Some(g)) if b < g => s.find("]>").map(|i| i + 2),
                (_, Some(g)) => Some(g + 1),
                _ => None,
            };
            let Some(end) = end else { return false };
            s = s[end..].trim_start();
        } else {
            return s.starts_with("<svg") || s.starts_with("<svg:svg");
        }
    }
}

/// Put typeset labels into a picture in place of its `\u{1}N\u{1}` markers.
fn with_labels(svg: &str, labels: &[SvgLabel], math: &[Result<MathSvg, String>]) -> String {
    let mut out = String::with_capacity(svg.len());
    for (k, part) in svg.split('\u{1}').enumerate() {
        if k % 2 == 0 {
            out.push_str(part);
            continue;
        }
        let Some(label) = part.parse::<usize>().ok().and_then(|n| labels.get(n)) else {
            continue;
        };
        let Ok(m) = &math[label.job] else { continue };
        let s = label.font / 1000.0;
        let (w, asc, desc) = (m.width * s, m.ascent * s, m.descent * s);
        let x = if label.width > 0.0 {
            (label.width - w) / 2.0
        } else {
            0.0
        };
        let top = match label.baseline {
            Some(b) if asc + desc <= label.height * 1.5 => b - asc,
            _ => (label.height - asc - desc) / 2.0,
        };
        let (Some(open_end), Some(close)) = (m.svg.find('>'), m.svg.rfind("</svg>")) else {
            continue;
        };
        let inner = &m.svg[open_end + 1..close];
        out.push_str(&format!(
            r#"<svg x="{x:.3}" y="{top:.3}" width="{w:.3}" height="{h:.3}" viewBox="{:.3} {:.3} {:.3} {:.3}" overflow="visible">{inner}</svg>"#,
            m.min_x,
            -m.ascent,
            m.width,
            (m.ascent + m.descent).max(1.0),
            h = (asc + desc).max(0.001),
        ));
    }
    out
}

/// Rasterise an SVG figure on white, at roughly 3x its nominal size.
fn svg_figure(data: &[u8]) -> Result<Vec<u8>, String> {
    let tree = raster::parse(data).map_err(|e| e.to_string())?;
    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.0);
    let scale = (1600.0 / size.width().max(1.0))
        .clamp(1.0, 4.0)
        .min(MAX_FIGURE_PX / longest);
    let pixmap = raster::render(&tree, scale, true).map_err(|e| e.to_string())?;
    raster::encode(&pixmap, PngMode::Rgb).map_err(|e| e.to_string())
}

pub struct Layout {
    pub filled: Vec<Node>,
    pub assets: Vec<Asset>,
    pub warnings: Vec<String>,
}

pub fn layout(
    book: &Book,
    math: &[Result<MathSvg, String>],
    fetched: &[Result<Vec<u8>, String>],
) -> Layout {
    let mut warnings = Vec::new();
    let mut assets = Vec::new();

    for (job, r) in book.math_jobs.iter().zip(math) {
        if let Err(e) = r {
            warnings.push(format!(
                "equation could not be typeset, kept as TeX: {} ({e})",
                job.tex
            ));
        }
    }

    // 1. Single equations, rendered once per distinct job.
    let mut single_jobs: Vec<usize> = book
        .slots
        .iter()
        .filter_map(|s| match s {
            Slot::Math { job, .. } => Some(*job),
            _ => None,
        })
        .collect();
    single_jobs.sort_unstable();
    single_jobs.dedup();
    let singles: HashMap<usize, Result<MathImage, String>> = single_jobs
        .par_iter()
        .map(|&j| (j, math[j].as_ref().map_err(|e| e.clone()).and_then(single)))
        .collect();

    // 2. Equation rows and inline pictures, one image per slot.
    let per_slot: Vec<Option<Result<MathImage, String>>> = book
        .slots
        .par_iter()
        .map(|s| match s {
            Slot::EqRows { table, rows } => {
                Some(eq_rows(&book.eq_tables[*table], rows.clone(), math, None))
            }
            _ => None,
        })
        .collect();
    let svgs: Vec<Option<Result<Vec<u8>, String>>> = book
        .slots
        .par_iter()
        .map(|s| match s {
            Slot::Svg { svg, labels, .. } => {
                Some(svg_figure(with_labels(svg, labels, math).as_bytes()))
            }
            _ => None,
        })
        .collect();

    // 3. Figures, converted once per distinct source.
    // Each also carries its natural width in CSS px, for images that come
    // without a width attribute.
    type Figure = (Vec<u8>, &'static str, &'static str, Option<f64>);
    let figures: Vec<Result<Figure, String>> = fetched
        .par_iter()
        .map(|r| {
            let data = r.as_ref().map_err(|e| e.clone())?;
            match sniff(data) {
                Some(("image/svg+xml", _)) => {
                    let w = raster::parse(data).ok().map(|t| t.size().width() as f64);
                    Ok((svg_figure(data)?, "image/png", "png", w))
                }
                Some((mt, ext)) => {
                    let w = imagesize::blob_size(data).ok().map(|s| s.width as f64);
                    Ok((data.clone(), mt, ext, w))
                }
                None => Err("unrecognised image format".to_string()),
            }
        })
        .collect();
    let mut figure_paths: Vec<Option<String>> = Vec::with_capacity(figures.len());
    let mut figure_px: Vec<Option<f64>> = Vec::with_capacity(figures.len());
    for (i, f) in figures.into_iter().enumerate() {
        figure_px.push(f.as_ref().ok().and_then(|f| f.3));
        match f {
            Ok((data, media_type, ext, _)) => {
                let path = format!("images/fig{i:03}.{ext}");
                assets.push(Asset {
                    path: path.clone(),
                    media_type,
                    data,
                });
                figure_paths.push(Some(path));
            }
            Err(e) => {
                warnings.push(format!(
                    "figure {} could not be included: {e}",
                    book.images[i]
                ));
                figure_paths.push(None);
            }
        }
    }

    let mut single_paths: HashMap<usize, String> = HashMap::new();
    for &j in &single_jobs {
        if let Ok(img) = &singles[&j] {
            let path = format!("images/m{j:04}.png");
            assets.push(Asset {
                path: path.clone(),
                media_type: "image/png",
                data: img.png.clone(),
            });
            single_paths.insert(j, path);
        }
    }

    let mut filled = Vec::with_capacity(book.slots.len());
    for (i, slot) in book.slots.iter().enumerate() {
        let node = match slot {
            Slot::Math { job, display } => {
                let tex = &book.math_jobs[*job].tex;
                match (&singles[job], single_paths.get(job)) {
                    (Ok(img), Some(path)) => math_img(img, path, tex, *display),
                    _ => tex_fallback(tex),
                }
            }
            Slot::EqRows { table, rows } => match per_slot[i].as_ref().unwrap() {
                Ok(img) => {
                    let path = format!("images/eq{i:04}.png");
                    assets.push(Asset {
                        path: path.clone(),
                        media_type: "image/png",
                        data: img.png.clone(),
                    });
                    let alt = book.eq_tables[*table].row_tex[rows.clone()].join(" \\\\ ");
                    math_img(img, &path, &alt, true)
                }
                Err(e) => {
                    let tex = book.eq_tables[*table].row_tex[rows.clone()].join(" \\\\ ");
                    warnings.push(format!(
                        "equation row could not be typeset, kept as TeX: {tex} ({e})"
                    ));
                    tex_fallback(&tex)
                }
            },
            Slot::Image {
                src,
                width,
                in_figure,
                alt,
            } => match &figure_paths[*src] {
                Some(path) => {
                    let mut img = El::new("img")
                        .attr("src", path.clone())
                        .attr("alt", alt.clone())
                        .class("ltx_graphics");
                    // No width given: size by pixels against a ~700px column;
                    // small images (icons) keep their natural size.
                    let auto = figure_px[*src]
                        .filter(|px| *px >= 200.0)
                        .map(|px| (px / 7.0).min(100.0));
                    if let (true, Some(w)) = (*in_figure, width.or(auto)) {
                        img.set("style", format!("width:{w:.1}%"));
                    }
                    img.into()
                }
                None => El::new("span")
                    .class("ltx_missing")
                    .text(format!("[{alt} unavailable]"))
                    .into(),
            },
            Slot::Svg { alt, style, .. } => match svgs[i].as_ref().unwrap() {
                Ok(png) => {
                    let path = format!("images/pic{i:04}.png");
                    assets.push(Asset {
                        path: path.clone(),
                        media_type: "image/png",
                        data: png.clone(),
                    });
                    let mut img = El::new("img")
                        .attr("src", path)
                        .attr("alt", alt.clone())
                        .class("ltx_graphics");
                    if let Some(st) = style {
                        img.set("style", st.clone());
                    }
                    img.into()
                }
                Err(e) => {
                    warnings.push(format!("inline picture could not be rendered: {e}"));
                    El::new("span")
                        .class("ltx_missing")
                        .text(format!("[{alt} unavailable]"))
                        .into()
                }
            },
        };
        filled.push(node);
    }

    Layout {
        filled,
        assets,
        warnings,
    }
}
