//! LaTeXML HTML (arXiv's HTML edition) -> book chapters in our output tree.
//!
//! The paper is `article.ltx_document`. Its title block and abstract become a
//! title page and every top-level `<section>` becomes one chapter. Equations,
//! figures and inline SVG become `Slot`s, filled once everything is rendered
//! (see `layout.rs`).

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use ego_tree::NodeRef;
use scraper::{ElementRef, Html, Node as HNode, Selector};
use url::Url;

use crate::math::MathJob;

mod web;
use crate::tree::{El, Node, normalize, sanitize_id, walk, walk_mut};

pub struct Book {
    pub title: String,
    /// TeX put in front of every equation (a web page's macro definitions).
    pub math_preamble: String,
    /// Whether the title came from the paper's own title element.
    pub has_title: bool,
    pub authors: Vec<String>,
    pub lang: String,
    pub chapters: Vec<Chapter>,
    pub slots: Vec<Slot>,
    pub math_jobs: Vec<MathJob>,
    pub eq_tables: Vec<EqTable>,
    pub images: Vec<String>,
    pub warnings: Vec<String>,
}

pub struct Chapter {
    pub file: String,
    pub title: String,
    pub body: Vec<Node>,
    pub notes: Vec<Node>,
    pub toc: Vec<TocEntry>,
}

pub struct TocEntry {
    pub level: u8,
    pub label: String,
    /// Fragment id within the chapter; None means the chapter start.
    pub id: Option<String>,
}

pub enum Slot {
    /// A single equation: inline, or display when outside an equation table.
    Math { job: usize, display: bool },
    /// Rows `rows` of equation table `table`, typeset as one image.
    EqRows {
        table: usize,
        rows: std::ops::Range<usize>,
    },
    /// A figure image (index into `Book::images`).
    Image {
        src: usize,
        width: Option<f64>,
        in_figure: bool,
        alt: String,
    },
    /// An inline `<svg>` picture. Its `foreignObject` labels are cut out and
    /// replaced by `\u{1}N\u{1}` markers, filled with typeset `labels[N]`.
    Svg {
        svg: String,
        alt: String,
        labels: Vec<SvgLabel>,
        style: Option<String>,
    },
}

/// A text/math label from a picture's `<foreignObject>`, in the
/// foreignObject's own coordinates.
pub struct SvgLabel {
    pub job: usize,
    /// Box width (the label is centred in it).
    pub width: f64,
    /// Box height, and the baseline's offset from the box top.
    pub height: f64,
    pub baseline: Option<f64>,
    /// Font size in user units.
    pub font: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A LaTeXML display-equation table whose cells are all single equations.
pub struct EqTable {
    pub aligns: Vec<Align>,
    /// Gap (in em) before each column.
    pub gaps: Vec<f64>,
    pub rows: Vec<Vec<Option<usize>>>,
    /// Fallback text per row, used if a cell fails to typeset.
    pub row_tex: Vec<String>,
}

const BULLETS: &[&str] = &[
    "•", "∙", "◦", "–", "—", "-", "∗", "*", "⋅", "·", "▪", "■", "○", "‣", "□", "▶", "►",
];

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

fn has(el: &ElementRef, class: &str) -> bool {
    el.value().classes().any(|c| c == class)
}

fn child_elements<'a>(el: &ElementRef<'a>) -> impl Iterator<Item = ElementRef<'a>> {
    el.children().filter_map(ElementRef::wrap)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// TeX source of a `<math>` element, as LaTeXML records it.
fn math_tex(el: &ElementRef) -> String {
    if let Some(a) = el.value().attr("alttext")
        && !a.trim().is_empty()
    {
        return a.trim().to_string();
    }
    for a in el.select(&sel("annotation")) {
        let enc = a.value().attr("encoding").unwrap_or("");
        if enc.eq_ignore_ascii_case("application/x-tex") || enc.eq_ignore_ascii_case("latex") {
            let t: String = a.text().collect();
            if !t.trim().is_empty() {
                return t.trim().to_string();
            }
        }
    }
    String::new()
}

/// Plain text for labels (TOC, <title>): math becomes its TeX, notes vanish.
fn plain_text(el: &ElementRef) -> String {
    fn go(n: NodeRef<HNode>, out: &mut String) {
        match n.value() {
            HNode::Text(t) => out.push_str(t),
            HNode::Element(e) => {
                let er = ElementRef::wrap(n).unwrap();
                if e.name() == "math" {
                    out.push_str(&math_tex(&er));
                    return;
                }
                if [
                    "ltx_note",
                    "ltx_ERROR",
                    "ltx_pubnotes",
                    "ltx_author_notes",
                    "ltx_orcid",
                ]
                .iter()
                .any(|c| has(&er, c))
                    || matches!(e.name(), "button" | "svg")
                {
                    return;
                }
                if e.name() == "br" {
                    out.push(' ');
                }
                for c in n.children() {
                    go(c, out);
                }
            }
            _ => {}
        }
    }
    let mut s = String::new();
    go(**el, &mut s);
    collapse_ws(&s)
}

/// Serialise a source subtree as XML (for MathML and inline SVG).
fn xml_subtree(n: NodeRef<HNode>, out: &mut String, root_ns: Option<&str>) {
    match n.value() {
        HNode::Text(t) => crate::tree::escape_text(t, out),
        HNode::Element(e) => {
            let name = e.name();
            if matches!(name, "annotation" | "annotation-xml") {
                return;
            }
            out.push('<');
            out.push_str(name);
            let mut seen = HashSet::new();
            if let Some(ns) = root_ns {
                out.push_str(&format!(" xmlns=\"{ns}\""));
                seen.insert("xmlns".to_string());
            }
            for (k, v) in e.attrs() {
                if !seen.insert(k.to_string()) || k.contains(':') {
                    continue;
                }
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                crate::tree::escape_attr(v, out);
                out.push('"');
            }
            out.push('>');
            for c in n.children() {
                xml_subtree(c, out, None);
            }
            out.push_str("</");
            out.push_str(name);
            out.push('>');
        }
        _ => {}
    }
}

/// Keep only presentational style properties that survive reflow.
fn filter_style(style: &str) -> Option<String> {
    let mut kept = Vec::new();
    for decl in style.split(';') {
        let Some((prop, val)) = decl.split_once(':') else {
            continue;
        };
        let (prop, val) = (prop.trim().to_ascii_lowercase(), val.trim());
        let ok = match prop.as_str() {
            // Font sizes are left to the stylesheet: LaTeX's \small in captions
            // and tables would otherwise shrink text a second time.
            "color" | "background-color" | "font-weight" | "font-style" | "font-variant"
            | "text-decoration" => true,
            _ => false,
        };
        if ok && !val.is_empty() && !val.contains(['<', '>', '"', '\\']) {
            kept.push(format!("{prop}:{val}"));
        }
    }
    (!kept.is_empty()).then(|| kept.join(";"))
}

struct Conv {
    base: Url,
    slots: Vec<Slot>,
    math_jobs: Vec<MathJob>,
    math_index: HashMap<(String, bool), usize>,
    eq_tables: Vec<EqTable>,
    images: Vec<String>,
    image_index: HashMap<String, usize>,
    ids: HashSet<String>,
    notes: Vec<Node>,
    note_seq: usize,
    toc: Vec<TocEntry>,
    figure_depth: u32,
    figure_label: Option<String>,
    graphics_ref_width: f64,
    warnings: Vec<String>,
    /// Set when converting a general web page rather than a LaTeXML paper.
    web: Option<web::WebCtx>,
}

impl Conv {
    fn new(base: Url, graphics_ref_width: f64) -> Self {
        Conv {
            base,
            slots: Vec::new(),
            math_jobs: Vec::new(),
            math_index: HashMap::new(),
            eq_tables: Vec::new(),
            images: Vec::new(),
            image_index: HashMap::new(),
            ids: HashSet::new(),
            notes: Vec::new(),
            note_seq: 0,
            toc: Vec::new(),
            figure_depth: 0,
            figure_label: None,
            graphics_ref_width,
            warnings: Vec::new(),
            web: None,
        }
    }

    fn slot(&mut self, s: Slot) -> Node {
        self.slots.push(s);
        Node::Slot(self.slots.len() - 1)
    }

    fn math_job(&mut self, tex: String, display: bool, mathml: Option<String>) -> usize {
        let key = (tex.clone(), display);
        if let Some(&i) = self.math_index.get(&key) {
            return i;
        }
        self.math_jobs.push(MathJob {
            tex,
            mathml,
            display,
        });
        let i = self.math_jobs.len() - 1;
        self.math_index.insert(key, i);
        i
    }

    fn image_src(&mut self, raw: &str) -> Option<usize> {
        let url = if raw.starts_with("data:") {
            raw.to_string()
        } else {
            self.base.join(raw).ok()?.to_string()
        };
        if let Some(&i) = self.image_index.get(&url) {
            return Some(i);
        }
        self.images.push(url.clone());
        self.image_index.insert(url, self.images.len() - 1);
        Some(self.images.len() - 1)
    }

    /// A sanitized id, or None if it was already used (first one wins).
    fn take_id(&mut self, raw: Option<&str>) -> Option<String> {
        let id = sanitize_id(raw?.trim());
        if raw?.trim().is_empty() || !self.ids.insert(id.clone()) {
            return None;
        }
        Some(id)
    }

    fn fresh_id(&mut self, stem: &str) -> String {
        let mut n = self.ids.len();
        loop {
            let id = format!("{stem}{n}");
            if self.ids.insert(id.clone()) {
                return id;
            }
            n += 1;
        }
    }

    /// Copy id / class / filtered style from a source element.
    fn base_el(&mut self, tag: &'static str, src: &ElementRef) -> El {
        let mut el = El::new(tag);
        if let Some(id) = self.take_id(src.value().attr("id")) {
            el.set("id", id);
        }
        let mut classes: Vec<&str> = src
            .value()
            .classes()
            .filter(|c| c.starts_with("ltx_"))
            .collect();
        if self.web.is_some() {
            if src
                .value()
                .classes()
                .any(|c| c.to_ascii_lowercase().contains("caption"))
            {
                classes.push("ltx_caption");
            }
            if tag == "table" {
                classes.push("web_table");
            }
        }
        if !classes.is_empty() {
            el.set("class", classes.join(" "));
        }
        if let Some(style) = src.value().attr("style").and_then(filter_style) {
            el.set("style", style);
        }
        el
    }

    // -- traversal -----------------------------------------------------------

    fn children(&mut self, el: &ElementRef) -> Vec<Node> {
        self.nodes(el.children())
    }

    fn nodes<'a>(&mut self, iter: impl Iterator<Item = NodeRef<'a, HNode>>) -> Vec<Node> {
        let mut out = Vec::new();
        let mut runin: Option<Node> = None;
        for c in iter {
            if let Some(e) = ElementRef::wrap(c)
                && is_heading(e.value().name())
                && has(&e, "ltx_runin")
            {
                if let Some(r) = runin.take() {
                    out.push(El::new("p").child(r).into());
                }
                let mut span = self.base_el("span", &e);
                span.children = self.children(&e);
                runin = Some(span.into());
                continue;
            }
            let mut nodes = self.node(c);
            if runin.is_some() && nodes.iter().any(|n| !n.is_blank_text()) {
                let r = runin.take().unwrap();
                if let Err(r) = prepend_to_first_p(&mut nodes, r) {
                    out.push(El::new("p").child(r).into());
                }
            }
            out.extend(nodes);
        }
        if let Some(r) = runin {
            out.push(El::new("p").child(r).into());
        }
        out
    }

    fn node(&mut self, n: NodeRef<HNode>) -> Vec<Node> {
        match n.value() {
            HNode::Text(t) if self.web.is_some() => self.web_text(t),
            HNode::Text(t) => vec![Node::text(t.to_string())],
            HNode::Element(_) => self.element(&ElementRef::wrap(n).unwrap()),
            _ => vec![],
        }
    }

    fn element(&mut self, el: &ElementRef) -> Vec<Node> {
        if self.web.is_some()
            && let Some(nodes) = self.web_element(el)
        {
            return nodes;
        }
        self.element_default(el)
    }

    fn element_default(&mut self, el: &ElementRef) -> Vec<Node> {
        let name = el.value().name();
        const DROP_CLASSES: &[&str] = &[
            "ltx_ERROR",
            "ltx_TOC",
            "ltx_pagination",
            "ltx_listing_data",
            "ltx_page_navbar",
            "ltx_page_footer",
            "ltx_page_logo",
            "ltx_rdf",
            "package-alerts",
            "ltx_rule",
            "ltx_orcid",
            "ltx_orcidlogo",
        ];
        if DROP_CLASSES.iter().any(|c| has(el, c)) || el.value().attr("id") == Some("alerts") {
            return vec![];
        }
        if has(el, "ltx_note") && !has(el, "ltx_note_outer") {
            return self.note(el);
        }
        if has(el, "ltx_pubnotes") {
            return self.pubnotes(el);
        }

        let tag: &'static str = match name {
            "math" => return self.math(el),
            "table" if has(el, "ltx_eqn_table") => return self.equation_table(el),
            "img" => return self.image(el, el.value().attr("src")),
            "object" | "embed" => {
                let src = el.value().attr("data").or_else(|| el.value().attr("src"));
                return self.image(el, src);
            }
            "svg" => return self.inline_svg(el),
            "a" => return self.link(el),
            "ul" | "ol" => return self.list(el),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => return self.heading(el),
            "figure" => return self.figure(el),
            "br" => return vec![El::new("br").into()],
            "hr" => return vec![El::new("hr").into()],
            "script" | "style" | "button" | "nav" | "noscript" | "form" | "input" | "select"
            | "textarea" | "iframe" | "template" | "head" | "link" | "meta" | "dialog"
            | "video" | "audio" | "canvas" | "map" | "area" => return vec![],
            "span" | "div" if has(el, "ltx_tabular") => "table",
            "span" | "div" if has(el, "ltx_thead") => "thead",
            "span" | "div" if has(el, "ltx_tbody") => "tbody",
            "span" | "div" if has(el, "ltx_tfoot") => "tfoot",
            "span" | "div" if has(el, "ltx_tr") => "tr",
            "span" | "div" if has(el, "ltx_td") => {
                if has(el, "ltx_th") {
                    "th"
                } else {
                    "td"
                }
            }
            "p" => "p",
            "div" | "header" | "footer" | "main" | "center" | "aside" | "article" => "div",
            "section" => "section",
            "blockquote" => "blockquote",
            "pre" => "pre",
            "code" | "tt" => "code",
            "em" => "em",
            "i" => "i",
            "b" => "b",
            "strong" => "strong",
            "sup" => "sup",
            "sub" => "sub",
            "small" => "small",
            "u" => "u",
            "s" | "strike" => "s",
            "del" => "del",
            "ins" => "ins",
            "mark" => "mark",
            "q" => "q",
            "abbr" => "abbr",
            "cite" => "cite",
            "dfn" => "dfn",
            "kbd" => "kbd",
            "samp" => "samp",
            "var" => "var",
            "span" | "font" | "big" | "label" => "span",
            "dl" => "dl",
            "dt" => "dt",
            "dd" => "dd",
            "li" => "li",
            "table" => "table",
            "caption" => "caption",
            "thead" => "thead",
            "tbody" => "tbody",
            "tfoot" => "tfoot",
            "tr" => "tr",
            "td" => "td",
            "th" => "th",
            "figcaption" => "figcaption",
            // Unknown element: keep its content.
            _ => return self.children(el),
        };

        let mut out = self.base_el(tag, el);
        if matches!(tag, "td" | "th") {
            for a in ["colspan", "rowspan"] {
                if let Some(v) = el
                    .value()
                    .attr(a)
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    && v > 1
                {
                    out.set(a, v.to_string());
                }
            }
        }
        out.children = self.children(el);
        // A paragraph left empty (e.g. it only held a LaTeXML error) adds nothing.
        if has(el, "ltx_para") && out.children.iter().all(Node::is_blank_text) {
            return vec![];
        }
        if has(el, "ltx_listingline") {
            // Lines are shown with white-space: pre-wrap, so the source's
            // line-ending whitespace would add a blank line after each line.
            out.children
                .retain(|n| !matches!(n, Node::Text(t) if t.trim().is_empty() && t.contains('\n')));
            if out.children.iter().all(Node::is_blank_text) {
                out.children = vec![El::new("br").into()];
            }
        }
        vec![out.into()]
    }

    // -- specific elements -------------------------------------------------

    fn math(&mut self, el: &ElementRef) -> Vec<Node> {
        let tex = math_tex(el);
        let display = el.value().attr("display") == Some("block");
        let mut mml = String::new();
        xml_subtree(**el, &mut mml, Some("http://www.w3.org/1998/Math/MathML"));
        if tex.is_empty() {
            let text = plain_text(el);
            if text.is_empty() {
                return vec![];
            }
            // Render the MathML; give MathJax the text as "TeX" so it fails over.
            let job = self.math_job(
                format!("\\text{{{}}}", text.replace(['{', '}', '\\'], "")),
                display,
                Some(mml),
            );
            return vec![self.slot(Slot::Math { job, display })];
        }
        let job = self.math_job(tex, display, Some(mml));
        let slot = self.slot(Slot::Math { job, display });
        if display {
            let mut div = El::new("div").class("ltx_eq");
            if let Some(id) = self.take_id(el.value().attr("id")) {
                div.set("id", id);
            }
            vec![div.child(slot).into()]
        } else {
            vec![slot]
        }
    }

    fn equation_table(&mut self, table: &ElementRef) -> Vec<Node> {
        struct Row<'a> {
            id: Option<String>,
            cells: Vec<(Align, Option<ElementRef<'a>>)>,
            eqno: Option<String>,
            span: usize,
            pure: bool,
            src: ElementRef<'a>,
        }

        // Collect rows, taking each tbody's id for its first row
        // (LaTeXML puts the equation label on the tbody in groups).
        let mut rows: Vec<Row> = Vec::new();
        let mut tr_list: Vec<(ElementRef, Option<String>)> = Vec::new();
        for c in child_elements(table) {
            match c.value().name() {
                "tr" => tr_list.push((c, None)),
                "thead" | "tbody" | "tfoot" => {
                    let mut gid = c.value().attr("id").map(str::to_string);
                    for tr in child_elements(&c).filter(|t| t.value().name() == "tr") {
                        tr_list.push((tr, gid.take()));
                    }
                }
                _ => {}
            }
        }
        for (tr, group_id) in tr_list {
            let mut row = Row {
                id: None,
                cells: Vec::new(),
                eqno: None,
                span: 1,
                pure: true,
                src: tr,
            };
            let mut ids = Vec::new();
            ids.extend(group_id);
            ids.extend(tr.value().attr("id").map(str::to_string));
            row.id = ids.into_iter().next();
            for td in child_elements(&tr).filter(|t| matches!(t.value().name(), "td" | "th")) {
                if has(&td, "ltx_eqn_center_padleft") || has(&td, "ltx_eqn_center_padright") {
                    continue;
                }
                if has(&td, "ltx_eqn_eqno") {
                    let t = plain_text(&td);
                    if !t.is_empty() {
                        row.eqno = Some(t);
                    }
                    row.span = td
                        .value()
                        .attr("rowspan")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1)
                        .max(1);
                    continue;
                }
                let align = if has(&td, "ltx_align_right") {
                    Align::Right
                } else if has(&td, "ltx_align_left") {
                    Align::Left
                } else {
                    Align::Center
                };
                let mut math = None;
                for n in td.children() {
                    match n.value() {
                        HNode::Text(t) if t.trim().is_empty() => {}
                        HNode::Element(e) if e.name() == "math" && math.is_none() => {
                            math = ElementRef::wrap(n);
                        }
                        HNode::Comment(_) => {}
                        _ => row.pure = false,
                    }
                }
                if td.value().attr("colspan").is_some_and(|c| c != "1") {
                    row.pure = false;
                }
                row.cells.push((align, math));
            }
            rows.push(row);
        }

        let mut group = self.base_el("div", table);
        group.set("class", "ltx_eqn_group");

        if rows.is_empty() {
            return vec![];
        }

        if !rows.iter().all(|r| r.pure) {
            // Something other than plain equations (text, nested tables...):
            // lay each row out as HTML instead.
            for r in &rows {
                let mut line = El::new("div").class("ltx_eq ltx_eq_html");
                if let Some(id) = self.take_id(r.id.as_deref()) {
                    line.set("id", id);
                }
                if let Some(no) = &r.eqno {
                    line.children
                        .push(El::new("span").class("ltx_eqno").text(no.clone()).into());
                }
                for td in child_elements(&r.src).filter(|t| matches!(t.value().name(), "td" | "th"))
                {
                    if has(&td, "ltx_eqn_eqno") {
                        continue;
                    }
                    line.children.extend(self.children(&td));
                    line.children.push(Node::text(" "));
                }
                group.children.push(line.into());
            }
            return vec![group.into()];
        }

        let ncols = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        let mut aligns = vec![Align::Center; ncols];
        for (j, a) in aligns.iter_mut().enumerate() {
            let mut cells = rows.iter().filter_map(|r| r.cells.get(j));
            let first_any = cells.clone().next();
            if let Some((al, _)) = cells.find(|(_, m)| m.is_some()).or(first_any) {
                *a = *al;
            }
        }
        let is_eqnarray = has(table, "ltx_eqn_eqnarray");
        let mut gaps = vec![0.0; ncols];
        for j in 1..ncols {
            gaps[j] = if is_eqnarray {
                0.3
            } else if aligns[j - 1] == Align::Left && aligns[j] == Align::Right {
                2.0
            } else {
                0.0
            };
        }

        let mut eq = EqTable {
            aligns: aligns.clone(),
            gaps,
            rows: Vec::new(),
            row_tex: Vec::new(),
        };
        for r in &rows {
            let mut cells = Vec::with_capacity(ncols);
            let mut texs = Vec::new();
            for j in 0..ncols {
                let job = match r.cells.get(j) {
                    Some((_, Some(m))) => {
                        let mut tex = math_tex(m);
                        let mut mml = String::new();
                        xml_subtree(**m, &mut mml, Some("http://www.w3.org/1998/Math/MathML"));
                        // amsmath starts every right-hand column with `{}` so a
                        // leading relation (`= ...`) gets its spacing.
                        if !is_eqnarray
                            && j > 0
                            && aligns[j] == Align::Left
                            && aligns[j - 1] == Align::Right
                        {
                            tex = format!("{{}}{tex}");
                        }
                        let display = m.value().attr("display") == Some("block");
                        texs.push(tex.clone());
                        Some(self.math_job(tex, display, Some(mml)))
                    }
                    _ => None,
                };
                cells.push(job);
            }
            eq.rows.push(cells);
            eq.row_tex.push(texs.join(" "));
        }
        self.eq_tables.push(eq);
        let t = self.eq_tables.len() - 1;

        let mut i = 0;
        while i < rows.len() {
            let span = rows[i].span.min(rows.len() - i);
            let mut line = El::new("div").class("ltx_eq");
            let mut ids: Vec<String> = Vec::new();
            for r in &rows[i..i + span] {
                if let Some(id) = self.take_id(r.id.as_deref()) {
                    ids.push(id);
                }
            }
            let mut ids = ids.into_iter();
            if let Some(first) = ids.next() {
                line.set("id", first);
            }
            for extra in ids {
                line.children.push(El::new("span").attr("id", extra).into());
            }
            if let Some(no) = &rows[i].eqno {
                line.children
                    .push(El::new("span").class("ltx_eqno").text(no.clone()).into());
            }
            let slot = self.slot(Slot::EqRows {
                table: t,
                rows: i..i + span,
            });
            line.children.push(slot);
            group.children.push(line.into());
            i += span;
        }
        vec![group.into()]
    }

    fn note(&mut self, el: &ElementRef) -> Vec<Node> {
        let mark = child_elements(el)
            .find(|c| c.value().name() == "sup" && has(c, "ltx_note_mark"))
            .map(|c| plain_text(&c))
            .unwrap_or_default();
        let content = el.select(&sel(".ltx_note_content")).next();

        if has(el, "ltx_role_footnotemark") || content.is_none() {
            if mark.is_empty() {
                return vec![];
            }
            return vec![El::new("sup").class("ltx_note_mark").text(mark).into()];
        }
        let content = content.unwrap();

        self.note_seq += 1;
        let mark = if mark.is_empty() {
            self.note_seq.to_string()
        } else {
            mark
        };
        let id = self
            .take_id(el.value().attr("id"))
            .unwrap_or_else(|| self.fresh_id("fn"));
        let ref_id = self.fresh_id(&format!("{id}-ref"));

        // The content repeats the mark and a tag; skip those.
        let mut skipped_tag = false;
        let body_nodes: Vec<NodeRef<HNode>> = content
            .children()
            .filter(|n| {
                let Some(e) = ElementRef::wrap(*n) else {
                    return true;
                };
                if has(&e, "ltx_note_mark") || has(&e, "ltx_note_type") {
                    return false;
                }
                if has(&e, "ltx_tag_note") && !skipped_tag {
                    skipped_tag = true;
                    return false;
                }
                true
            })
            .collect();
        let body = self.nodes(body_nodes.into_iter());

        let back = El::new("a")
            .class("fn-back")
            .attr("href", format!("#{ref_id}"))
            .text(mark.clone());
        let mut p = El::new("p").child(back).text(" ");
        p.children.extend(trim_nodes(body));
        let aside = El::new("aside")
            .attr("epub:type", "footnote")
            .attr("id", id.clone())
            .class("footnote")
            .child(p);
        self.notes.push(aside.into());

        let link = El::new("a")
            .attr("epub:type", "noteref")
            .class("noteref")
            .attr("id", ref_id)
            .attr("href", format!("#{id}"))
            .child(El::new("sup").text(mark));
        vec![link.into()]
    }

    fn image(&mut self, el: &ElementRef, src: Option<&str>) -> Vec<Node> {
        let Some(src) = src.map(str::trim).filter(|s| !s.is_empty()) else {
            if has(el, "ltx_missing_image") || el.value().name() == "img" {
                let what = self.figure_label.clone().unwrap_or_else(|| "image".into());
                self.warnings
                    .push(format!("{what}: arXiv's HTML has no image file for it"));
                return vec![
                    El::new("span")
                        .class("ltx_missing")
                        .text(format!("[{what}: image missing from arXiv's HTML version]"))
                        .into(),
                ];
            }
            return self.children(el);
        };
        let Some(idx) = self.image_src(src) else {
            self.warnings.push(format!("bad image URL: {src}"));
            return vec![];
        };
        let width = el
            .value()
            .attr("width")
            .and_then(|w| w.trim_end_matches("px").parse::<f64>().ok());
        let alt = match el.value().attr("alt").map(str::trim) {
            Some(a) if !a.is_empty() && a != "Refer to caption" && a != "[Uncaptioned image]" => {
                a.to_string()
            }
            _ => self.figure_label.clone().unwrap_or_else(|| "image".into()),
        };
        let pct = width.map(|w| (w / self.graphics_ref_width * 100.0).clamp(5.0, 100.0));
        // Web images are sized like figures: their own width attribute, or
        // their pixel size, relative to a typical column.
        let in_figure = self.figure_depth > 0 || self.web.is_some();
        let slot = self.slot(Slot::Image {
            src: idx,
            width: pct,
            in_figure,
            alt,
        });
        match self.take_id(el.value().attr("id")) {
            Some(id) => vec![
                El::new("span")
                    .attr("id", id)
                    .class("ltx_graphics_wrap")
                    .child(slot)
                    .into(),
            ],
            None => vec![slot],
        }
    }

    fn inline_svg(&mut self, el: &ElementRef) -> Vec<Node> {
        let mut svg = String::new();
        let mut labels = Vec::new();
        self.svg_subtree(**el, &mut svg, true, &mut labels);
        let alt = self
            .figure_label
            .clone()
            .unwrap_or_else(|| "picture".into());
        // LaTeXML sizes pictures in px at 100 dpi, with 10pt text = 13.84px.
        let width = el
            .value()
            .attr("width")
            .and_then(|w| w.trim_end_matches("px").trim().parse::<f64>().ok());
        let style = width.filter(|w| *w > 0.0).map(|w| {
            if self.figure_depth > 0 {
                format!(
                    "width:{:.1}%",
                    (w / self.graphics_ref_width * 100.0).clamp(5.0, 100.0)
                )
            } else {
                format!("width:{:.2}em", w / LATEXML_PX_PER_EM)
            }
        });
        let slot = self.slot(Slot::Svg {
            svg,
            alt,
            labels,
            style,
        });
        match self.take_id(el.value().attr("id")) {
            Some(id) => vec![
                El::new("span")
                    .attr("id", id)
                    .class("ltx_graphics_wrap")
                    .child(slot)
                    .into(),
            ],
            None => vec![slot],
        }
    }

    /// Serialise an inline SVG, swapping each `foreignObject` for a marker.
    fn svg_subtree(
        &mut self,
        n: NodeRef<HNode>,
        out: &mut String,
        root: bool,
        labels: &mut Vec<SvgLabel>,
    ) {
        match n.value() {
            HNode::Text(t) => crate::tree::escape_text(t, out),
            HNode::Element(e) => {
                let name = e.name();
                if name == "foreignObject" {
                    let num = |k: &str| e.attr(k).and_then(|v| v.trim().parse::<f64>().ok());
                    let style = e.attr("style").unwrap_or("");
                    let var = |k: &str| {
                        style.split(';').find_map(|d| {
                            let (p, v) = d.split_once(':')?;
                            (p.trim() == k)
                                .then(|| v.trim().trim_end_matches("em").parse::<f64>().ok())
                                .flatten()
                        })
                    };
                    let font_pt = style
                        .split(';')
                        .find_map(|d| {
                            let (p, v) = d.split_once(':')?;
                            (p.trim() == "font-size")
                                .then(|| v.trim().trim_end_matches("pt").parse::<f64>().ok())
                                .flatten()
                        })
                        .unwrap_or(10.0);
                    let font = font_pt * 100.0 / 72.27;
                    if let Some((tex, mathml)) = label_tex(n) {
                        let job = self.math_job(tex, false, mathml);
                        labels.push(SvgLabel {
                            job,
                            width: num("width").unwrap_or(0.0),
                            height: num("height").unwrap_or(font),
                            baseline: var("--ltx-fo-height").map(|a| a * font),
                            font,
                        });
                        out.push_str("<g");
                        if let Some(t) = e.attr("transform") {
                            out.push_str(" transform=\"");
                            crate::tree::escape_attr(t, out);
                            out.push('"');
                        }
                        out.push_str(&format!(">\u{1}{}\u{1}</g>", labels.len() - 1));
                    }
                    return;
                }
                out.push('<');
                out.push_str(name);
                let mut seen = HashSet::new();
                if root {
                    out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
                    seen.insert("xmlns".to_string());
                }
                for (k, v) in e.attrs() {
                    if !seen.insert(k.to_string()) || k.contains(':') {
                        continue;
                    }
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    crate::tree::escape_attr(v, out);
                    out.push('"');
                }
                out.push('>');
                for c in n.children() {
                    self.svg_subtree(c, out, false, labels);
                }
                out.push_str("</");
                out.push_str(name);
                out.push('>');
            }
            _ => {}
        }
    }

    /// `\thanks` on the title: one footnote holding every note.
    fn pubnotes(&mut self, el: &ElementRef) -> Vec<Node> {
        let id = self.fresh_id("titlenote");
        let ref_id = self.fresh_id("titlenote-ref");
        let mut aside = El::new("aside")
            .attr("epub:type", "footnote")
            .attr("id", id.clone())
            .class("footnote");
        let mut first = true;
        for note in el.select(&sel(".ltx_pubnote")) {
            let mut p = El::new("p");
            if first {
                p.children.push(
                    El::new("a")
                        .class("fn-back")
                        .attr("href", format!("#{ref_id}"))
                        .text("*")
                        .into(),
                );
                p.children.push(Node::text(" "));
                first = false;
            }
            p.children.extend(trim_nodes(self.children(&note)));
            aside.children.push(p.into());
        }
        if first {
            return vec![];
        }
        self.notes.push(aside.into());
        let link = El::new("a")
            .attr("epub:type", "noteref")
            .class("noteref")
            .attr("id", ref_id)
            .attr("href", format!("#{id}"))
            .child(El::new("sup").text("*"));
        vec![link.into()]
    }

    fn link(&mut self, el: &ElementRef) -> Vec<Node> {
        let href = el.value().attr("href").map(str::trim).unwrap_or("");
        let target = if let Some(frag) = href.strip_prefix('#') {
            (!frag.is_empty()).then(|| format!("#{}", sanitize_id(frag)))
        } else if href.is_empty() || href.starts_with("javascript:") || href.starts_with("data:") {
            None
        } else {
            self.base
                .join(href)
                .ok()
                .filter(|u| match u.scheme() {
                    "http" | "https" | "ftp" => u.host_str().is_some_and(|h| !h.is_empty()),
                    "mailto" => u.path().contains('@'),
                    _ => false,
                })
                .map(|mut u| {
                    // A link to a spot on this same page is an internal link.
                    let mut page = self.base.clone();
                    page.set_fragment(None);
                    let mut target = u.clone();
                    target.set_fragment(None);
                    if target == page
                        && let Some(f) = u.fragment().filter(|f| !f.is_empty())
                    {
                        return format!("#{}", sanitize_id(f));
                    }
                    // A '#' inside the fragment is not a valid URL ("...##x").
                    if let Some(f) = u
                        .fragment()
                        .filter(|f| f.contains('#'))
                        .map(|f| f.replace('#', "%23"))
                    {
                        u.set_fragment(Some(&f));
                    }
                    u.to_string()
                })
        };
        let mut a = self.base_el(if target.is_some() { "a" } else { "span" }, el);
        if let Some(t) = target {
            a.set("href", t);
        }
        a.children = self.children(el);
        vec![a.into()]
    }

    fn heading(&mut self, el: &ElementRef) -> Vec<Node> {
        let name = el.value().name();
        let mut tag: &'static str = match name {
            "h1" => "h1",
            "h2" => "h2",
            "h3" => "h3",
            "h4" => "h4",
            "h5" => "h5",
            _ => "h6",
        };
        if has(el, "ltx_title_abstract") {
            tag = "h2";
        }
        let mut h = self.base_el(tag, el);
        h.children = self.children(el);

        // Table of contents: section-level titles, anchored on their section.
        let parent = el.parent().and_then(ElementRef::wrap);
        let level = parent.as_ref().and_then(|p| {
            if has(el, "ltx_title_abstract")
                || [
                    "ltx_section",
                    "ltx_appendix",
                    "ltx_bibliography",
                    "ltx_acknowledgements",
                    "ltx_index",
                    "ltx_chapter",
                    "ltx_part",
                ]
                .iter()
                .any(|c| has(p, c))
            {
                Some(1)
            } else if has(p, "ltx_subsection") {
                Some(2)
            } else if has(p, "ltx_subsubsection") {
                Some(3)
            } else {
                None
            }
        });
        if let (Some(level), Some(parent)) = (level, parent) {
            let label = plain_text(el);
            if !label.is_empty() {
                let pid = parent.value().attr("id").map(sanitize_id);
                let id = match pid.filter(|p| self.ids.contains(p)) {
                    Some(p) => p,
                    None => {
                        let id = h
                            .get("id")
                            .map(str::to_string)
                            .unwrap_or_else(|| self.fresh_id("sec"));
                        h.set("id", id.clone());
                        id
                    }
                };
                self.toc.push(TocEntry {
                    level,
                    label,
                    id: Some(id),
                });
            }
        }
        vec![h.into()]
    }

    fn figure(&mut self, el: &ElementRef) -> Vec<Node> {
        let label = el
            .select(&sel("figcaption .ltx_tag"))
            .next()
            .map(|t| plain_text(&t).trim_end_matches(':').trim().to_string())
            .filter(|t| !t.is_empty());
        let inherited = label.or_else(|| self.figure_label.clone());
        let saved = std::mem::replace(&mut self.figure_label, inherited);
        self.figure_depth += 1;
        let mut fig = self.base_el("figure", el);
        fig.children = self.children(el);
        self.figure_depth -= 1;
        self.figure_label = saved;
        if fig.children.iter().all(Node::is_blank_text) {
            return vec![];
        }
        vec![fig.into()]
    }

    fn list(&mut self, el: &ElementRef) -> Vec<Node> {
        let ordered = el.value().name() == "ol";
        let items: Vec<ElementRef> = child_elements(el)
            .filter(|c| c.value().name() == "li")
            .collect();
        let tag_of = |li: &ElementRef| -> Option<String> {
            child_elements(li)
                .find(|c| has(c, "ltx_tag"))
                .map(|t| plain_text(&t))
        };
        let tags: Vec<Option<String>> = items.iter().map(tag_of).collect();

        // Use the reader's own list rendering when LaTeX's labels are the
        // standard ones; otherwise keep the labels as text.
        let mut start = 1;
        let native = if ordered {
            let nums: Option<Vec<i64>> = tags
                .iter()
                .map(|t| {
                    t.as_deref()
                        .and_then(|t| t.strip_suffix('.'))
                        .and_then(|n| n.parse().ok())
                })
                .collect();
            match nums {
                Some(n) if !n.is_empty() => {
                    start = n[0];
                    n.windows(2).all(|w| w[1] == w[0] + 1)
                }
                _ => false,
            }
        } else {
            tags.iter().all(|t| {
                t.as_deref()
                    .is_none_or(|t| t.is_empty() || BULLETS.contains(&t))
            })
        };

        let mut list = self.base_el(if ordered { "ol" } else { "ul" }, el);
        if !native {
            let c = list
                .get("class")
                .map(|c| format!("{c} ltx_custom_list"))
                .unwrap_or("ltx_custom_list".into());
            list.set("class", c);
        } else if ordered && start != 1 {
            list.set("start", start.to_string());
        }

        for li in child_elements(el) {
            if li.value().name() != "li" {
                continue;
            }
            let mut item = self.base_el("li", &li);
            let mut tag_node = None;
            let rest: Vec<NodeRef<HNode>> = li
                .children()
                .filter(|n| {
                    if let Some(e) = ElementRef::wrap(*n)
                        && tag_node.is_none()
                        && has(&e, "ltx_tag")
                    {
                        tag_node = Some(e);
                        return false;
                    }
                    true
                })
                .collect();
            let mut body = self.nodes(rest.into_iter());
            if !native && let Some(t) = tag_node {
                let mut span = self.base_el("span", &t);
                span.children = self.children(&t);
                let tag: Node = span.into();
                if let Err(tag) = prepend_to_first_p(&mut body, tag) {
                    body.insert(0, tag);
                }
            }
            item.children = body;
            list.children.push(item.into());
        }
        vec![list.into()]
    }
}

/// LaTeXML's px per em of 10pt text (it typesets at 100 dpi).
const LATEXML_PX_PER_EM: f64 = 10.0 * 100.0 / 72.27;

/// Escape plain text for `\text{...}`.
fn tex_text_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\backslash "),
            '{' | '}' | '$' | '&' | '#' | '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            '^' => out.push_str("\\hat{}"),
            '~' => out.push_str("\\sim "),
            c => out.push(c),
        }
    }
    out
}

/// TeX for a picture label (the content of a `foreignObject`): text becomes
/// `\text{}`, math keeps its TeX, `<br>` starts a new line. Also returns the
/// MathML when the label is a single equation, as a fallback.
fn label_tex(fo: NodeRef<HNode>) -> Option<(String, Option<String>)> {
    enum Piece {
        Text(String),
        Math(String),
    }
    fn go(n: NodeRef<HNode>, lines: &mut Vec<Vec<Piece>>, maths: &mut Vec<String>) {
        match n.value() {
            HNode::Text(t) => {
                let line = lines.last_mut().unwrap();
                match line.last_mut() {
                    Some(Piece::Text(s)) => s.push_str(t),
                    _ => line.push(Piece::Text(t.to_string())),
                }
            }
            HNode::Element(e) => {
                let er = ElementRef::wrap(n).unwrap();
                match e.name() {
                    "math" => {
                        let mut mml = String::new();
                        xml_subtree(n, &mut mml, Some("http://www.w3.org/1998/Math/MathML"));
                        maths.push(mml);
                        lines.last_mut().unwrap().push(Piece::Math(math_tex(&er)));
                    }
                    "br" => lines.push(Vec::new()),
                    "svg" | "button" => {}
                    _ => {
                        for c in n.children() {
                            go(c, lines, maths);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut lines = vec![Vec::new()];
    let mut maths = Vec::new();
    for c in fo.children() {
        go(c, &mut lines, &mut maths);
    }
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|p| match p {
                    Piece::Text(t) => {
                        // Collapse runs of whitespace but keep the edges, so
                        // "of $x$ and" keeps its spaces around the math.
                        let mut c = collapse_ws(t);
                        if c.is_empty() {
                            return if t.is_empty() {
                                String::new()
                            } else {
                                "\\ ".to_string()
                            };
                        }
                        if t.starts_with(char::is_whitespace) {
                            c.insert(0, ' ');
                        }
                        if t.ends_with(char::is_whitespace) {
                            c.push(' ');
                        }
                        format!("\\text{{{}}}", tex_text_escape(&c))
                    }
                    Piece::Math(m) => format!("{{{m}}}"),
                })
                .collect::<String>()
        })
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l != "\\")
        .collect();
    let single_math = maths.len() == 1
        && lines.iter().flatten().all(|p| match p {
            Piece::Text(t) => t.trim().is_empty(),
            Piece::Math(_) => true,
        });
    let tex = match rendered.len() {
        0 => return None,
        1 => rendered.into_iter().next().unwrap(),
        _ => format!(
            "\\begin{{array}}{{c}}{}\\end{{array}}",
            rendered.join("\\\\")
        ),
    };
    Some((tex, single_math.then(|| maths.pop().unwrap())))
}

fn is_heading(name: &str) -> bool {
    matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

/// Insert `node` (plus a space) at the start of the first paragraph in
/// `nodes`, looking through wrapper divs. Gives the node back if the content
/// doesn't start with a paragraph.
fn prepend_to_first_p(nodes: &mut [Node], node: Node) -> Result<(), Node> {
    for n in nodes.iter_mut() {
        if n.is_blank_text() {
            continue;
        }
        let Node::El(e) = n else { return Err(node) };
        return match e.tag {
            "p" => {
                e.children.insert(0, Node::text(" "));
                e.children.insert(0, node);
                Ok(())
            }
            "div" | "section" => prepend_to_first_p(&mut e.children, node),
            _ => Err(node),
        };
    }
    Err(node)
}

/// Strip leading/trailing whitespace text from a node list.
fn trim_nodes(mut nodes: Vec<Node>) -> Vec<Node> {
    while nodes.first().is_some_and(Node::is_blank_text) {
        nodes.remove(0);
    }
    while nodes.last().is_some_and(Node::is_blank_text) {
        nodes.pop();
    }
    if let Some(Node::Text(t)) = nodes.first_mut() {
        *t = t.trim_start().to_string();
    }
    if let Some(Node::Text(t)) = nodes.last_mut() {
        *t = t.trim_end().to_string();
    }
    nodes
}

fn author_names(article: &ElementRef) -> Vec<String> {
    let mut names = Vec::new();
    for person in article.select(&sel(".ltx_authors .ltx_personname")) {
        // Text up to the first line break: later lines are affiliations/emails.
        let mut first = String::new();
        let mut stop = false;
        fn go(n: NodeRef<HNode>, out: &mut String, stop: &mut bool) {
            if *stop {
                return;
            }
            match n.value() {
                HNode::Text(t) => out.push_str(t),
                HNode::Element(e) => {
                    let er = ElementRef::wrap(n).unwrap();
                    if e.name() == "br" {
                        *stop = true;
                        return;
                    }
                    if has(&er, "ltx_note") || e.name() == "sup" {
                        return;
                    }
                    if e.name() == "math" {
                        out.push_str(&plain_text(&er));
                        return;
                    }
                    for c in n.children() {
                        go(c, out, stop);
                    }
                }
                _ => {}
            }
        }
        go(*person, &mut first, &mut stop);
        // LaTeXML runs co-authors on one line, separated by wide spaces or commas/"and".
        for part in first.split("  ") {
            for name in part.split([',', ';']).flat_map(|p| p.split(" and ")) {
                let name = collapse_ws(name);
                if !name.is_empty()
                    && name.chars().any(char::is_alphabetic)
                    && !names.contains(&name)
                {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// Fetches a supporting file (e.g. a Distill page's bibliography) by URL.
pub type Fetch<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

pub fn convert(html: &str, base: Url) -> Result<Book> {
    convert_with(html, base, &|_| None)
}

pub fn convert_with(html: &str, base: Url, fetch: Fetch) -> Result<Book> {
    let doc = Html::parse_document(html);
    let Some(article) = doc.select(&sel("article.ltx_document")).next() else {
        // Not an arXiv/LaTeXML paper: treat it as a general web page.
        return web::convert(&doc, base, fetch);
    };

    let doc_title = article
        .select(&sel(".ltx_title_document"))
        .next()
        .map(|t| plain_text(&t))
        .filter(|t| !t.is_empty());
    let has_title = doc_title.is_some();
    let title = doc_title
        .or_else(|| {
            doc.select(&sel("title"))
                .next()
                .map(|t| collapse_ws(&t.text().collect::<String>()))
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Untitled".into());
    let authors = author_names(&article);
    let lang = doc
        .root_element()
        .value()
        .attr("lang")
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or("en")
        .to_string();

    let widths: Vec<f64> = article
        .select(&sel(".ltx_graphics"))
        .filter_map(|g| {
            g.value()
                .attr("width")?
                .trim_end_matches("px")
                .parse::<f64>()
                .ok()
        })
        .collect();
    // LaTeXML gives each graphic its typeset width in px. The most common width
    // is almost always \linewidth, so figures are sized as a share of that.
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for w in &widths {
        *counts.entry(w.round() as i64).or_default() += 1;
    }
    let mode = counts
        .iter()
        .max_by_key(|(w, n)| (**n, **w))
        .filter(|(_, n)| **n >= 2)
        .map(|(w, _)| *w as f64);
    let graphics_ref_width = mode
        .unwrap_or_else(|| widths.iter().cloned().fold(0.0, f64::max))
        .clamp(300.0, 800.0);

    let mut cv = Conv::new(
        base,
        if widths.is_empty() {
            476.0
        } else {
            graphics_ref_width
        },
    );

    // Split: front matter, then one chapter per top-level <section>.
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut front: Vec<Node> = Vec::new();
    cv.toc.push(TocEntry {
        level: 1,
        label: title.clone(),
        id: None,
    });

    for child in article.children() {
        let el = ElementRef::wrap(child);
        let is_section = el.as_ref().is_some_and(|e| e.value().name() == "section");
        if is_section {
            let e = el.unwrap();
            if chapters.is_empty() {
                chapters.push(finish_chapter(
                    &mut cv,
                    "title.xhtml",
                    title.clone(),
                    std::mem::take(&mut front),
                ));
            }
            let heading = child_elements(&e).find(|c| is_heading(c.value().name()));
            let ch_title = heading
                .map(|h| plain_text(&h))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| title.clone());
            let body = cv.element(&e);
            let file = format!("ch{:02}.xhtml", chapters.len());
            chapters.push(finish_chapter(&mut cv, &file, ch_title, body));
        } else {
            let nodes = cv.node(child);
            match chapters.last_mut() {
                Some(last) => {
                    last.body.extend(nodes);
                    last.notes.append(&mut cv.notes);
                    last.toc.append(&mut cv.toc);
                }
                None => front.extend(nodes),
            }
        }
    }
    if chapters.is_empty() {
        chapters.push(finish_chapter(&mut cv, "title.xhtml", title.clone(), front));
    }

    // Front-matter notes (\thanks) and any trailing content stay on the title page.
    let mut book = Book {
        title,
        math_preamble: String::new(),
        has_title,
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

fn finish_chapter(cv: &mut Conv, file: &str, title: String, body: Vec<Node>) -> Chapter {
    Chapter {
        file: file.to_string(),
        title,
        body,
        notes: std::mem::take(&mut cv.notes),
        toc: std::mem::take(&mut cv.toc),
    }
}

/// Point every internal `#id` link at the chapter holding that id; links to
/// ids that didn't make it into the book become plain text.
fn resolve_links(book: &mut Book) {
    let mut where_is: HashMap<String, usize> = HashMap::new();
    for (i, ch) in book.chapters.iter().enumerate() {
        let mut f = |e: &El| {
            if let Some(id) = e.get("id") {
                where_is.entry(id.to_string()).or_insert(i);
            }
        };
        walk(&ch.body, &mut f);
        walk(&ch.notes, &mut f);
    }
    let files: Vec<String> = book.chapters.iter().map(|c| c.file.clone()).collect();
    let mut dangling = 0usize;
    for (i, ch) in book.chapters.iter_mut().enumerate() {
        let mut f = |e: &mut El| {
            if e.tag != "a" {
                return;
            }
            let Some(href) = e.get("href") else { return };
            let Some(frag) = href.strip_prefix('#') else {
                return;
            };
            match where_is.get(frag) {
                Some(&j) if j == i => {}
                Some(&j) => {
                    let h = format!("{}#{frag}", files[j]);
                    e.set("href", h);
                }
                None => {
                    e.remove("href");
                    e.remove("epub:type");
                    e.tag = "span";
                    dangling += 1;
                }
            }
        };
        walk_mut(&mut ch.body, &mut f);
        walk_mut(&mut ch.notes, &mut f);
    }
    if dangling > 0 {
        book.warnings.push(format!("{dangling} internal links pointed at content that isn't in the page; kept as text"));
    }
}
