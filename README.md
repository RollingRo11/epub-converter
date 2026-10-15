# epub-converter

Turn arXiv papers and math-heavy web pages into clean EPUB books for e-readers
(Kindle, Kobo, KOReader, Apple Books, ...).

Equations, figures, tables, footnotes, lists and links all carry over. The text
reflows to your screen and font size instead of being stuck in a PDF page.

```bash
epub-converter 2501.14926                            # an arXiv paper
epub-converter https://ericjmichaud.com/quanta/      # a blog post with math
```

It is a single program written in Rust. A typical paper takes 1–3 seconds,
mostly spent downloading figures.

---

## Contents

- [Install](#install)
- [Usage](#usage)
- [Reading on your device](#reading-on-your-device)
- [arXiv papers](#arxiv-papers)
- [Web pages](#web-pages)
- [How equations are drawn](#how-equations-are-drawn)
- [How it works](#how-it-works)
- [Development](#development)
- [Known limits](#known-limits)
- [Credits and license](#credits-and-license)

---

## Install

You need Rust. If you don't have it, install it from [rustup.rs](https://rustup.rs).
Then:

```bash
git clone https://github.com/RollingRo11/epub-converter
cd epub-converter
cargo install --path . --locked
```

This puts the `epub-converter` command on your PATH. Nothing else is needed:
MathJax (the program that draws equations) is built into the binary, so you
don't need Node.js, a browser or LaTeX.

To update later, `git pull` and run the same `cargo install` line again.

---

## Usage

```bash
epub-converter <paper or page> [options]
```

The input can be:

| Input | Example |
|---|---|
| arXiv id | `2501.14926`, `2501.14926v4`, `arXiv:2501.14926`, `hep-th/9901001` |
| arXiv link | `https://arxiv.org/abs/2501.14926` (also `/pdf/` and `/html/` links) |
| arXiv DOI | `10.48550/arXiv.2501.14926` |
| Any web page | `https://transformer-circuits.pub/2021/framework/index.html` |
| A saved HTML file | `paper.html` |

Without a version number, you get the latest version of an arXiv paper.

The EPUB is written to the folder you run the command from. It's named after
the arXiv id (`2501.14926v4.epub`) or the page address
(`.../quanta/` → `quanta.epub`).

### Options

| Option | What it does |
|---|---|
| `-o, --output <file>` | Where to write the EPUB, e.g. `-o ~/Books/paper.epub` |
| `--no-cover` | Don't generate a cover image |
| `--base-url <url>` | For saved HTML files: where their images live online |
| `-v, --verbose` | Also list equations that needed a fallback (see below) |
| `-q, --quiet` | Only print errors |
| `-h, --help` | Show help |

### What it prints

```
fetching 2501.14926v4
parsed "Interpretability in Parameter Space: ...": 14 chapters, 289 equations, 25 images
wrote 2501.14926v4.epub (6.6 MB) in 1.8s: 289 equations (0 via MathML fallback, 0 kept as TeX), 296 images
```

- **via MathML fallback**: equations whose LaTeX used the author's own macros.
  They were still drawn, from the expanded MathML version. Fine.
- **kept as TeX**: equations that could not be drawn at all, so they show as
  LaTeX text. This should be 0. If it isn't, please open an issue with the link.
- **warning:** lines explain anything that was left out (for example, a figure
  missing from arXiv's own HTML).

---

## Reading on your device

**Kindle.** Send the `.epub` with [Send to Kindle](https://www.amazon.com/sendtokindle)
or email it to your `@kindle.com` address. Amazon converts it for your device.

**Kobo, Apple Books, KOReader, other readers.** Copy the `.epub` over as you
would any other book.

**Supernote, and other devices with a weak built-in reader.** Some built-in
readers support only part of the styling EPUBs use. On a Supernote, the built-in
reader draws inline equations slightly too high, puts equation numbers in odd
places and breaks tables. [KOReader](https://koreader.rocks) shows the same
files correctly, so use it if your device can run it.

> **KOReader tip:** to make a setting (font, size, margins) the default for every
> book, long-press it in the menu and confirm "set as default". Books you have
> already opened keep their own settings until you reset them (long-press the
> book in the file browser → Reset).

**Testing a new device.** `cargo run --release --example device_test` builds
`reader-test.epub`, a short book showing equations, equation numbers, tables
and figures laid out in several ways (A, B, C...). Open it on the device to see
which ways it displays correctly.

---

## arXiv papers

arXiv publishes an HTML version of most papers, made from the LaTeX source by a
tool called [LaTeXML](https://math.nist.gov/~BMiller/LaTeXML/). epub-converter
reads that HTML. If arXiv has none, it tries [ar5iv](https://ar5iv.labs.arxiv.org).

What carries over:

- **Title page**: title, authors (from arXiv's official records), affiliations,
  abstract and `\thanks` notes.
- **Chapters**: one per section, with subsections in the table of contents.
- **Equations**: inline and display, with their numbers. In multi-line
  `align` equations the `=` signs stay lined up (see below).
- **Figures**: PNG/JPEG as they are. SVG figures and TikZ drawings are turned
  into images, with TikZ labels (text and math) drawn in.
- **Tables**, with their rules (`\toprule`, `\midrule`, ...).
- **Lists**: real bullet and numbered lists; custom labels like (a) or (i) are kept.
- **Footnotes**: pop-up footnotes where the reader supports them, and listed at
  the end of each chapter.
- **Links**: citations, and references to equations, figures and sections.
- **Theorems, proofs, algorithms, code listings, bibliography.**
- **Cover**: a plain cover with the title and authors.

---

## Web pages

Any link that isn't an arXiv paper is treated as a general web page:

- **Finding the article.** It looks for where most of the paragraph text is, and
  leaves out menus, headers, footers, sidebars, share buttons and comment sections.
- **Finding the math.** Many sites write LaTeX straight into the text, like
  `$x^2$`, and draw it in the browser with KaTeX or MathJax. epub-converter
  reads which markers the page itself uses (`$...$`, `\(...\)`, `$$...$$`,
  `\begin{equation}`...) and the page's own macros. So text counts as math
  exactly where it does on the site, and a plain dollar sign stays a dollar sign.
- **Math that is already drawn** is converted from its LaTeX source too:
  KaTeX and MathJax output, MathML, Distill's `<d-math>`, and equation images
  that carry their LaTeX as a description (WordPress, Wikipedia).
- **Chapters** are split at the page's section headings.
- **Distill articles** (distill.pub, transformer-circuits.pub) also get their
  footnotes, appendix, author list and a numbered reference list built from the
  page's bibliography.

Tested on ericjmichaud.com/quanta, transformer-circuits.pub, Lilian Weng's blog,
colah's blog, Karpathy's blog, Terence Tao's blog, Gregory Gundersen's blog,
The Illustrated Transformer and Wikipedia. Every site is built differently, so
an unusual layout may still need a fix. Please open an issue with the link.

---

## How equations are drawn

E-readers handle math very unevenly, so every equation becomes a small image.
The method follows [md2kindle](https://github.com/ARahim3/md2kindle):

1. Take the equation's LaTeX source.
2. Draw it with MathJax (LaTeX → SVG).
3. Turn the SVG into a PNG at high resolution, so it stays sharp on 300 ppi screens.
4. Size the image in `em` (units of the reader's font size), so equations grow
   and shrink with your font, and shift inline equations down so they sit on
   the text line.

Additions on top of that:

- **Author macros.** If MathJax doesn't know a macro (an author's own
  `\newcommand`), the equation is drawn from LaTeXML's MathML instead, where those
  macros are already expanded. On web pages, the page's own macro definitions
  are applied to every equation.
- **Aligned equations.** Rows of an `align` group are drawn with the same column
  widths, so the `=` signs line up even though each row is its own image.
- **Equation numbers** are kept next to their equation.

---

## How it works

```
input (arXiv id / link / file)
  │
  ├─ source.rs       fetch the HTML (arXiv, ar5iv, any URL, or a file)
  ├─ convert.rs      arXiv/LaTeXML HTML → chapters; equations and images
  │   └─ web.rs      become placeholders. web.rs does the same for any
  │                  other web page.
  ├─ math.rs         MathJax inside QuickJS (an embedded JavaScript engine),
  │                  one copy per CPU core, all equations in parallel
  │                  (figures download at the same time)
  ├─ layout.rs       placeholders → images: equation sizing, aligned rows,
  │   └─ raster.rs   SVG → PNG with resvg
  ├─ tree.rs         makes the XHTML always valid: correct nesting, escaping,
  │                  unique ids
  ├─ cover.rs        the cover image
  └─ epub.rs         EPUB 3 package, plus an EPUB 2 table of contents for
                     older readers
```

---

## Development

```bash
cargo build --release      # binary at target/release/epub-converter
cargo test --release       # tests (offline)
```

The tests include two full end-to-end conversions of small made-up pages, one
arXiv-style and one blog-style. They check equations, lists, footnotes, links,
picture labels and well-formed output.

**The MathJax bundle.** `assets/mathjax.js` is MathJax 3.2.2, bundled from
`mathjax/entry.js` and committed, so building needs no Node.js. To rebuild it
after editing `entry.js`:

```bash
cd mathjax && npm install && npm run build
```

**How releases were checked.** Generated books were validated with the official
EPUB checker, [EPUBCheck](https://www.w3.org/publishing/epubcheck/): 0 errors and
0 warnings on 21 arXiv papers and 10 web pages. Each book's text was also
compared word by word against the source page, to make sure nothing was dropped.

---

## Known limits

- **No HTML version, no EPUB.** Some arXiv papers only exist as PDFs
  (non-LaTeX submissions, or arXiv's conversion failed). The tool says so.
  Send the PDF to your device instead.
- **Equation images have transparent backgrounds** (as in md2kindle). Fine on
  e-ink; in a phone or tablet app with a dark theme they can be hard to see.
- **Wide tables stay tables.** On small screens you may need to pan or zoom.
- **Web pages vary.** Pages built by JavaScript after loading (where the text
  isn't in the HTML at all) can't be converted.

---

## Credits and license

- Equation method from [md2kindle](https://github.com/ARahim3/md2kindle) by ARahim3.
- [MathJax](https://www.mathjax.org) draws the equations (Apache 2.0, see
  `assets/MATHJAX-LICENSE`); it runs inside [QuickJS](https://github.com/quickjs-ng/quickjs)
  via [rquickjs](https://github.com/DelSkayn/rquickjs).
- [resvg](https://github.com/linebender/resvg) turns SVG into PNG.
- arXiv's HTML papers are made by [LaTeXML](https://math.nist.gov/~BMiller/LaTeXML/).

epub-converter is MIT licensed (see `LICENSE`).
