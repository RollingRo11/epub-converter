//! EPUB 3 packaging, with an EPUB 2 NCX and `<meta name="cover">` for Kindle.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use anyhow::Result;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::convert::{Book, Chapter};
use crate::layout::Asset;
use crate::tree::{esc, serialize};

const CSS: &str = include_str!("style.css");

pub struct Meta {
    pub identifier: String,
    pub source_url: Option<String>,
    pub arxiv_id: Option<String>,
    pub modified: String,
}

fn xhtml_page(lang: &str, title: &str, body_class: Option<&str>, body: &str) -> String {
    let class = body_class
        .map(|c| format!(" class=\"{c}\""))
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{lang}\" xml:lang=\"{lang}\">\n\
         <head>\n<meta charset=\"utf-8\"/>\n<title>{title}</title>\n\
         <link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>\n</head>\n\
         <body{class}>\n{body}\n</body>\n</html>\n",
        lang = esc(lang),
        title = esc(title),
    )
}

fn chapter_xhtml(book: &Book, ch: &Chapter) -> String {
    let mut body = String::new();
    serialize(&ch.body, &mut body);
    if !ch.notes.is_empty() {
        body.push_str("\n<section class=\"footnotes\" epub:type=\"footnotes\">\n");
        for n in &ch.notes {
            serialize(std::slice::from_ref(n), &mut body);
            body.push('\n');
        }
        body.push_str("</section>");
    }
    let title = if ch.title.trim().is_empty() {
        &book.title
    } else {
        &ch.title
    };
    xhtml_page(&book.lang, title, None, &body)
}

struct Entry {
    level: u8,
    label: String,
    href: String,
}

fn toc_entries(book: &Book) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for ch in &book.chapters {
        for t in ch.toc.iter().filter(|t| !t.label.trim().is_empty()) {
            let href = match &t.id {
                Some(id) => format!("{}#{id}", ch.file),
                None => ch.file.clone(),
            };
            // Nesting may only deepen one level at a time.
            let prev = out.last().map_or(1, |e| e.level);
            let level = t.level.clamp(1, prev + 1);
            out.push(Entry {
                level,
                label: t.label.clone(),
                href,
            });
        }
    }
    out
}

fn nav_xhtml(book: &Book, entries: &[Entry], has_cover: bool) -> String {
    let mut s = String::from("<nav epub:type=\"toc\" id=\"toc\">\n<h1>Contents</h1>\n<ol>\n");
    let mut depth = 1u8;
    for (i, e) in entries.iter().enumerate() {
        while depth < e.level {
            s.push_str("<ol>\n");
            depth += 1;
        }
        let _ = write!(s, "<li><a href=\"{}\">{}</a>", esc(&e.href), esc(&e.label));
        let next = entries.get(i + 1).map_or(1, |n| n.level);
        if next > e.level {
            s.push('\n');
        } else {
            s.push_str("</li>\n");
            while depth > next {
                s.push_str("</ol></li>\n");
                depth -= 1;
            }
        }
    }
    s.push_str("</ol>\n</nav>\n");
    s.push_str("<nav epub:type=\"landmarks\" id=\"landmarks\" hidden=\"hidden\">\n<ol>\n");
    if has_cover {
        s.push_str("<li><a epub:type=\"cover\" href=\"cover.xhtml\">Cover</a></li>\n");
    }
    let _ = writeln!(
        s,
        "<li><a epub:type=\"bodymatter\" href=\"{}\">Start</a></li>",
        book.chapters[0].file
    );
    s.push_str("</ol>\n</nav>");
    xhtml_page(&book.lang, "Contents", None, &s)
}

fn ncx(book: &Book, meta: &Meta, entries: &[Entry]) -> String {
    let depth = entries.iter().map(|e| e.level).max().unwrap_or(1);
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\" xml:lang=\"{lang}\">\n\
         <head>\n<meta name=\"dtb:uid\" content=\"{uid}\"/>\n<meta name=\"dtb:depth\" content=\"{depth}\"/>\n\
         <meta name=\"dtb:totalPageCount\" content=\"0\"/>\n<meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n</head>\n\
         <docTitle><text>{title}</text></docTitle>\n<navMap>\n",
        lang = esc(&book.lang),
        uid = esc(&meta.identifier),
        title = esc(&book.title),
    );
    let mut open = 0u8;
    for (i, e) in entries.iter().enumerate() {
        while open >= e.level {
            s.push_str("</navPoint>\n");
            open -= 1;
        }
        let _ = writeln!(
            s,
            "<navPoint id=\"np{n}\" playOrder=\"{n}\"><navLabel><text>{label}</text></navLabel><content src=\"{src}\"/>",
            n = i + 1,
            label = esc(&e.label),
            src = esc(&e.href),
        );
        open = e.level;
    }
    while open > 0 {
        s.push_str("</navPoint>\n");
        open -= 1;
    }
    s.push_str("</navMap>\n</ncx>\n");
    s
}

fn opf(book: &Book, meta: &Meta, assets: &[Asset], has_cover: bool) -> String {
    let mut m = String::new();
    let _ = writeln!(
        m,
        "<dc:identifier id=\"pub-id\">{}</dc:identifier>",
        esc(&meta.identifier)
    );
    let _ = writeln!(m, "<dc:title>{}</dc:title>", esc(&book.title));
    for (i, a) in book.authors.iter().enumerate() {
        let _ = writeln!(m, "<dc:creator id=\"creator{i}\">{}</dc:creator>", esc(a));
    }
    let _ = writeln!(m, "<dc:language>{}</dc:language>", esc(&book.lang));
    if let Some(src) = &meta.source_url {
        let _ = writeln!(m, "<dc:source>{}</dc:source>", esc(src));
    }
    if meta.arxiv_id.is_some() {
        m.push_str("<dc:publisher>arXiv</dc:publisher>\n");
    }
    let _ = writeln!(
        m,
        "<meta property=\"dcterms:modified\">{}</meta>",
        meta.modified
    );
    if has_cover {
        m.push_str("<meta name=\"cover\" content=\"cover-image\"/>\n");
    }

    let mut items = String::new();
    items.push_str("<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n");
    items.push_str("<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n");
    items.push_str("<item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n");
    if has_cover {
        items.push_str("<item id=\"cover-image\" href=\"images/cover.png\" media-type=\"image/png\" properties=\"cover-image\"/>\n");
        items.push_str(
            "<item id=\"cover\" href=\"cover.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
        );
    }
    let mut spine = String::new();
    if has_cover {
        spine.push_str("<itemref idref=\"cover\" linear=\"yes\"/>\n");
    }
    for (i, ch) in book.chapters.iter().enumerate() {
        let _ = writeln!(
            items,
            "<item id=\"c{i}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>",
            esc(&ch.file)
        );
        let _ = writeln!(spine, "<itemref idref=\"c{i}\"/>");
    }
    for (i, a) in assets.iter().enumerate() {
        let _ = writeln!(
            items,
            "<item id=\"img{i}\" href=\"{}\" media-type=\"{}\"/>",
            esc(&a.path),
            a.media_type
        );
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"pub-id\" xml:lang=\"{lang}\">\n\
         <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n{m}</metadata>\n\
         <manifest>\n{items}</manifest>\n<spine toc=\"ncx\">\n{spine}</spine>\n\
         <guide>\n<reference type=\"text\" title=\"Start\" href=\"{start}\"/>\n</guide>\n</package>\n",
        lang = esc(&book.lang),
        start = esc(&book.chapters[0].file),
    )
}

pub fn write(
    path: &Path,
    book: &Book,
    meta: &Meta,
    assets: &[Asset],
    cover_png: Option<&[u8]>,
) -> Result<()> {
    let tmp = path.with_extension("epub.part");
    let file = std::fs::File::create(&tmp)?;
    let mut zip = ZipWriter::new(std::io::BufWriter::new(file));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // The mimetype must be first and uncompressed.
    zip.start_file("mimetype", stored)?;
    zip.write_all(b"application/epub+zip")?;

    zip.start_file("META-INF/container.xml", deflated)?;
    zip.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
          <container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
          <rootfiles>\n<rootfile full-path=\"EPUB/package.opf\" media-type=\"application/oebps-package+xml\"/>\n\
          </rootfiles>\n</container>\n",
    )?;

    let has_cover = cover_png.is_some();
    let entries = toc_entries(book);
    let mut text = |name: &str, body: &str| -> Result<()> {
        zip.start_file(format!("EPUB/{name}"), deflated)?;
        zip.write_all(body.as_bytes())?;
        Ok(())
    };
    text("package.opf", &opf(book, meta, assets, has_cover))?;
    text("nav.xhtml", &nav_xhtml(book, &entries, has_cover))?;
    text("toc.ncx", &ncx(book, meta, &entries))?;
    text("style.css", CSS)?;
    if has_cover {
        let body = format!(
            "<div class=\"cover\"><img src=\"images/cover.png\" alt=\"{}\"/></div>",
            esc(&book.title)
        );
        text(
            "cover.xhtml",
            &xhtml_page(&book.lang, "Cover", Some("cover"), &body),
        )?;
    }
    for ch in &book.chapters {
        text(&ch.file, &chapter_xhtml(book, ch))?;
    }
    if let Some(png) = cover_png {
        zip.start_file("EPUB/images/cover.png", stored)?;
        zip.write_all(png)?;
    }
    for a in assets {
        zip.start_file(format!("EPUB/{}", a.path), stored)?;
        zip.write_all(&a.data)?;
    }
    zip.finish()?.flush()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
