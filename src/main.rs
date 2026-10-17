use epub_converter::{browser, convert, cover, epub, layout, math, source, tree};

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;

/// Turn an arXiv paper or a web page with math into an EPUB for e-readers.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// arXiv id (2501.14926, 2501.14926v4, hep-th/9901001), arXiv link
    /// (abs/pdf/html), any web page link, or a local .html file.
    input: String,

    /// Output file. Defaults to <arxiv-id>.epub, or a name taken from the page address or file.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Base URL for resolving relative image paths (for saved HTML files).
    #[arg(long)]
    base_url: Option<String>,

    /// Don't generate a cover image.
    #[arg(long)]
    no_cover: bool,

    /// Only print errors.
    #[arg(short, long)]
    quiet: bool,

    /// Also list every equation whose TeX MathJax rejected (and why).
    #[arg(short, long)]
    verbose: bool,

    /// Always open web pages in a hidden browser and photograph their
    /// figures (normally only done for pages with interactive figures).
    #[arg(long, conflicts_with = "no_browser")]
    browser: bool,

    /// Never use a browser; convert the page's HTML only.
    #[arg(long)]
    no_browser: bool,

    /// The browser to use (any Chromium-based one: Chrome, Chromium, Brave, Edge).
    #[arg(long, value_name = "PATH")]
    browser_path: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(&args) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<()> {
    let started = Instant::now();
    let log = |msg: String| {
        if !args.quiet {
            eprintln!("{msg}");
        }
    };

    let input = source::parse_input(&args.input)?;
    log(format!("fetching {}", args.input));
    let (paper, metadata) = std::thread::scope(|s| {
        let meta = s.spawn(|| match &input {
            source::Input::Arxiv(id) => source::arxiv_metadata(id),
            _ => None,
        });
        let paper = source::fetch_paper(&input, args.base_url.as_deref());
        (paper, meta.join().ok().flatten())
    });
    let mut paper = paper?;

    // Interactive web pages: let a real browser draw the figures.
    let mut shots: Vec<Vec<u8>> = Vec::new();
    let is_web = paper.arxiv_id.is_none() && !convert::is_latexml(&paper.html);
    if is_web && !args.no_browser && (args.browser || convert::needs_browser(&paper.html)) {
        match browser::find_browser(args.browser_path.as_deref()) {
            Some(exe) => {
                log(format!(
                    "rendering the page in {} (hidden, with a temporary profile)",
                    exe.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                ));
                let detail = |msg: String| {
                    if args.verbose {
                        eprintln!("{msg}");
                    }
                };
                let rendered = browser::render(paper.base.as_str(), &exe, &log, &detail)?;
                paper.html = rendered.html;
                shots = rendered.shots;
            }
            None => eprintln!(
                "warning: this page's figures are drawn by JavaScript; install Chrome, Chromium, \
                 Brave or Edge (or pass --browser-path) to include them as pictures"
            ),
        }
    }

    let mut book =
        convert::convert_with(&paper.html, paper.base.clone(), &|u| source::fetch(u).ok())?;
    if let Some((title, authors)) = metadata {
        if !authors.is_empty() {
            book.authors = authors;
        }
        if !book.has_title {
            book.title = title;
            if let Some(first) = book.chapters.first_mut() {
                first.title = book.title.clone();
                if let Some(entry) = first.toc.first_mut() {
                    entry.label = book.title.clone();
                }
            }
        }
    }
    log(format!(
        "parsed \"{}\": {} chapters, {} equations, {} images",
        book.title,
        book.chapters.len(),
        book.math_jobs.len(),
        book.images.len()
    ));

    // Typeset equations while the figures download.
    let (math, fetched) = std::thread::scope(|s| {
        let images = &book.images;
        let shots = &shots;
        let downloads = s.spawn(move || {
            // Figure photos from browser mode are already in memory.
            let mut out = source::fetch_images(images);
            for (url, r) in images.iter().zip(out.iter_mut()) {
                if let Some(n) = url.strip_prefix("epubshot:") {
                    *r = n
                        .parse::<usize>()
                        .ok()
                        .and_then(|n| shots.get(n).cloned())
                        .ok_or_else(|| format!("missing figure photo {n}"));
                }
            }
            out
        });
        let math = math::render_all(&book.math_jobs, &book.math_preamble);
        (
            math,
            downloads.join().expect("image download thread panicked"),
        )
    });
    let math = math?;

    let via_mathml = math
        .iter()
        .filter(|m| m.as_ref().is_ok_and(|m| m.from_mathml))
        .count();
    let failed = math.iter().filter(|m| m.is_err()).count();
    if args.verbose {
        for (job, m) in book.math_jobs.iter().zip(&math) {
            if let Ok(m) = m
                && let Some(e) = &m.tex_error
            {
                eprintln!("note: MathML used for `{}`: {e}", job.tex);
            }
        }
    }

    let laid = layout::layout(&book, &math, &fetched);
    for ch in &mut book.chapters {
        tree::fill_slots(&mut ch.body, &|i| laid.filled[i].clone());
        tree::fill_slots(&mut ch.notes, &|i| laid.filled[i].clone());
    }

    let footer = match &paper.arxiv_id {
        Some(id) => format!("arXiv:{id}"),
        None => paper.base.host_str().unwrap_or("").to_string(),
    };
    let cover_png = if args.no_cover {
        None
    } else {
        match cover::render(&book.title, &book.authors, &footer) {
            Ok(png) => Some(png),
            Err(e) => {
                book.warnings.push(format!("cover not generated: {e}"));
                None
            }
        }
    };

    let identifier = match &paper.arxiv_id {
        Some(id) => format!("urn:arxiv:{id}"),
        None => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            paper.html.hash(&mut h);
            let a = h.finish();
            book.title.hash(&mut h);
            let b = h.finish();
            format!(
                "urn:uuid:{:08x}-{:04x}-5{:03x}-a{:03x}-{:012x}",
                a >> 32,
                (a >> 16) & 0xffff,
                a & 0xfff,
                (b >> 52) & 0xfff,
                b & 0xffff_ffff_ffff
            )
        }
    };
    let modified = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map(|s| format!("{}Z", &s[..19]))
        .context("formatting time")?;
    let meta = epub::Meta {
        identifier,
        source_url: paper
            .arxiv_id
            .as_ref()
            .map(|id| format!("https://arxiv.org/abs/{id}"))
            .or_else(|| Some(paper.base.to_string())),
        arxiv_id: paper.arxiv_id.clone(),
        modified,
    };

    let output = match &args.output {
        Some(o) => o.clone(),
        None => {
            let stem = match (&paper.arxiv_id, &input) {
                (Some(id), _) => id.replace('/', "_"),
                (None, source::Input::File(p)) => p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or("paper".into()),
                _ => page_stem(&paper.base),
            };
            PathBuf::from(format!("{stem}.epub"))
        }
    };
    epub::write(&output, &book, &meta, &laid.assets, cover_png.as_deref())?;

    for w in book.warnings.iter().chain(&laid.warnings) {
        eprintln!("warning: {w}");
    }
    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    log(format!(
        "wrote {} ({:.1} MB) in {:.1}s: {} equations ({} via MathML fallback, {} kept as TeX), {} images",
        output.display(),
        size as f64 / 1e6,
        started.elapsed().as_secs_f64(),
        math.len(),
        via_mathml,
        failed,
        laid.assets.len(),
    ));
    Ok(())
}

/// A file name for a web page: its last meaningful URL path segment
/// (".../interference_effectiveness_helpfulness/index.html" -> that folder name).
fn page_stem(url: &url::Url) -> String {
    let segment = url
        .path_segments()
        .into_iter()
        .flatten()
        .rfind(|s| !s.is_empty() && !s.starts_with("index."))
        .map(|s| s.rsplit_once('.').map_or(s, |(stem, _)| stem).to_string())
        .or_else(|| url.host_str().map(str::to_string))
        .unwrap_or_default();
    let clean: String = segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.trim_matches(['_', '.']).is_empty() {
        "page".into()
    } else {
        clean
    }
}
