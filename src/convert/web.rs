//! General web pages (blogs, Distill articles, Wikipedia, ...).
//!
//! Unlike LaTeXML papers there is no fixed structure, so:
//! * the article is found by where the paragraph text is (as Readability does);
//! * math is found the way the page's own renderer would find it: the
//!   delimiters come from the page's KaTeX / MathJax settings, and TeX written
//!   in the text is cut out of text nodes. Pre-rendered math (KaTeX, MathJax,
//!   MathML, Distill `<d-math>`, LaTeX images with TeX alt text) is used too;
//! * chapters are split at the page's top section headings.

use std::collections::HashMap;

use anyhow::Result;
use ego_tree::NodeId;
use scraper::{ElementRef, Html};
use url::Url;

use super::{
    Book, Chapter, Conv, Fetch, Slot, TocEntry, collapse_ws, has, plain_text, resolve_links, sel,
};
use crate::tree::{El, Node, normalize, sanitize_id, walk, walk_mut};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Delim {
    left: String,
    right: String,
    display: bool,
    /// `\begin{env}...\end{env}`: the delimiters are part of the TeX.
    env: bool,
}

pub(super) struct WebCtx {
    delims: Vec<Delim>,
    /// `\$` is a literal dollar sign rather than a delimiter.
    escapes: bool,
    /// Depth inside elements whose text is never math (pre, code, ...).
    skip: u32,
    /// Macro definitions found in the page's math.
    definitions: Vec<String>,
    bib: HashMap<String, HashMap<String, String>>,
    cite_order: Vec<String>,
}

const ENVIRONMENTS: &[&str] = &[
    "equation",
    "equation*",
    "align",
    "align*",
    "alignat",
    "alignat*",
    "gather",
    "gather*",
    "multline",
    "multline*",
    "flalign",
    "flalign*",
    "eqnarray",
    "eqnarray*",
    "displaymath",
    "CD",
];

/// Class / id words that mark page furniture rather than article content.
const CHROME_WORDS: &[&str] = &[
    "comments",
    "commentlist",
    "respond",
    "share",
    "sharing",
    "social",
    "related",
    "navbar",
    "navbox",
    "subscribe",
    "newsletter",
    "sidebar",
    "widget",
    "breadcrumb",
    "breadcrumbs",
    "toc",
    "menu",
    "pagination",
    "advert",
    "ads",
    "promo",
    "cookie",
    "editsection",
    "noprint",
    "printfooter",
    "catlinks",
    "skip",
    "jump",
];

/// Shortcuts KaTeX has built in but MathJax lacks, for pages drawn by KaTeX.
const KATEX_BUILTINS: &[(&str, &str)] = &[
    // Number sets
    ("R", r"\mathbb{R}"), ("N", r"\mathbb{N}"), ("Z", r"\mathbb{Z}"), ("Q", r"\mathbb{Q}"),
    ("C", r"\mathbb{C}"), ("Reals", r"\mathbb{R}"), ("reals", r"\mathbb{R}"),
    ("Complex", r"\mathbb{C}"), ("cnums", r"\mathbb{C}"), ("natnums", r"\mathbb{N}"),
    // Greek capitals that look like Latin letters
    ("Alpha", r"\mathrm{A}"), ("Beta", r"\mathrm{B}"), ("Epsilon", r"\mathrm{E}"),
    ("Zeta", r"\mathrm{Z}"), ("Eta", r"\mathrm{H}"), ("Iota", r"\mathrm{I}"),
    ("Kappa", r"\mathrm{K}"), ("Mu", r"\mathrm{M}"), ("Nu", r"\mathrm{N}"),
    ("Omicron", r"\mathrm{O}"), ("Rho", r"\mathrm{P}"), ("Tau", r"\mathrm{T}"),
    ("Chi", r"\mathrm{X}"), ("omicron", "o"), ("thetasym", r"\vartheta"),
    // HTML-style names
    ("lang", r"\langle"), ("rang", r"\rangle"), ("larr", r"\leftarrow"),
    ("rarr", r"\rightarrow"), ("Larr", r"\Leftarrow"), ("lArr", r"\Leftarrow"),
    ("Rarr", r"\Rightarrow"), ("rArr", r"\Rightarrow"), ("harr", r"\leftrightarrow"),
    ("lrarr", r"\leftrightarrow"), ("Harr", r"\Leftrightarrow"), ("hArr", r"\Leftrightarrow"),
    ("Lrarr", r"\Leftrightarrow"), ("lrArr", r"\Leftrightarrow"), ("uarr", r"\uparrow"),
    ("darr", r"\downarrow"), ("Uarr", r"\Uparrow"), ("uArr", r"\Uparrow"),
    ("Darr", r"\Downarrow"), ("dArr", r"\Downarrow"), ("empty", r"\emptyset"),
    ("infin", r"\infty"), ("isin", r"\in"), ("sub", r"\subset"), ("sube", r"\subseteq"),
    ("supe", r"\supseteq"), ("real", r"\Re"), ("image", r"\Im"), ("alef", r"\aleph"),
    ("alefsym", r"\aleph"), ("exist", r"\exists"), ("sdot", r"\cdot"), ("plusmn", r"\pm"),
    ("sect", r"\S"), ("bull", r"\bullet"), ("Dagger", r"\ddagger"), ("weierp", r"\wp"),
    ("clubs", r"\clubsuit"), ("diamonds", r"\diamondsuit"), ("hearts", r"\heartsuit"),
    ("spades", r"\spadesuit"),
];

// ---------------------------------------------------------------------------
// Reading the page's math settings (JavaScript object literals, loosely).

/// All string literals in a JS snippet, unescaped.
fn js_strings(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if !matches!(c, '"' | '\'' | '`') {
            continue;
        }
        let mut lit = String::new();
        while let Some(d) = chars.next() {
            match d {
                '\\' => match chars.next() {
                    Some('n') => lit.push('\n'),
                    Some('t') => lit.push('\t'),
                    Some(e) => lit.push(e),
                    None => break,
                },
                d if d == c => break,
                d => lit.push(d),
            }
        }
        out.push(lit);
    }
    out
}

/// The bracketed/braced group starting at the first `open` at or after `from`.
fn group(s: &str, from: usize, open: char, close: char) -> Option<&str> {
    let start = from + s[from..].find(open)?;
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s[start..].char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=start + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// `key: [['$','$'], ['\\(','\\)']]` (MathJax) -> pairs.
fn mathjax_pairs(js: &str, key: &str, display: bool) -> Vec<Delim> {
    let mut out = Vec::new();
    for (pos, _) in js.match_indices(key) {
        let after = &js[pos + key.len()..];
        if !after.trim_start().starts_with(':') {
            continue;
        }
        if let Some(arr) = group(js, pos, '[', ']') {
            let strs = js_strings(arr);
            for pair in strs.chunks(2) {
                if let [l, r] = pair
                    && !l.is_empty()
                    && !r.is_empty()
                {
                    out.push(Delim {
                        left: l.clone(),
                        right: r.clone(),
                        display,
                        env: false,
                    });
                }
            }
        }
    }
    out
}

/// `delimiters: [{left: "$$", right: "$$", display: true}, ...]` (KaTeX).
fn katex_delims(js: &str) -> Vec<Delim> {
    let mut out = Vec::new();
    for (pos, _) in js.match_indices("delimiters") {
        let Some(arr) = group(js, pos, '[', ']') else {
            continue;
        };
        let mut i = 0;
        while let Some(obj) = arr.get(i..).and_then(|rest| group(rest, 0, '{', '}')) {
            let start = arr[i..].find('{').unwrap() + i;
            i = start + obj.len();
            let field = |name: &str| -> Option<String> {
                let at = obj.find(name)?;
                js_strings(&obj[at + name.len()..]).into_iter().next()
            };
            let (Some(left), Some(right)) = (field("left"), field("right")) else {
                continue;
            };
            let display = obj
                .find("display")
                .map(|at| {
                    obj[at..]
                        .trim_start_matches("display")
                        .trim_start_matches([' ', ':'])
                        .starts_with("true")
                })
                .unwrap_or(false);
            if !left.is_empty() && !right.is_empty() {
                out.push(Delim {
                    left,
                    right,
                    display,
                    env: false,
                });
            }
        }
    }
    out
}

/// `macros: { "\\RR": "\\mathbb{R}", bold: ["{\\bf #1}", 1] }` -> TeX definitions.
fn config_macros(js: &str) -> String {
    let mut out = String::new();
    for key in ["macros", "Macros"] {
        for (pos, _) in js.match_indices(key) {
            if !js[pos + key.len()..].trim_start().starts_with(':') {
                continue;
            }
            let Some(obj) = group(js, pos, '{', '}') else {
                continue;
            };
            let inner = &obj[1..obj.len() - 1];
            // Split top-level entries on commas outside brackets and strings.
            let mut entries = Vec::new();
            let (mut depth, mut quote, mut esc, mut start) = (0i32, None::<char>, false, 0);
            for (i, c) in inner.char_indices() {
                if let Some(q) = quote {
                    if esc {
                        esc = false;
                    } else if c == '\\' {
                        esc = true;
                    } else if c == q {
                        quote = None;
                    }
                    continue;
                }
                match c {
                    '"' | '\'' | '`' => quote = Some(c),
                    '[' | '{' => depth += 1,
                    ']' | '}' => depth -= 1,
                    ',' if depth == 0 => {
                        entries.push(&inner[start..i]);
                        start = i + 1;
                    }
                    _ => {}
                }
            }
            entries.push(&inner[start..]);
            for e in entries {
                let Some((k, v)) = e.split_once(':') else {
                    continue;
                };
                let k = k.trim();
                let name = js_strings(k)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| k.to_string());
                let name = name.trim_start_matches('\\');
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphabetic()) {
                    continue;
                }
                let strs = js_strings(v);
                let Some(body) = strs.first() else { continue };
                let nargs = v
                    .rsplit(',')
                    .next()
                    .and_then(|n| n.trim().trim_end_matches(']').trim().parse::<u8>().ok())
                    .filter(|_| v.trim_start().starts_with('['));
                match nargs {
                    Some(n) if n > 0 => {
                        out.push_str(&format!("\\newcommand{{\\{name}}}[{n}]{{{body}}}"))
                    }
                    _ => out.push_str(&format!("\\def\\{name}{{{body}}}")),
                }
            }
        }
    }
    out
}

struct MathSetup {
    delims: Vec<Delim>,
    escapes: bool,
    katex: bool,
    macros: String,
}

fn math_setup(doc: &Html) -> MathSetup {
    let mut js = String::new();
    let mut srcs = String::new();
    for s in doc.select(&sel("script")) {
        match s.value().attr("src") {
            Some(src) => {
                srcs.push_str(src);
                srcs.push('\n');
            }
            None => {
                js.push_str(&s.text().collect::<String>());
                js.push('\n');
            }
        }
    }
    for l in doc.select(&sel("link[href]")) {
        srcs.push_str(l.value().attr("href").unwrap_or(""));
        srcs.push('\n');
    }
    let srcs = srcs.to_ascii_lowercase();
    // Distill's <d-math> is drawn by KaTeX too.
    let katex = srcs.contains("katex")
        || js.contains("renderMathInElement")
        || js.contains("katex.render")
        || doc.select(&sel("d-math")).next().is_some();
    let mathjax = srcs.contains("mathjax") || js.contains("MathJax");

    let mut delims = Vec::new();
    if js.contains("renderMathInElement") {
        delims.extend(katex_delims(&js));
    }
    if mathjax {
        delims.extend(mathjax_pairs(&js, "inlineMath", false));
        delims.extend(mathjax_pairs(&js, "displayMath", true));
    }
    if delims.is_empty() && (katex || mathjax) {
        // Both libraries' defaults: no single `$`.
        for (l, r, d) in [
            ("$$", "$$", true),
            ("\\[", "\\]", true),
            ("\\(", "\\)", false),
        ] {
            delims.push(Delim {
                left: l.into(),
                right: r.into(),
                display: d,
                env: false,
            });
        }
    }
    if katex || mathjax {
        for env in ENVIRONMENTS {
            delims.push(Delim {
                left: format!("\\begin{{{env}}}"),
                right: format!("\\end{{{env}}}"),
                display: true,
                env: true,
            });
        }
    }
    let mut seen = Vec::new();
    delims.retain(|d| {
        let key = (d.left.clone(), d.right.clone());
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    // Longest opening delimiter first, so `$$` wins over `$`.
    delims.sort_by_key(|d| std::cmp::Reverse(d.left.len()));

    let escapes = delims.iter().any(|d| d.left == "$");
    let mut macros = config_macros(&js);
    if katex {
        let builtins: String = KATEX_BUILTINS.iter().map(|(name, body)| format!("\\def\\{name}{{{body}}}")).collect();
        macros.insert_str(0, &builtins);
    }
    MathSetup {
        delims,
        escapes,
        katex,
        macros,
    }
}

// ---------------------------------------------------------------------------
// Finding math in text.

#[derive(Debug, PartialEq)]
pub(super) enum Piece {
    Text(String),
    Math { tex: String, display: bool },
}

/// End of math starting at `start`: the next `right` outside braces, skipping
/// backslash-escaped characters (KaTeX's and MathJax's rule).
fn find_end(text: &str, start: usize, right: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut j = start;
    while j < text.len() {
        if depth <= 0 && text[j..].starts_with(right) {
            return Some(j);
        }
        let c = text[j..].chars().next()?;
        j += c.len_utf8();
        match c {
            '\\' => {
                if let Some(n) = text[j..].chars().next() {
                    j += n.len_utf8();
                }
            }
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

pub(super) fn split_math(text: &str, delims: &[Delim], escapes: bool) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    'outer: while i < text.len() {
        let rest = &text[i..];
        if escapes && rest.starts_with("\\$") {
            buf.push('$');
            i += 2;
            continue;
        }
        for d in delims {
            if !rest.starts_with(&d.left) {
                continue;
            }
            let start = i + d.left.len();
            if let Some(end) = find_end(text, start, &d.right) {
                let inner = &text[start..end];
                if !inner.trim().is_empty() {
                    let tex = if d.env {
                        format!("{}{inner}{}", d.left, d.right)
                    } else {
                        inner.trim().to_string()
                    };
                    if !buf.is_empty() {
                        out.push(Piece::Text(std::mem::take(&mut buf)));
                    }
                    out.push(Piece::Math {
                        tex,
                        display: d.display,
                    });
                    i = end + d.right.len();
                    continue 'outer;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        buf.push(c);
        i += c.len_utf8();
    }
    if !buf.is_empty() {
        out.push(Piece::Text(buf));
    }
    out
}

/// `\newcommand{\x}[1]{...}`, `\def\x#1{...}`, `\DeclareMathOperator{\x}{...}`,
/// `\let\a\b` at the start of `tex`: the remaining TeX after them.
fn strip_definitions(tex: &str) -> &str {
    let mut s = tex.trim_start();
    loop {
        let before = s;
        for cmd in [
            "\\newcommand",
            "\\renewcommand",
            "\\providecommand",
            "\\DeclareMathOperator",
            "\\def",
            "\\gdef",
            "\\let",
        ] {
            let Some(rest) = s.strip_prefix(cmd) else {
                continue;
            };
            let mut r = rest.trim_start().trim_start_matches('*').trim_start();
            if cmd == "\\let" {
                // \let\a\b or \let\a=\b
                let mut n = 0;
                while n < 2 && r.starts_with('\\') {
                    let len = 1 + r[1..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphabetic())
                        .count()
                        .max(1);
                    r = r[len..].trim_start().trim_start_matches('=').trim_start();
                    n += 1;
                }
                s = r;
                break;
            }
            // The name: {\x} or \x
            if r.starts_with('{') {
                let Some(g) = group(r, 0, '{', '}') else {
                    return tex;
                };
                r = &r[g.len()..];
            } else if r.starts_with('\\') {
                let len = 1 + r[1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .count()
                    .max(1);
                r = &r[len..];
            } else {
                return tex;
            }
            // Arguments: [n][default] or #1#2
            loop {
                let t = r.trim_start();
                if t.starts_with('[') {
                    let Some(end) = t.find(']') else { return tex };
                    r = &t[end + 1..];
                } else if t.starts_with('#') {
                    r = &t[2.min(t.len())..];
                } else {
                    r = t;
                    break;
                }
            }
            let Some(body) = group(r, 0, '{', '}').filter(|_| r.starts_with('{')) else {
                return tex;
            };
            s = r[body.len()..].trim_start();
            break;
        }
        if s.len() == before.len() {
            return s;
        }
    }
}

/// Pull `\tag{...}` out of display TeX; returns the TeX and the number text.
fn take_tag(tex: &str) -> (String, Option<String>) {
    for (cmd, wrap) in [("\\tag*", false), ("\\tag", true)] {
        if let Some(pos) = tex.find(cmd) {
            let after = &tex[pos + cmd.len()..];
            if let Some(g) =
                group(after, 0, '{', '}').filter(|_| after.trim_start().starts_with('{'))
            {
                let off = after.find('{').unwrap();
                let label = g[1..g.len() - 1].trim().to_string();
                let rest = format!("{}{}", &tex[..pos], &after[off + g.len()..]);
                let label = if wrap { format!("({label})") } else { label };
                return (rest, Some(label));
            }
        }
    }
    (tex.to_string(), None)
}

// ---------------------------------------------------------------------------
// Conversion hooks.

fn classes_and_id(el: &ElementRef) -> impl Iterator<Item = String> {
    let id = el.value().attr("id").unwrap_or("").to_ascii_lowercase();
    let classes: Vec<String> = el
        .value()
        .classes()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    classes.into_iter().chain(std::iter::once(id))
}

fn is_chrome(el: &ElementRef) -> bool {
    if el.value().attr("role").is_some_and(|r| {
        matches!(
            r,
            "navigation" | "banner" | "contentinfo" | "search" | "complementary"
        )
    }) {
        return true;
    }
    classes_and_id(el).any(|name| name.split(['-', '_']).any(|w| CHROME_WORDS.contains(&w)))
}

fn is_hidden(el: &ElementRef) -> bool {
    let v = el.value();
    if v.attr("hidden").is_some() || v.attr("aria-hidden") == Some("true") {
        return true;
    }
    v.attr("style").is_some_and(|s| {
        let s: String = s
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        s.contains("display:none") || s.contains("visibility:hidden")
    })
}

/// Is this `<img>` a picture of an equation whose alt text is the TeX?
fn latex_image(el: &ElementRef) -> Option<(String, bool)> {
    let v = el.value();
    let alt = v.attr("alt")?.trim();
    if alt.is_empty() {
        return None;
    }
    let src = v.attr("src").unwrap_or("");
    let classes: Vec<&str> = v.classes().collect();
    let is_latex = classes
        .iter()
        .any(|c| *c == "latex" || c.starts_with("mwe-math-fallback-image"))
        || src.contains("latex.php")
        || src.contains("latex.codecogs.com")
        || src.contains("chart.apis.google.com/chart?cht=tx");
    if !is_latex {
        return None;
    }
    let display = classes.iter().any(|c| c.contains("display"));
    Some((alt.to_string(), display))
}

/// The real image URL of an `<img>`, looking past lazy-loading placeholders.
fn image_url(el: &ElementRef) -> Option<String> {
    let v = el.value();
    let src = v.attr("src").map(str::trim).filter(|s| !s.is_empty());
    let placeholder = src.is_none_or(|s| {
        (s.starts_with("data:") && s.len() < 400)
            || s.contains("placeholder")
            || s.contains("blank.gif")
    });
    if !placeholder {
        return src.map(str::to_string);
    }
    for a in [
        "data-src",
        "data-lazy-src",
        "data-original",
        "data-srcset",
        "srcset",
    ] {
        if let Some(u) = v
            .attr(a)
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.split_whitespace().next())
            && !u.is_empty()
        {
            return Some(u.to_string());
        }
    }
    src.map(str::to_string)
}

impl Conv {
    fn web(&mut self) -> &mut WebCtx {
        self.web.as_mut().expect("web mode")
    }

    /// Math from a web page: definitions go to the preamble, `\tag` becomes a
    /// visible number.
    fn web_math(&mut self, tex: &str, display: bool, mathml: Option<String>) -> Vec<Node> {
        let tex = tex.trim();
        if tex.is_empty() {
            return vec![];
        }
        if strip_definitions(tex).trim().is_empty() {
            self.web().definitions.push(tex.to_string());
            return vec![];
        }
        let (tex, tag) = if display {
            take_tag(tex)
        } else {
            (tex.to_string(), None)
        };
        let job = self.math_job(tex, display, mathml);
        let slot = self.slot(Slot::Math { job, display });
        if !display {
            return vec![slot];
        }
        let mut div = El::new("div").class("ltx_eq");
        if let Some(t) = tag {
            div.children
                .push(El::new("span").class("ltx_eqno").text(t).into());
        }
        div.children.push(slot);
        vec![div.into()]
    }

    pub(super) fn web_text(&mut self, text: &str) -> Vec<Node> {
        let ctx = self.web.as_ref().unwrap();
        if ctx.skip > 0 || ctx.delims.is_empty() {
            return vec![Node::text(text)];
        }
        let pieces = split_math(text, &ctx.delims, ctx.escapes);
        let mut out = Vec::new();
        for p in pieces {
            match p {
                Piece::Text(t) => out.push(Node::Text(t)),
                Piece::Math { tex, display } => out.extend(self.web_math(&tex, display, None)),
            }
        }
        out
    }

    fn web_note(&mut self, body: Vec<Node>) -> Vec<Node> {
        self.note_seq += 1;
        let mark = self.note_seq.to_string();
        let id = self.fresh_id("fn");
        let ref_id = self.fresh_id("fnref");
        let mut p = El::new("p")
            .child(
                El::new("a")
                    .class("fn-back")
                    .attr("href", format!("#{ref_id}"))
                    .text(mark.clone()),
            )
            .text(" ");
        p.children.extend(super::trim_nodes(body));
        self.notes.push(
            El::new("aside")
                .attr("epub:type", "footnote")
                .attr("id", id.clone())
                .class("footnote")
                .child(p)
                .into(),
        );
        vec![
            El::new("a")
                .attr("epub:type", "noteref")
                .class("noteref")
                .attr("id", ref_id)
                .attr("href", format!("#{id}"))
                .child(El::new("sup").text(mark))
                .into(),
        ]
    }

    fn web_cite(&mut self, keys: &str) -> Vec<Node> {
        let mut span = El::new("span").class("ltx_cite").text("[");
        for (n, key) in keys
            .split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .enumerate()
        {
            let ctx = self.web();
            let num = match ctx.cite_order.iter().position(|k| k == key) {
                Some(i) => i + 1,
                None => {
                    ctx.cite_order.push(key.to_string());
                    ctx.cite_order.len()
                }
            };
            if n > 0 {
                span.children.push(Node::text(", "));
            }
            span.children.push(
                El::new("a")
                    .class("ltx_ref")
                    .attr("href", format!("#{}", sanitize_id(&format!("ref-{key}"))))
                    .text(num.to_string())
                    .into(),
            );
        }
        span.children.push(Node::text("]"));
        vec![span.into()]
    }

    /// Web-specific handling; None falls through to the shared element mapping.
    pub(super) fn web_element(&mut self, el: &ElementRef) -> Option<Vec<Node>> {
        let name = el.value().name();
        if name == "script" {
            let ty = el.value().attr("type").unwrap_or("");
            if ty.starts_with("math/tex") {
                let tex: String = el.text().collect();
                return Some(self.web_math(&tex, ty.contains("mode=display"), None));
            }
            return Some(vec![]);
        }
        // Equation images first: Wikipedia marks them aria-hidden.
        if name == "img"
            && let Some((tex, display)) = latex_image(el)
        {
            return Some(self.web_math(&tex, display, None));
        }
        if is_hidden(el) {
            return Some(vec![]);
        }
        if matches!(
            name,
            "nav"
                | "footer"
                | "form"
                | "button"
                | "iframe"
                | "noscript"
                | "d-front-matter"
                | "d-bibliography"
                | "d-toc"
                | "distill-header"
                | "distill-footer"
                | "d-interstitial"
                | "d-byline"
                | "d-footnote-list"
                | "d-citation-list"
                | "d-hover-box"
        ) || is_chrome(el)
        {
            return Some(vec![]);
        }
        // Rendered math whose source sits next to it in a <script>.
        if el.value().classes().any(|c| {
            matches!(
                c,
                "MathJax_Preview"
                    | "MathJax"
                    | "MathJax_Display"
                    | "MathJax_SVG"
                    | "MathJax_SVG_Display"
                    | "MathJax_CHTML"
                    | "MJX_Assistive_MathML"
            )
        }) {
            return Some(vec![]);
        }
        if has(el, "katex") {
            let tex = el
                .select(&sel("annotation"))
                .find(|a| a.value().attr("encoding") == Some("application/x-tex"))
                .map(|a| a.text().collect::<String>())
                .unwrap_or_default();
            let display = el
                .parent()
                .and_then(ElementRef::wrap)
                .is_some_and(|p| has(&p, "katex-display"));
            return Some(self.web_math(&tex, display, None));
        }
        match name {
            "d-math" => {
                let tex: String = el.text().collect();
                Some(self.web_math(&tex, el.value().attr("block").is_some(), None))
            }
            "mjx-container" => {
                let math = el.select(&sel("math")).next()?;
                Some(self.math(&math))
            }
            "d-footnote" => {
                let body = self.children(el);
                Some(self.web_note(body))
            }
            "d-cite" => Some(self.web_cite(el.value().attr("key").unwrap_or(""))),
            "img" => {
                let url = image_url(el);
                Some(self.image(el, url.as_deref()))
            }
            "pre" | "code" | "textarea" | "kbd" | "samp" | "tt" => {
                self.web().skip += 1;
                let out = self.element_default(el);
                self.web().skip -= 1;
                Some(out)
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Page structure.

fn text_len(el: &ElementRef) -> usize {
    el.descendants()
        .filter(|n| {
            n.value().is_text()
                && !n.ancestors().any(|a| {
                    a.value().as_element().is_some_and(|e| {
                        matches!(e.name(), "script" | "style" | "noscript" | "template")
                    })
                })
        })
        .map(|n| n.value().as_text().map_or(0, |t| t.trim().chars().count()))
        .sum()
}

/// The element holding the article: where most paragraph text sits.
fn pick_root(doc: &Html) -> ElementRef<'_> {
    let body = doc
        .select(&sel("body"))
        .next()
        .unwrap_or_else(|| doc.root_element());
    if let Some(a) = doc.select(&sel("d-article")).next() {
        return a;
    }
    let mut scores: HashMap<NodeId, f64> = HashMap::new();
    for p in doc.select(&sel("p, pre")) {
        if p.ancestors().filter_map(ElementRef::wrap).any(|a| {
            matches!(
                a.value().name(),
                "nav" | "footer" | "aside" | "header" | "form"
            ) || is_chrome(&a)
        }) {
            continue;
        }
        let len = p.text().map(|t| t.trim().chars().count()).sum::<usize>() as f64;
        if len < 25.0 {
            continue;
        }
        let mut anc = p.ancestors().filter_map(ElementRef::wrap);
        if let Some(parent) = anc.next() {
            *scores.entry(parent.id()).or_default() += len;
            if let Some(gp) = anc.next() {
                *scores.entry(gp.id()).or_default() += len / 2.0;
            }
        }
    }
    let Some(best_id) = scores
        .iter()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(id, _)| *id)
    else {
        return body;
    };
    let mut best = ElementRef::wrap(doc.tree.get(best_id).unwrap()).unwrap();
    // Widen to wrappers that add little text (they usually hold the title).
    let mut best_len = text_len(&best);
    while let Some(parent) = best.parent().and_then(ElementRef::wrap) {
        if matches!(parent.value().name(), "body" | "html") {
            break;
        }
        let len = text_len(&parent);
        if len as f64 > best_len as f64 * 1.15 + 300.0 {
            break;
        }
        best = parent;
        best_len = len;
    }
    best
}

fn meta(doc: &Html, keys: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for m in doc.select(&sel("meta[content]")) {
        let k = m
            .value()
            .attr("name")
            .or_else(|| m.value().attr("property"))
            .unwrap_or("");
        if keys.contains(&k) {
            let v = collapse_ws(m.value().attr("content").unwrap_or(""));
            if !v.is_empty() && !out.contains(&v) {
                out.push(v);
            }
        }
    }
    out
}

fn authors(doc: &Html) -> Vec<String> {
    let cites = meta(doc, &["citation_author", "dc.creator", "DC.creator"]);
    if !cites.is_empty() {
        return cites;
    }
    // Distill keeps its byline in a JSON blob.
    if let Some(fm) = doc.select(&sel("d-front-matter script")).next() {
        let js: String = fm.text().collect();
        let mut names = Vec::new();
        for (pos, _) in js.match_indices("\"author\"") {
            if let Some(n) = js_strings(&js[pos + 8..]).into_iter().next()
                && !n.is_empty()
                && !names.contains(&n)
            {
                names.push(n);
            }
        }
        if !names.is_empty() {
            return names;
        }
    }
    let named: Vec<String> = meta(doc, &["author", "article:author"])
        .into_iter()
        .filter(|a| !a.starts_with("http"))
        .flat_map(|a| a.split([',', ';']).map(collapse_ws).collect::<Vec<_>>())
        .filter(|a| !a.is_empty())
        .collect();
    if !named.is_empty() {
        return named;
    }
    // Bylines marked up as class="author" (marks like * and † dropped).
    let mut names = Vec::new();
    for a in doc.select(&sel(".author, .byline .name, [rel=author]")) {
        let text: String = a
            .descendants()
            .filter(|n| {
                n.value().is_text()
                    && !n
                        .ancestors()
                        .any(|x| x.value().as_element().is_some_and(|e| e.name() == "sup"))
            })
            .filter_map(|n| n.value().as_text().map(|t| t.to_string()))
            .collect();
        let name = collapse_ws(
            text.trim()
                .trim_end_matches([',', ';', '&'])
                .trim_start_matches("by ")
                .trim(),
        );
        if !name.is_empty() && name.len() < 60 && !names.contains(&name) {
            names.push(name);
        }
    }
    if names.len() > 60 { Vec::new() } else { names }
}

/// `{...}` starting at the first `{` (BibTeX: no string quoting; `\{` is literal).
fn brace_group(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0;
    let mut escaped = false;
    for (i, c) in s[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=start + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// LaTeX accents and special letters in BibTeX text -> Unicode.
fn latex_text(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            if !matches!(c, '{' | '}') {
                out.push(if c == '~' { ' ' } else { c });
            }
            continue;
        }
        let Some(&n) = chars.peek() else { break };
        let accent = match n {
            '"' => Some('\u{308}'),
            '\'' => Some('\u{301}'),
            '`' => Some('\u{300}'),
            '^' => Some('\u{302}'),
            '~' => Some('\u{303}'),
            '=' => Some('\u{304}'),
            '.' => Some('\u{307}'),
            _ => None,
        };
        if let Some(mark) = accent {
            chars.next();
            // \"o or \"{o}
            let mut base = chars.next();
            if base == Some('{') {
                base = chars.next();
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
            }
            if let Some(b) = base {
                out.push(b);
                out.push(mark);
            }
            continue;
        }
        let word: String =
            std::iter::from_fn(|| chars.next_if(|c| c.is_ascii_alphabetic())).collect();
        match word.as_str() {
            "L" => out.push('Ł'),
            "l" => out.push('ł'),
            "o" => out.push('ø'),
            "O" => out.push('Ø'),
            "ss" => out.push('ß'),
            "aa" => out.push('å'),
            "ae" => out.push('æ'),
            "i" => out.push('ı'),
            "" => {
                if let Some(c) = chars.next() {
                    out.push(c); // \& \% \_ \$ ...
                }
            }
            _ => {} // other commands (\emph, \textit, ...): keep only their argument
        }
    }
    out
}

/// Minimal BibTeX reader, for Distill's citation list.
fn parse_bibtex(src: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out = HashMap::new();
    let mut rest = src;
    while let Some(at) = rest.find('@') {
        rest = &rest[at + 1..];
        let Some(body) = brace_group(rest) else { break };
        let open = rest.find('{').unwrap();
        let inner = &body[1..body.len() - 1];
        rest = &rest[open + body.len()..];
        let Some((key, fields_src)) = inner.split_once(',') else {
            continue;
        };
        let mut fields = HashMap::new();
        let mut s = fields_src;
        while let Some(eq) = s.find('=') {
            let name = s[..eq]
                .trim()
                .trim_start_matches(',')
                .trim()
                .to_ascii_lowercase();
            let v = s[eq + 1..].trim_start();
            let (value, used) = if v.starts_with('{') {
                match brace_group(v) {
                    Some(g) => (g[1..g.len() - 1].to_string(), g.len()),
                    None => break,
                }
            } else if let Some(q) = v.strip_prefix('"') {
                match q.find('"') {
                    Some(e) => (q[..e].to_string(), e + 2),
                    None => break,
                }
            } else {
                let e = v.find(',').unwrap_or(v.len());
                (v[..e].trim().to_string(), e)
            };
            let clean = collapse_ws(&latex_text(&value).replace("--", "–"));
            fields.insert(name, clean);
            s = &v[used..];
        }
        out.insert(key.trim().to_string(), fields);
    }
    out
}

fn format_bib(f: &HashMap<String, String>) -> String {
    let authors: Vec<String> = f
        .get("author")
        .map(|a| {
            a.split(" and ")
                .map(|n| match n.split_once(',') {
                    Some((last, first)) => format!("{} {}", first.trim(), last.trim()),
                    None => n.trim().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut parts = Vec::new();
    if !authors.is_empty() {
        parts.push(authors.join(", "));
    }
    if let Some(t) = f.get("title") {
        parts.push(t.clone());
    }
    let venue: Vec<&str> = ["journal", "booktitle", "publisher"]
        .iter()
        .filter_map(|k| f.get(*k).map(String::as_str))
        .take(1)
        .chain(f.get("year").map(String::as_str))
        .collect();
    if !venue.is_empty() {
        parts.push(venue.join(", "));
    }
    parts.join(". ") + "."
}

fn heading_level(n: &Node) -> Option<u8> {
    match n.as_el()?.tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        _ => None,
    }
}

fn contains_level(nodes: &[Node], level: u8) -> bool {
    nodes.iter().any(|n| {
        heading_level(n) == Some(level)
            || n.as_el().is_some_and(|e| {
                matches!(e.tag, "div" | "section" | "header") && contains_level(&e.children, level)
            })
    })
}

/// Split at headings of `level`, looking through wrapper divs/sections.
fn split_at(nodes: Vec<Node>, level: u8, chapters: &mut Vec<Vec<Node>>) {
    for n in nodes {
        if heading_level(&n) == Some(level) {
            chapters.push(vec![n]);
            continue;
        }
        match n {
            Node::El(mut e)
                if matches!(e.tag, "div" | "section" | "header")
                    && contains_level(&e.children, level) =>
            {
                if let Some(id) = e.remove("id") {
                    chapters
                        .last_mut()
                        .unwrap()
                        .push(El::new("span").attr("id", id).into());
                }
                split_at(std::mem::take(&mut e.children), level, chapters);
            }
            n => chapters.last_mut().unwrap().push(n),
        }
    }
}

impl Conv {
    /// Readable text of output nodes, with equations as their TeX.
    fn node_text(&self, nodes: &[Node], out: &mut String) {
        for n in nodes {
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Slot(i) => {
                    if let Slot::Math { job, .. } = &self.slots[*i] {
                        out.push_str(&self.math_jobs[*job].tex);
                    }
                }
                Node::El(e) if e.get("epub:type") == Some("noteref") => {}
                Node::El(e) => self.node_text(&e.children, out),
            }
        }
    }

    fn label(&self, el: &El) -> String {
        let mut s = String::new();
        self.node_text(&el.children, &mut s);
        collapse_ws(&s)
    }

    /// Give a heading an id (if it has none) and return it.
    fn heading_id(&mut self, el: &mut El) -> String {
        if let Some(id) = el.get("id") {
            return id.to_string();
        }
        let id = self.fresh_id("sec");
        el.set("id", id.clone());
        id
    }
}

pub(super) fn convert(doc: &Html, base: Url, fetch: Fetch) -> Result<Book> {
    let root = pick_root(doc);
    let setup = math_setup(doc);
    // Distill's bibliography: inline, or a .bib file next to the page.
    let mut bib_src: String = doc
        .select(&sel(
            "d-bibliography script, script[type=\"text/bibliography\"]",
        ))
        .map(|s| s.text().collect::<String>())
        .collect();
    for b in doc.select(&sel("d-bibliography[src]")) {
        if let Some(data) = base
            .join(b.value().attr("src").unwrap_or(""))
            .ok()
            .and_then(|u| fetch(u.as_str()))
        {
            bib_src.push_str(&String::from_utf8_lossy(&data));
        }
    }
    let bib = parse_bibtex(&bib_src);

    let mut cv = Conv::new(base, 700.0);
    cv.web = Some(WebCtx {
        delims: setup.delims,
        escapes: setup.escapes,
        skip: 0,
        definitions: Vec::new(),
        bib,
        cite_order: Vec::new(),
    });

    let root_h1 = root
        .select(&sel("h1"))
        .next()
        .map(|h| plain_text(&h))
        .filter(|t| !t.is_empty());
    let title = root_h1
        .clone()
        .or_else(|| {
            meta(doc, &["citation_title", "og:title", "twitter:title"])
                .into_iter()
                .next()
        })
        .or_else(|| {
            doc.select(&sel("title"))
                .next()
                .map(|t| collapse_ws(&t.text().collect::<String>()))
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Untitled".into());
    let authors = authors(doc);
    let lang = doc
        .root_element()
        .value()
        .attr("lang")
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or("en")
        .to_string();

    let mut nodes = cv.children(&root);
    // Distill keeps acknowledgements, contributions etc. in a separate appendix.
    if root.value().name() == "d-article" {
        for app in doc.select(&sel("d-appendix")) {
            nodes.extend(cv.children(&app));
        }
    }
    if root_h1.is_none() {
        let mut head = vec![El::new("h1").text(title.clone()).into()];
        if !authors.is_empty() {
            head.push(
                El::new("p")
                    .class("ltx_authors")
                    .text(authors.join(", "))
                    .into(),
            );
        }
        nodes.splice(0..0, head);
    }

    // Chapters: split at the highest section level the page uses.
    let mut counts = [0usize; 5];
    walk(&nodes, &mut |e: &El| {
        if let Some(l) = heading_level(&Node::El(El::new(e.tag))) {
            counts[l as usize] += 1;
        }
    });
    let level = if counts[1] >= 2 {
        1
    } else {
        (2..=4).find(|l| counts[*l as usize] > 0).unwrap_or(0)
    };
    let mut parts: Vec<Vec<Node>> = vec![Vec::new()];
    if level > 0 {
        split_at(nodes, level, &mut parts);
    } else {
        parts[0] = nodes;
    }

    let mut chapters = Vec::new();
    for (i, mut body) in parts.into_iter().enumerate() {
        let mut toc = Vec::new();
        let mut ch_title = title.clone();
        if i == 0 {
            toc.push(TocEntry {
                level: 1,
                label: title.clone(),
                id: None,
            });
        } else if let Some(Node::El(h)) = body.first_mut() {
            let id = cv.heading_id(h);
            ch_title = cv.label(h);
            toc.push(TocEntry {
                level: 1,
                label: ch_title.clone(),
                id: Some(id),
            });
        }
        // Next level down goes in the table of contents too.
        let mut subs: Vec<(String, String)> = Vec::new();
        let sub = level + 1;
        let skip_first = usize::from(i > 0).min(body.len());
        walk_mut(&mut body[skip_first..], &mut |e: &mut El| {
            if heading_level(&Node::El(El::new(e.tag))) == Some(sub) {
                let id = cv.heading_id(e);
                subs.push((cv.label(e), id));
            }
        });
        for (label, id) in subs {
            if !label.is_empty() {
                toc.push(TocEntry {
                    level: 2,
                    label,
                    id: Some(id),
                });
            }
        }
        let file = if i == 0 {
            "title.xhtml".to_string()
        } else {
            format!("ch{i:02}.xhtml")
        };
        chapters.push(Chapter {
            file,
            title: ch_title,
            body,
            notes: Vec::new(),
            toc,
        });
    }

    // Distill references, numbered in order of first citation.
    let web = cv.web.take().unwrap();
    if !web.cite_order.is_empty() {
        let mut list = El::new("ol").class("ltx_biblist");
        for key in &web.cite_order {
            let text = web
                .bib
                .get(key)
                .map(format_bib)
                .unwrap_or_else(|| key.clone());
            list.children.push(
                El::new("li")
                    .class("ltx_bibitem")
                    .attr("id", sanitize_id(&format!("ref-{key}")))
                    .text(text)
                    .into(),
            );
        }
        let file = format!("ch{:02}.xhtml", chapters.len());
        chapters.push(Chapter {
            file,
            title: "References".into(),
            body: vec![
                El::new("h2")
                    .attr("id", "references")
                    .text("References")
                    .into(),
                list.into(),
            ],
            notes: Vec::new(),
            toc: vec![TocEntry {
                level: 1,
                label: "References".into(),
                id: Some("references".into()),
            }],
        });
    }

    // Each footnote goes to the end of the chapter that refers to it.
    let mut notes: Vec<Option<Node>> = std::mem::take(&mut cv.notes)
        .into_iter()
        .map(Some)
        .collect();
    for ch in &mut chapters {
        let mut refs = Vec::new();
        walk(&ch.body, &mut |e: &El| {
            if e.get("epub:type") == Some("noteref")
                && let Some(h) = e.get("href")
            {
                refs.push(h.trim_start_matches('#').to_string());
            }
        });
        for r in refs {
            if let Some(slot) = notes.iter_mut().find(|n| {
                n.as_ref().and_then(|n| n.as_el()).and_then(|e| e.get("id")) == Some(r.as_str())
            }) {
                ch.notes.push(slot.take().unwrap());
            }
        }
    }
    if let Some(last) = chapters.last_mut() {
        last.notes.extend(notes.into_iter().flatten());
    }

    let mut preamble = setup.macros;
    for d in &web.definitions {
        preamble.push_str(d);
    }
    let _ = setup.katex;

    let mut book = Book {
        title,
        math_preamble: preamble,
        has_title: true,
        authors,
        lang,
        chapters,
        slots: cv.slots,
        math_jobs: cv.math_jobs,
        eq_tables: cv.eq_tables,
        images: cv.images,
        warnings: cv.warnings,
    };
    for ch in &mut book.chapters {
        let body = std::mem::take(&mut ch.body);
        ch.body = body.into_iter().flat_map(normalize).collect();
        let notes = std::mem::take(&mut ch.notes);
        ch.notes = notes.into_iter().flat_map(normalize).collect();
    }
    resolve_links(&mut book);
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(l: &str, r: &str, display: bool) -> Delim {
        Delim {
            left: l.into(),
            right: r.into(),
            display,
            env: false,
        }
    }

    #[test]
    fn splits_text_like_katex() {
        let delims = vec![d("$$", "$$", true), d("$", "$", false)];
        let p = split_math(
            r"cost \$5, so $a_{1}$ and $$\frac{1}{2}$$ end",
            &delims,
            true,
        );
        assert_eq!(
            p,
            vec![
                Piece::Text("cost $5, so ".into()),
                Piece::Math {
                    tex: "a_{1}".into(),
                    display: false
                },
                Piece::Text(" and ".into()),
                Piece::Math {
                    tex: r"\frac{1}{2}".into(),
                    display: true
                },
                Piece::Text(" end".into()),
            ]
        );
        // A `$` inside braces doesn't close the math; an unmatched one is text.
        let p = split_math(r"$\text{a$b}$ and $ alone", &delims, true);
        assert_eq!(
            p[0],
            Piece::Math {
                tex: r"\text{a$b}".into(),
                display: false
            }
        );
        assert_eq!(p[1], Piece::Text(" and $ alone".into()));
    }

    #[test]
    fn reads_page_settings() {
        let js = r#"renderMathInElement(document.body, { delimiters: [ {left: "$$", right: "$$", display: true}, {left: "\\(", right: "\\)", display: false} ], macros: {"\\RR": "\\mathbb{R}"} });"#;
        assert_eq!(
            katex_delims(js),
            vec![d("$$", "$$", true), d("\\(", "\\)", false)]
        );
        assert_eq!(config_macros(js), r"\def\RR{\mathbb{R}}");
        let js = r#"MathJax = { tex: { inlineMath: [['$', '$'], ['\\(', '\\)']], macros: { bold: ["{\\bf #1}", 1] } } };"#;
        assert_eq!(
            mathjax_pairs(js, "inlineMath", false),
            vec![d("$", "$", false), d("\\(", "\\)", false)]
        );
        assert_eq!(config_macros(js), r"\newcommand{\bold}[1]{{\bf #1}}");
    }

    #[test]
    fn definitions_and_tags() {
        assert_eq!(
            strip_definitions(r"\newcommand{\vx}{\mathbf{x}} \def\a#1{#1}"),
            ""
        );
        assert_eq!(
            strip_definitions(r"\newcommand{\vx}{\mathbf{x}} x^2"),
            "x^2"
        );
        assert_eq!(
            take_tag(r"a=b \tag{3}"),
            ("a=b ".to_string(), Some("(3)".to_string()))
        );
    }

    #[test]
    fn bibtex() {
        let b = parse_bibtex(
            "@article{k1, title={Attention {Is} All}, author={Vaswani, Ashish and Noam Shazeer}, year={2017}, journal={NeurIPS}}",
        );
        assert_eq!(
            format_bib(&b["k1"]),
            "Ashish Vaswani, Noam Shazeer. Attention Is All. NeurIPS, 2017."
        );
        let b = parse_bibtex(
            r#"@misc{a, author={G{\"o}del, Kurt and \L{}ukasz Kaiser}, title={On \emph{x} \& y}} @misc{b, title="Second"}"#,
        );
        assert_eq!(b["a"]["author"], "Go\u{308}del, Kurt and Łukasz Kaiser");
        assert_eq!(b["a"]["title"], "On x & y");
        assert_eq!(b["b"]["title"], "Second");
    }
}
