//! Builds `reader-test.epub`: the same equations, tables and figures laid out
//! in a few different ways (labelled A, B, C, ...), to find out which layout a
//! given e-reader displays correctly.
//!
//!     cargo run --release --example device_test [-- out.epub]

use anyhow::{Result, anyhow};
use epub_converter::convert::{Align, Book, Chapter, EqTable, TocEntry};
use epub_converter::layout::{self, Asset, MathImage};
use epub_converter::math::{self, MathJob, MathSvg};
use epub_converter::tree::{El, Node};
use epub_converter::{epub, raster, source};

const INLINE: &[&str] = &[
    r"P_{c}",
    r"f_{\theta^{*}}(x)",
    r"\sum_{i=1}^{n}x_{i}",
    r"\frac{a}{b}",
    r"y_{j}",
    r"\mathcal{L}_{\text{minimality}}",
];
const EQ1: &str =
    r"Z^{(c)}_{:,j}=\begin{cases}W_{:,c}&\text{if }j=c,\\ \textbf{0}&\text{otherwise}.\end{cases}";
const ROWS: &[[&str; 2]] = &[
    [
        r"\displaystyle\mathcal{L}_{\text{MDL}}(x)",
        r"{}\displaystyle=\sum^{C}_{c=1}||A_{c}(x)||_{0}\left(\alpha\sum_{l}\text{rank}(P_{c,l})+\log_{2}(C)\right)",
    ],
    [
        "",
        r"{}\displaystyle=\log_{2}(C)\left(\sum^{C}_{c=1}||A_{c}(x)||_{0}\right)+\alpha\left(\sum^{C}_{c=1}||A_{c}(x)||_{0}\sum_{l}\text{rank}(P_{c,l})\right)",
    ],
    [
        "",
        r"{}\displaystyle=:\log_{2}(C)\mathcal{L}_{\text{minimality}}^{\text{idealized}}(x)+\alpha\mathcal{L}_{\text{simplicity}}^{\text{idealized}}(x)",
    ],
];
const NUMBERS: &[&str] = &["(1)", "(11)", "(12)", "(13)"];
const FIG1: &str = "https://arxiv.org/html/2501.14926v4/spd_target2.svg";
const FIG6: &str =
    "https://arxiv.org/html/2501.14926v4/figures/resid_mlp_weights_1layers_8qz1si1l.png";

struct Images(Vec<Asset>);

impl Images {
    fn add(&mut self, png: Vec<u8>) -> String {
        let path = format!("images/t{:03}.png", self.0.len());
        self.0.push(Asset {
            path: path.clone(),
            media_type: "image/png",
            data: png,
        });
        path
    }
    fn math(&mut self, img: &MathImage, alt: &str, style: String) -> Node {
        let path = self.add(img.png.clone());
        El::new("img")
            .attr("src", path)
            .attr("alt", alt)
            .class("ltx_math_img")
            .attr("style", style)
            .into()
    }
}

fn p(text: &str) -> Node {
    El::new("p").text(text).into()
}

fn label(text: &str) -> Node {
    El::new("h3").text(text).into()
}

fn size(img: &MathImage) -> String {
    format!("width:{:.3}em;height:{:.3}em", img.width_em, img.height_em)
}

fn chapter(file: &str, title: &str, body: Vec<Node>) -> Chapter {
    let mut section = El::new("section").child(El::new("h2").text(title));
    section.children.extend(body);
    Chapter {
        file: file.into(),
        title: title.into(),
        body: vec![section.into()],
        notes: vec![],
        toc: vec![TocEntry {
            level: 1,
            label: title.into(),
            id: None,
        }],
    }
}

/// The test sentence, with `eq(i)` producing the i-th inline equation.
fn sentence(mut eq: impl FnMut(usize) -> Node) -> Node {
    let words = [
        "Typography test: a component ",
        " and its output ",
        " give ",
        " with ratio ",
        " for every ",
        ", keeping ",
        " very young, glossy, and happy.",
    ];
    let mut el = El::new("p").text(words[0]);
    for (i, w) in words[1..].iter().enumerate() {
        el.children.push(eq(i));
        el.children.push(Node::text(*w));
    }
    el.into()
}

fn get(m: &[Result<MathSvg, String>], i: usize) -> Result<&MathSvg> {
    m[i].as_ref().map_err(|e| anyhow!("equation {i}: {e}"))
}

fn main() -> Result<()> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "reader-test.epub".into());

    // Typeset everything in one batch.
    let mut jobs: Vec<MathJob> = INLINE
        .iter()
        .map(|t| MathJob {
            tex: t.to_string(),
            mathml: None,
            display: false,
        })
        .collect();
    let eq1 = jobs.len();
    jobs.push(MathJob {
        tex: EQ1.into(),
        mathml: None,
        display: true,
    });
    let rows0 = jobs.len();
    for row in ROWS {
        for cell in row {
            jobs.push(MathJob {
                tex: cell.to_string(),
                mathml: None,
                display: false,
            });
        }
    }
    let nums0 = jobs.len();
    for n in NUMBERS {
        jobs.push(MathJob {
            tex: format!(r"\text{{{n}}}"),
            mathml: None,
            display: false,
        });
    }
    let tms = jobs.len();
    jobs.push(MathJob {
        tex: "TMS_{5-2}".into(),
        mathml: None,
        display: false,
    });
    jobs.push(MathJob {
        tex: "TMS_{40-10}".into(),
        mathml: None,
        display: false,
    });
    let m = math::render_all(&jobs, "")?;
    let mut imgs = Images(Vec::new());
    let e = |r: Result<MathImage, String>| r.map_err(|e| anyhow!(e));

    // --- Inline equations -------------------------------------------------
    let mut inline = vec![p(
        "Each version below is the same sentence. Check whether the bottom of each equation's letters lines up \
         with the bottom of the words around it (look at letters like x, a, n). Letters such as y, g and j \
         should hang below the line the same way in the equations and in the words.",
    )];
    inline.push(label("A (current)"));
    inline.push(sentence(|i| {
        let img = layout::single(get(&m, i).unwrap()).unwrap();
        let st = format!("{};vertical-align:-{:.3}em", size(&img), img.depth_em);
        imgs.math(&img, INLINE[i], st)
    }));
    inline.push(label("B (padded image, vertical-align: middle)"));
    inline.push(sentence(|i| {
        let img = layout::padded_for_middle(get(&m, i).unwrap(), 0.24).unwrap();
        let st = format!("{};vertical-align:middle", size(&img));
        imgs.math(&img, INLINE[i], st)
    }));
    inline.push(label(
        "C (B, plus the exact shift for readers that support it)",
    ));
    inline.push(sentence(|i| {
        let img = layout::padded_for_middle(get(&m, i).unwrap(), 0.24).unwrap();
        let st = format!(
            "{};vertical-align:middle;vertical-align:-{:.3}em",
            size(&img),
            img.depth_em
        );
        imgs.math(&img, INLINE[i], st)
    }));
    inline.push(label("D (moved down with position: relative)"));
    inline.push(sentence(|i| {
        let img = layout::single(get(&m, i).unwrap()).unwrap();
        let st = format!("{};position:relative;top:{:.3}em", size(&img), img.depth_em);
        imgs.math(&img, INLINE[i], st)
    }));
    inline.push(label("E (no shift at all, for comparison)"));
    inline.push(sentence(|i| {
        let img = layout::single(get(&m, i).unwrap()).unwrap();
        imgs.math(&img, INLINE[i], size(&img))
    }));

    // --- Equation numbers -------------------------------------------------
    let table = EqTable {
        aligns: vec![Align::Right, Align::Left],
        gaps: vec![0.0, 0.0],
        rows: (0..ROWS.len())
            .map(|r| {
                (0..2)
                    .map(|c| (!ROWS[r][c].is_empty()).then_some(rows0 + 2 * r + c))
                    .collect()
            })
            .collect(),
        row_tex: vec![String::new(); ROWS.len()],
    };
    let num = |k: usize| get(&m, nums0 + k).unwrap();
    let group_num_w = (1..4).map(|k| num(k).width).fold(0.0, f64::max);
    let display =
        |img: &MathImage| format!("width:{:.3}em;max-width:100%;height:auto", img.width_em);

    let mut numbers = vec![p(
        "Equation (1), then equations (11) to (13), which should have their = signs lined up. \
         Check where the numbers end up: ideally on the right, level with their equation.",
    )];
    numbers.push(label("A (current: number floated to the right)"));
    let img = e(layout::single(get(&m, eq1)?))?;
    let node = imgs.math(&img, "eq 1", display(&img));
    numbers.push(
        El::new("div")
            .class("ltx_eq")
            .child(El::new("span").class("ltx_eqno").text("(1)"))
            .child(node)
            .into(),
    );
    for r in 0..3 {
        let img = e(layout::eq_rows(&table, r..r + 1, &m, None))?;
        let node = imgs.math(&img, "row", display(&img));
        let no = NUMBERS[r + 1];
        numbers.push(
            El::new("div")
                .class("ltx_eq")
                .child(El::new("span").class("ltx_eqno").text(no))
                .child(node)
                .into(),
        );
    }

    numbers.push(label("B (number drawn inside the equation image)"));
    let img = e(layout::with_number(get(&m, eq1)?, num(0), 0.0))?;
    let node = imgs.math(&img, "eq 1", display(&img));
    numbers.push(El::new("div").class("ltx_eq").child(node).into());
    for r in 0..3 {
        let img = e(layout::eq_rows(
            &table,
            r..r + 1,
            &m,
            Some((num(r + 1), group_num_w)),
        ))?;
        let node = imgs.math(&img, "row", display(&img));
        numbers.push(El::new("div").class("ltx_eq").child(node).into());
    }

    numbers.push(label("C (number as text straight after the equation)"));
    let inline_after = |imgs: &mut Images, img: &MathImage, no: &str| -> Node {
        let st = format!(
            "width:{:.3}em;max-width:85%;height:auto;vertical-align:middle",
            img.width_em
        );
        let node = imgs.math(img, "eq", st);
        El::new("p")
            .attr("style", "text-align:center")
            .child(node)
            .text(format!("\u{a0}\u{a0}{no}"))
            .into()
    };
    let img = e(layout::single(get(&m, eq1)?))?;
    numbers.push(inline_after(&mut imgs, &img, "(1)"));
    for r in 0..3 {
        let img = e(layout::eq_rows(&table, r..r + 1, &m, None))?;
        numbers.push(inline_after(&mut imgs, &img, NUMBERS[r + 1]));
    }

    // --- Tables -----------------------------------------------------------
    let tms_imgs: Vec<MathImage> = (0..2)
        .map(|k| layout::single(get(&m, tms + k).unwrap()).unwrap())
        .collect();
    let tms_node = |imgs: &mut Images, k: usize, style_inline: bool| -> Node {
        let img = &tms_imgs[k];
        let st = if style_inline {
            format!("{};vertical-align:-{:.3}em", size(img), img.depth_em)
        } else {
            size(img)
        };
        imgs.math(img, "TMS", st)
    };
    let rows = [
        ["0.998 ± 0.000", "0.893 ± 0.004"],
        ["0.996 ± 0.003", "0.935 ± 0.001"],
    ];
    let mut tables = vec![p(
        "Table 1 from the paper, twice. It should look like a small grid: 3 columns, a header row \
         (MMCS, ML2R) and 2 rows of numbers. If it looks wrong, a photo of each version helps a lot.",
    )];
    tables.push(label("A (current)"));
    let mut t = El::new("table").class("ltx_tabular ltx_centering");
    let mut head = El::new("tr");
    for h in ["", "MMCS", "ML2R"] {
        head.children.push(
            El::new("th")
                .class("ltx_border_tt ltx_align_center")
                .text(h)
                .into(),
        );
    }
    t.children.push(El::new("thead").child(head).into());
    let mut body = El::new("tbody");
    for (k, row) in rows.iter().enumerate() {
        let border = if k == 0 {
            "ltx_border_t"
        } else {
            "ltx_border_bb"
        };
        let mut tr = El::new("tr").child(
            El::new("th")
                .class(format!("ltx_align_left {border}"))
                .child(tms_node(&mut imgs, k, true)),
        );
        for c in row {
            tr.children.push(
                El::new("td")
                    .class(format!("ltx_align_center {border}"))
                    .text(*c)
                    .into(),
            );
        }
        body.children.push(tr.into());
    }
    t.children.push(body.into());
    tables.push(El::new("figure").class("ltx_table").child(t).into());

    tables.push(label(
        "B (plain table, every cell boxed, no stylesheet rules)",
    ));
    let cell = "border:1px solid #000;padding:0.2em 0.5em";
    let mut t = El::new("table");
    let mut head = El::new("tr");
    for h in ["", "MMCS", "ML2R"] {
        head.children
            .push(El::new("td").attr("style", cell).text(h).into());
    }
    t.children.push(head.into());
    for (k, row) in rows.iter().enumerate() {
        let mut tr = El::new("tr").child(
            El::new("td")
                .attr("style", cell)
                .child(tms_node(&mut imgs, k, false)),
        );
        for c in row {
            tr.children
                .push(El::new("td").attr("style", cell).text(*c).into());
        }
        t.children.push(tr.into());
    }
    tables.push(t.into());

    // --- Figures ----------------------------------------------------------
    let fetched = source::fetch_images(&[FIG1.to_string(), FIG6.to_string()]);
    let fig1 = fetched[0].as_ref().map_err(|e| anyhow!(e.clone()))?;
    let tree = raster::parse(fig1)?;
    let scale = (1600.0 / tree.size().width()).clamp(1.0, 4.0);
    let pix = raster::render(&tree, scale, true)?;
    let fig1_rgb = (pix.width(), pix.height(), raster::pixmap_rgb(&pix));
    let fig6_rgb = raster::decode_png_rgb(fetched[1].as_ref().map_err(|e| anyhow!(e.clone()))?)?;

    let mut figures = vec![p(
        "Figures 1 and 6 from the paper, each in three versions. Check whether the light colours \
         (pale fills, light lines) are still visible, without the figure becoming too dark overall.",
    )];
    for (name, (w, h, rgb)) in [("Figure 1", &fig1_rgb), ("Figure 6", &fig6_rgb)] {
        for (tag, strength, gamma) in [
            ("A (original colours)", None, 1.0),
            ("B (grayscale, light colours darkened)", Some(0.45), 1.0),
            ("C (grayscale, darkened more)", Some(0.8), 1.4),
        ] {
            figures.push(label(&format!("{name}: {tag}")));
            let png = match strength {
                None => {
                    let mut out = Vec::new();
                    {
                        let mut enc = png::Encoder::new(&mut out, *w, *h);
                        enc.set_color(png::ColorType::Rgb);
                        enc.set_depth(png::BitDepth::Eight);
                        enc.write_header()?.write_image_data(rgb)?;
                    }
                    out
                }
                Some(s) => raster::encode_gray(*w, *h, &raster::eink_gray(rgb, s, gamma))?,
            };
            let path = imgs.add(png);
            figures.push(
                El::new("figure")
                    .child(
                        El::new("img")
                            .attr("src", path)
                            .attr("alt", name)
                            .class("ltx_graphics")
                            .attr("style", "width:100%"),
                    )
                    .into(),
            );
        }
    }

    let intro = vec![
        p(
            "This book tests how your e-reader shows equations, equation numbers, tables and figures.",
        ),
        p(
            "Each chapter shows the same thing a few different ways, labelled A, B, C and so on. \
           For each chapter, note which letters look right and which look wrong.",
        ),
        p(
            "The device: which reader and which app (for example: Supernote Nomad, built-in reader).",
        ),
    ];
    let book = Book {
        title: "epub-converter reader test".into(),
        math_preamble: String::new(),
        has_title: true,
        authors: vec!["epub-converter".into()],
        lang: "en".into(),
        chapters: vec![
            chapter("title.xhtml", "How to use this test", intro),
            chapter("ch01.xhtml", "1. Equations inside text", inline),
            chapter("ch02.xhtml", "2. Equation numbers", numbers),
            chapter("ch03.xhtml", "3. Tables", tables),
            chapter("ch04.xhtml", "4. Light colours in figures", figures),
        ],
        slots: vec![],
        math_jobs: vec![],
        eq_tables: vec![],
        images: vec![],
        warnings: vec![],
    };
    let meta = epub::Meta {
        identifier: "urn:uuid:5d1e6f2a-0c3b-5a7e-a1f4-3b9c0d2e8f71".into(),
        source_url: None,
        arxiv_id: None,
        modified: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map(|s| format!("{}Z", &s[..19]))?,
    };
    epub::write(std::path::Path::new(&out), &book, &meta, &imgs.0, None)?;
    eprintln!("wrote {out}");
    Ok(())
}
