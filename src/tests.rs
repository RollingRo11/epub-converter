//! End-to-end test on a small synthetic LaTeXML paper (no network needed).

use std::io::Read;

use url::Url;

use crate::{convert, epub, layout, math, source, tree};

const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn fixture() -> String {
    let pad_l = r#"<td class="ltx_eqn_cell ltx_eqn_center_padleft"></td>"#;
    let pad_r = r#"<td class="ltx_eqn_cell ltx_eqn_center_padright"></td>"#;
    r##"<!DOCTYPE html><html lang="en"><head><title>ignored</title></head><body>
<nav class="ltx_page_navbar">navigation</nav>
<article class="ltx_document">
<h1 class="ltx_title ltx_title_document">A Test <math alttext="x^{2}" display="inline"><msup><mi>x</mi><mn>2</mn></msup></math> Paper</h1>
<div class="ltx_authors"><span class="ltx_creator ltx_role_author"><span class="ltx_personname">Ada Lovelace<br class="ltx_break">Analytical Engines Ltd</span></span></div>
<div id="abstract1" class="ltx_abstract"><h6 class="ltx_title ltx_title_abstract">Abstract</h6><p class="ltx_p">Short &amp; sweet.</p></div>
<nav class="ltx_TOC"><h6>Contents</h6></nav>
<section id="S1" class="ltx_section"><h2 class="ltx_title ltx_title_section"><span class="ltx_tag ltx_tag_section">1 </span>Intro</h2>
<div id="S1.p1" class="ltx_para"><p id="S1.p1.1" class="ltx_p">See <a href="#bib.bib1" class="ltx_ref">[1]</a> and Eq. <a href="#S1.E1" class="ltx_ref">1</a> and <a href="#nowhere" class="ltx_ref">??</a>.<span id="footnote1" class="ltx_note ltx_role_footnote"><sup class="ltx_note_mark">1</sup><span class="ltx_note_outer"><span class="ltx_note_content"><sup class="ltx_note_mark">1</sup><span class="ltx_tag ltx_tag_note">1</span>A note with <math alttext="y" display="inline"><mi>y</mi></math>.</span></span></span>
<span class="ltx_ERROR undefined">\oops</span><button>Report issue</button></p>
<ul class="ltx_itemize"><li class="ltx_item" style="list-style-type:none;"><span class="ltx_tag ltx_tag_item">•</span><div class="ltx_para"><p class="ltx_p">one</p></div></li></ul>
<ol class="ltx_enumerate"><li class="ltx_item"><span class="ltx_tag ltx_tag_item">1.</span><div class="ltx_para"><p class="ltx_p">first</p></div></li><li class="ltx_item"><span class="ltx_tag ltx_tag_item">2.</span><div class="ltx_para"><p class="ltx_p">second</p></div></li></ol>
<ol class="ltx_enumerate"><li class="ltx_item"><span class="ltx_tag ltx_tag_item">(a)</span><div class="ltx_para"><p class="ltx_p">alpha</p></div></li></ol>
<table id="S1.E1" class="ltx_equation ltx_eqn_table"><tbody><tr class="ltx_equation ltx_eqn_row">{pad_l}<td class="ltx_eqn_cell ltx_align_center"><math alttext="E=mc^{2}" display="block"><mi>E</mi></math></td>{pad_r}<td class="ltx_eqn_cell ltx_eqn_eqno"><span class="ltx_tag">(1)</span></td></tr></tbody></table>
<table id="S1.EGx1" class="ltx_equationgroup ltx_eqn_align ltx_eqn_table">
<tbody id="S1.E2"><tr class="ltx_equation ltx_eqn_row">{pad_l}<td class="ltx_eqn_cell ltx_align_right"><math alttext="\displaystyle f(x)" display="inline"><mi>f</mi></math></td><td class="ltx_eqn_cell ltx_align_left"><math alttext="\displaystyle=\sum_{i}x_{i}" display="inline"><mi>s</mi></math></td>{pad_r}<td class="ltx_eqn_cell ltx_eqn_eqno"><span class="ltx_tag">(2)</span></td></tr></tbody>
<tbody id="S1.E3"><tr class="ltx_equation ltx_eqn_row">{pad_l}<td class="ltx_eqn_cell"></td><td class="ltx_eqn_cell ltx_align_left"><math alttext="\displaystyle=0" display="inline"><mn>0</mn></math></td>{pad_r}<td class="ltx_eqn_cell ltx_eqn_eqno"><span class="ltx_tag">(3)</span></td></tr></tbody>
</table>
<figure id="S1.F1" class="ltx_figure"><img src="data:image/png;base64,{PNG_1X1}" class="ltx_graphics" width="476" height="100" alt="Refer to caption"><figcaption class="ltx_caption"><span class="ltx_tag ltx_tag_figure">Figure 1: </span>Cap.</figcaption></figure>
<p class="ltx_p">Unknown macro <math alttext="\foo{x}" display="inline"><mi>x</mi></math> and a <a href="https://example.com/a##b">link</a> and <a href="mailto:">no one</a>.</p>
</div></section>
<section id="bib" class="ltx_bibliography"><h2 class="ltx_title ltx_title_bibliography">References</h2>
<ul class="ltx_biblist"><li id="bib.bib1" class="ltx_bibitem"><span class="ltx_tag ltx_tag_bibitem">[1]</span><span class="ltx_bibblock">Someone. A book.</span></li></ul></section>
</article></body></html>"##
        .replace("{pad_l}", pad_l)
        .replace("{pad_r}", pad_r)
        .replace("{PNG_1X1}", PNG_1X1)
}

#[test]
fn end_to_end() {
    let base = Url::parse("https://arxiv.org/html/0000.00000v1").unwrap();
    let mut book = convert::convert(&fixture(), base).unwrap();
    assert_eq!(book.title, "A Test x^{2} Paper");
    assert_eq!(book.authors, vec!["Ada Lovelace".to_string()]);
    let files: Vec<&str> = book.chapters.iter().map(|c| c.file.as_str()).collect();
    assert_eq!(files, ["title.xhtml", "ch01.xhtml", "ch02.xhtml"]);

    let math = math::render_all(&book.math_jobs, &book.math_preamble).unwrap();
    assert!(math.iter().all(|m| m.is_ok()), "every equation typesets");
    let foo = book
        .math_jobs
        .iter()
        .position(|j| j.tex.contains("\\foo"))
        .unwrap();
    assert!(
        math[foo].as_ref().unwrap().from_mathml,
        "unknown macro falls back to MathML"
    );

    let fetched = source::fetch_images(&book.images);
    let laid = layout::layout(&book, &math, &fetched);
    assert!(laid.warnings.is_empty(), "{:?}", laid.warnings);
    for ch in &mut book.chapters {
        tree::fill_slots(&mut ch.body, &|i| laid.filled[i].clone());
        tree::fill_slots(&mut ch.notes, &|i| laid.filled[i].clone());
    }

    let dir = std::env::temp_dir().join(format!("epub-converter-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("t.epub");
    let meta = epub::Meta {
        identifier: "urn:arxiv:0000.00000v1".into(),
        source_url: None,
        arxiv_id: Some("0000.00000v1".into()),
        modified: "2026-01-01T00:00:00Z".into(),
    };
    epub::write(&out, &book, &meta, &laid.assets, None).unwrap();

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    {
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name().unwrap(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
    }
    let mut read = |name: &str| {
        let mut s = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    };
    let ch1 = read("EPUB/ch01.xhtml");
    for name in [
        "EPUB/title.xhtml",
        "EPUB/ch01.xhtml",
        "EPUB/ch02.xhtml",
        "EPUB/nav.xhtml",
        "EPUB/toc.ncx",
        "EPUB/package.opf",
    ] {
        let text = read(name);
        let opt = resvg::usvg::roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        resvg::usvg::roxmltree::Document::parse_with_options(&text, opt)
            .unwrap_or_else(|e| panic!("{name} is not well-formed: {e}"));
    }

    // Links: cross-chapter, same-chapter, and dangling ones turned into text.
    assert!(ch1.contains(r##"href="ch02.xhtml#bib.bib1""##));
    assert!(ch1.contains(r##"href="#S1.E1""##));
    assert!(!ch1.contains("#nowhere"));
    assert!(ch1.contains(r#"href="https://example.com/a#%23b""#));
    assert!(!ch1.contains("mailto:"));
    // Footnote: a noteref and its aside.
    assert!(
        ch1.contains(r##"epub:type="noteref""##)
            && ch1.contains(r#"<aside epub:type="footnote" id="footnote1""#)
    );
    // Page chrome and LaTeXML errors are gone.
    assert!(!ch1.contains("oops") && !ch1.contains("Report issue"));
    // Standard bullets and numbers use the reader's lists; custom labels stay.
    assert!(!ch1.contains('•') && !ch1.contains(">1.<"));
    assert!(ch1.contains("ltx_custom_list") && ch1.contains("(a)"));
    // Figure sized relative to the text width.
    assert!(ch1.contains("width:100.0%"));
    // Rows of an aligned group share one width, so they line up.
    let widths: Vec<&str> = ch1
        .match_indices("ltx_math_display\" style=\"width:")
        .map(|(i, m)| &ch1[i + m.len()..i + m.len() + 6])
        .collect();
    assert_eq!(widths.len(), 3, "{ch1}");
    assert_eq!(widths[1], widths[2]);
    // Inline math is sized in em with a baseline shift.
    assert!(ch1.contains("vertical-align:-"));

    let nav = read("EPUB/nav.xhtml");
    assert!(
        nav.contains("ch01.xhtml#S1")
            && nav.contains("1 Intro")
            && nav.contains("title.xhtml#abstract1")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn normalize_splits_blocks_out_of_paragraphs() {
    use tree::{El, Node};
    let p = El::new("p")
        .attr("id", "x")
        .text("before")
        .child(El::new("div").text("block"))
        .text("after");
    let out = tree::normalize(Node::El(p));
    let tags: Vec<&str> = out.iter().map(|n| n.as_el().unwrap().tag).collect();
    assert_eq!(tags, ["p", "div", "p"]);
    assert_eq!(out[0].as_el().unwrap().get("id"), Some("x"));
    assert_eq!(out[2].as_el().unwrap().get("id"), None);
}

#[test]
fn ids_are_valid_xml_names() {
    assert_eq!(tree::sanitize_id("S1.E1"), "S1.E1");
    assert_eq!(tree::sanitize_id("1abc"), "x1abc");
    assert_eq!(tree::sanitize_id("a b:c"), "a_b_c");
}

#[test]
fn picture_labels_are_drawn() {
    // A TikZ picture whose only content is a foreignObject label: if the label
    // isn't typeset, the PNG comes out blank.
    let html = r##"<html><body><article class="ltx_document"><section id="S1" class="ltx_section"><h2 class="ltx_title">One</h2>
<figure class="ltx_figure"><svg class="ltx_picture" width="60" height="30" viewBox="0 0 60 30" overflow="visible" version="1.1">
<g transform="translate(0,30) matrix(1 0 0 -1 0 0)"><g transform="matrix(1.0 0.0 0.0 1.0 10 10)" color="#000000">
<foreignObject width="20" height="9" transform="matrix(1 0 0 -1 0 6)" overflow="visible" style="--ltx-fo-width:1.4em;--ltx-fo-height:0.43em;--ltx-fo-depth:0.2em;font-size:10pt;">
<span class="ltx_foreignobject_container"><span class="ltx_foreignobject_content"><math alttext="m_{2}" display="inline"><msub><mi>m</mi><mn>2</mn></msub></math></span></span>
</foreignObject></g></g></svg></figure></section></article></body></html>"##;
    let book = convert::convert(html, Url::parse("https://example.org/").unwrap()).unwrap();
    let math = math::render_all(&book.math_jobs, &book.math_preamble).unwrap();
    assert_eq!(math.len(), 1);
    let laid = layout::layout(&book, &math, &[]);
    assert!(laid.warnings.is_empty(), "{:?}", laid.warnings);
    let png = &laid
        .assets
        .iter()
        .find(|a| a.path.contains("pic"))
        .expect("picture asset")
        .data;

    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let dark = buf[..info.buffer_size()]
        .chunks(3)
        .filter(|px| px.iter().any(|&c| c < 128))
        .count();
    assert!(dark > 20, "label was not drawn ({dark} dark pixels)");
}

#[test]
fn web_page() {
    // A KaTeX blog page: `$` math in text, a nav bar, a hidden element, a
    // WordPress-style LaTeX image, a definition-only block, and a \tag.
    let html = r##"<html lang="en"><head><title>Blog | Site</title>
<meta name="author" content="Ada Lovelace">
<script src="https://cdn.jsdelivr.net/npm/katex@0.16.25/dist/contrib/auto-render.min.js"></script>
<script>renderMathInElement(document.body, {delimiters: [{left: "$$", right: "$$", display: true}, {left: "$", right: "$", display: false}]});</script>
</head><body>
<nav><a href="/">Home</a> <a href="/about">About me and my many interests</a></nav>
<main><article>
<h1>Notes on <em>scaling</em></h1>
<p>$\newcommand{\vx}{\mathbf{x}}$</p>
<p>Loss falls as $L(n) \propto n^{-\alpha}$ for model size $n$, costing \$5 per run, and the input is $\vx$ with $e^{-\Beta{}x}$.</p>
<p>$$ L = \sum_k p_k \tag{2}$$</p>
<h2>Background</h2>
<p>Text with an image of math <img class="latex" src="https://s0.wp.com/latex.php?latex=x" alt="x^2 + 1"> inline, and code <code>$not math$</code>.</p>
<p style="display:none">hidden words</p>
<h2>Results</h2>
<p>Results paragraph that is long enough to count as the main article text for the root finder.</p>
<div class="share-buttons">Share this on everything</div>
<div class="panel"><figcaption>A stray caption outside any figure.</figcaption></div>
<section id="social-security-appendix"><h2>Appendix: Social Security</h2><p>Appendix text that must be kept.</p></section>
</article></main>
<footer>Copyright and links that are not part of the article at all.</footer>
</body></html>"##;
    let book = convert::convert(html, Url::parse("https://example.org/post/").unwrap()).unwrap();
    assert_eq!(book.title, "Notes on scaling");
    assert_eq!(book.authors, vec!["Ada Lovelace".to_string()]);
    assert_eq!(book.chapters.len(), 4, "title page + one chapter per h2");
    assert!(book.math_preamble.contains(r"\newcommand{\vx}"));
    let texs: Vec<&str> = book.math_jobs.iter().map(|j| j.tex.as_str()).collect();
    assert!(
        texs.contains(&r"L(n) \propto n^{-\alpha}")
            && texs.contains(&"x^2 + 1")
            && texs.contains(&r"\vx")
    );
    assert!(
        !texs
            .iter()
            .any(|t| t.contains("not math") || t.contains("newcommand") || t.contains("tag"))
    );

    let math = math::render_all(&book.math_jobs, &book.math_preamble).unwrap();
    assert!(
        math.iter()
            .all(|m| m.as_ref().is_ok_and(|m| !m.from_mathml)),
        "all TeX typesets, using the page's macro"
    );
    let laid = layout::layout(&book, &math, &[]);
    let mut out = String::new();
    for mut ch in book.chapters {
        tree::fill_slots(&mut ch.body, &|i| laid.filled[i].clone());
        tree::serialize(&ch.body, &mut out);
    }
    assert!(out.contains("costing $5 per run"));
    assert!(out.contains(r#"<span class="ltx_eqno">(2)</span>"#));
    assert!(out.contains("$not math$"));
    assert!(out.contains("Appendix text that must be kept"));
    assert!(out.contains(r#"<div class="ltx_caption">A stray caption"#));
    assert!(!out.contains("<figcaption>A stray"));
    for gone in ["Home", "hidden words", "Copyright", "Share this"] {
        assert!(!out.contains(gone), "{gone} should have been dropped");
    }
}
