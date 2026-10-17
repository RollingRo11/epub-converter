//! A small output DOM for the XHTML we write, plus the passes that make it
//! valid EPUB content no matter what the source looked like:
//!
//! * `normalize` enforces the HTML content model (no blocks inside `<p>`,
//!   only `<li>` in lists, no nested links, ...), splitting elements as needed;
//! * `serialize` writes well-formed XML with every string escaped and
//!   characters that XML forbids removed.

use std::fmt::Write;

#[derive(Debug, Clone)]
pub enum Node {
    El(El),
    Text(String),
    /// Placeholder for an image filled in after equations/figures are rendered.
    Slot(usize),
}

#[derive(Debug, Clone)]
pub struct El {
    pub tag: &'static str,
    pub attrs: Vec<(&'static str, String)>,
    pub children: Vec<Node>,
}

impl El {
    pub fn new(tag: &'static str) -> Self {
        Self {
            tag,
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn attr(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.set(name, value);
        self
    }

    pub fn class(self, value: impl Into<String>) -> Self {
        self.attr("class", value)
    }

    pub fn child(mut self, node: impl Into<Node>) -> Self {
        self.children.push(node.into());
        self
    }

    pub fn text(self, text: impl Into<String>) -> Self {
        self.child(Node::Text(text.into()))
    }

    pub fn with_children(mut self, nodes: Vec<Node>) -> Self {
        self.children.extend(nodes);
        self
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, name: &'static str, value: impl Into<String>) {
        let value = value.into();
        match self.attrs.iter_mut().find(|(k, _)| *k == name) {
            Some(slot) => slot.1 = value,
            None => self.attrs.push((name, value)),
        }
    }

    pub fn remove(&mut self, name: &str) -> Option<String> {
        let i = self.attrs.iter().position(|(k, _)| *k == name)?;
        Some(self.attrs.remove(i).1)
    }
}

impl From<El> for Node {
    fn from(el: El) -> Self {
        Node::El(el)
    }
}

impl Node {
    pub fn text(s: impl Into<String>) -> Self {
        Node::Text(s.into())
    }

    pub fn is_blank_text(&self) -> bool {
        matches!(self, Node::Text(t) if t.trim().is_empty())
    }

    pub fn as_el(&self) -> Option<&El> {
        match self {
            Node::El(e) => Some(e),
            _ => None,
        }
    }

    pub fn as_el_mut(&mut self) -> Option<&mut El> {
        match self {
            Node::El(e) => Some(e),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Content model

/// Elements that are phrasing content (may sit inside a paragraph).
pub fn is_phrasing(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "abbr"
            | "b"
            | "bdi"
            | "bdo"
            | "br"
            | "cite"
            | "code"
            | "dfn"
            | "em"
            | "i"
            | "img"
            | "kbd"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "u"
            | "var"
            | "wbr"
            | "del"
            | "ins"
    )
}

/// Elements whose content must be phrasing only.
fn phrasing_only(tag: &str) -> bool {
    is_phrasing(tag)
        || matches!(
            tag,
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre" | "dt"
        )
}

fn node_is_phrasing(n: &Node) -> bool {
    match n {
        Node::Text(_) => true,
        // Slots become <img> or a block equation; layout tells us which by
        // wrapping block slots in a <div> before normalisation.
        Node::Slot(_) => true,
        Node::El(e) => is_phrasing(e.tag),
    }
}

/// Make the tree obey the content model. Returns the node(s) that replace `node`.
pub fn normalize(node: Node) -> Vec<Node> {
    let mut out = normalize_in(node, false);
    stray_captions(&mut out);
    out
}

/// A `<figcaption>` outside a `<figure>` is invalid: make it a caption block.
fn stray_captions(nodes: &mut [Node]) {
    for n in nodes {
        if let Node::El(e) = n
            && e.tag == "figcaption"
        {
            e.tag = "div";
            let class = e
                .get("class")
                .map_or("ltx_caption".to_string(), |c| format!("{c} ltx_caption"));
            e.set("class", class);
        }
    }
}

fn normalize_in(node: Node, in_link: bool) -> Vec<Node> {
    let Node::El(mut el) = node else {
        return vec![node];
    };

    // Nested links are invalid: keep the inner link's content only.
    let unwrap_link = el.tag == "a" && in_link;
    let child_in_link = in_link || el.tag == "a";

    let mut children = Vec::with_capacity(el.children.len());
    for c in std::mem::take(&mut el.children) {
        children.extend(normalize_in(c, child_in_link));
    }

    if unwrap_link {
        return children;
    }

    match el.tag {
        "ul" | "ol" => {
            el.children = wrap_runs(
                children,
                |n| n.as_el().is_some_and(|e| e.tag == "li"),
                || El::new("li"),
            );
            return vec![Node::El(el)];
        }
        "dl" => {
            el.children = wrap_runs(
                children,
                |n| n.as_el().is_some_and(|e| e.tag == "dt" || e.tag == "dd"),
                || El::new("dd"),
            );
            return vec![Node::El(el)];
        }
        "tr" => {
            el.children = wrap_runs(
                children,
                |n| n.as_el().is_some_and(|e| e.tag == "td" || e.tag == "th"),
                || El::new("td"),
            );
            return vec![Node::El(el)];
        }
        "thead" | "tbody" | "tfoot" => {
            el.children = wrap_runs(
                children,
                |n| n.as_el().is_some_and(|e| e.tag == "tr"),
                || El::new("tr"),
            );
            return vec![Node::El(el)];
        }
        "table" => {
            el.children = normalize_table(children);
            return vec![Node::El(el)];
        }
        "figure" => {
            el.children = normalize_figure(children);
            return vec![Node::El(el)];
        }
        _ => {}
    }

    // A <div> inside a heading (common on web pages) is just a wrapper for
    // the heading's text: keep it inline instead of splitting the heading.
    if matches!(el.tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
        for c in &mut children {
            if let Node::El(e) = c
                && e.tag == "div"
                && e.children.iter().all(node_is_phrasing)
            {
                e.tag = "span";
            }
        }
    }

    if el.tag != "figure" {
        stray_captions(&mut children);
    }

    if !phrasing_only(el.tag) || children.iter().all(node_is_phrasing) {
        el.children = children;
        return vec![Node::El(el)];
    }

    // Split this phrasing-only element around its block children:
    // <p>a<div/>b</p>  ->  <p>a</p><div/><p>b</p>
    let mut out = Vec::new();
    let mut run: Vec<Node> = Vec::new();
    let mut id = el.remove("id");
    let template = El {
        tag: el.tag,
        attrs: el.attrs,
        children: Vec::new(),
    };

    let flush = |run: &mut Vec<Node>, out: &mut Vec<Node>, id: &mut Option<String>| {
        if run.iter().all(Node::is_blank_text) {
            run.clear();
            return;
        }
        let mut part = template.clone();
        if let Some(id) = id.take() {
            part.set("id", id);
        }
        part.children = std::mem::take(run);
        out.push(Node::El(part));
    };

    for c in children {
        if node_is_phrasing(&c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out, &mut id);
            out.push(c);
        }
    }
    flush(&mut run, &mut out, &mut id);

    // Nothing inline survived to carry the id: keep it on an empty anchor.
    if let Some(id) = id {
        out.insert(0, Node::El(El::new("span").attr("id", id)));
    }
    out
}

/// Wrap every maximal run of children failing `ok` in a fresh element.
fn wrap_runs(children: Vec<Node>, ok: impl Fn(&Node) -> bool, make: impl Fn() -> El) -> Vec<Node> {
    let mut out = Vec::new();
    let mut run: Vec<Node> = Vec::new();
    for c in children {
        if ok(&c) {
            if !run.iter().all(Node::is_blank_text) {
                out.push(Node::El(make().with_children(std::mem::take(&mut run))));
            }
            run.clear();
            out.push(c);
        } else {
            run.push(c);
        }
    }
    if !run.iter().all(Node::is_blank_text) {
        out.push(Node::El(make().with_children(run)));
    }
    out
}

/// A table holds an optional caption, then either row groups or bare rows.
fn normalize_table(children: Vec<Node>) -> Vec<Node> {
    let mut caption = None;
    let mut groups: Vec<Node> = Vec::new();
    let mut loose: Vec<Node> = Vec::new();

    let flush = |loose: &mut Vec<Node>, groups: &mut Vec<Node>| {
        if !loose.is_empty() {
            let body = El::new("tbody").with_children(std::mem::take(loose));
            groups.extend(normalize(Node::El(body)));
        }
    };

    for c in children {
        if c.is_blank_text() {
            continue;
        }
        match c.as_el().map(|e| e.tag) {
            Some("caption") if caption.is_none() => caption = Some(c),
            Some("thead" | "tbody" | "tfoot") => {
                flush(&mut loose, &mut groups);
                groups.push(c);
            }
            _ => loose.push(c),
        }
    }
    flush(&mut loose, &mut groups);

    // thead must come first and tfoot last.
    groups.sort_by_key(|g| match g.as_el().map(|e| e.tag) {
        Some("thead") => 0,
        Some("tfoot") => 2,
        _ => 1,
    });
    let mut seen_head = false;
    let mut seen_foot = false;
    for g in &mut groups {
        if let Some(e) = g.as_el_mut() {
            if e.tag == "thead" {
                if seen_head {
                    e.tag = "tbody";
                }
                seen_head = true;
            } else if e.tag == "tfoot" {
                if seen_foot {
                    e.tag = "tbody";
                }
                seen_foot = true;
            }
        }
    }

    caption.into_iter().chain(groups).collect()
}

/// A figcaption must be the first or last child of its figure.
fn normalize_figure(mut children: Vec<Node>) -> Vec<Node> {
    children.retain(|c| !c.is_blank_text());
    let last = children.len().saturating_sub(1);
    let mut kept_caption = false;
    for (i, c) in children.iter_mut().enumerate() {
        if let Some(e) = c.as_el_mut()
            && e.tag == "figcaption"
        {
            if (i == 0 || i == last) && !kept_caption {
                kept_caption = true;
            } else {
                e.tag = "div";
            }
        }
    }
    children
}

// ---------------------------------------------------------------------------
// Serialisation

fn is_xml_char(c: char) -> bool {
    matches!(c, '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

pub fn escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if is_xml_char(c) => out.push(c),
            _ => {}
        }
    }
}

pub fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#9;"),
            '\r' => {}
            c if is_xml_char(c) => out.push(c),
            _ => {}
        }
    }
}

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_attr(s, &mut out);
    out
}

fn is_void(tag: &str) -> bool {
    matches!(tag, "img" | "br" | "hr" | "wbr" | "meta" | "link" | "col")
}

pub fn serialize(nodes: &[Node], out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => escape_text(t, out),
            Node::Slot(i) => panic!("unfilled image slot {i}"),
            Node::El(e) => {
                out.push('<');
                out.push_str(e.tag);
                for (k, v) in &e.attrs {
                    let _ = write!(out, " {k}=\"");
                    escape_attr(v, out);
                    out.push('"');
                }
                if is_void(e.tag) {
                    out.push_str("/>");
                } else {
                    out.push('>');
                    serialize(&e.children, out);
                    let _ = write!(out, "</{}>", e.tag);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Walking

pub fn walk_mut(nodes: &mut [Node], f: &mut impl FnMut(&mut El)) {
    for n in nodes {
        if let Node::El(e) = n {
            f(e);
            walk_mut(&mut e.children, f);
        }
    }
}

pub fn walk(nodes: &[Node], f: &mut impl FnMut(&El)) {
    for n in nodes {
        if let Node::El(e) = n {
            f(e);
            walk(&e.children, f);
        }
    }
}

/// Replace every `Slot` using `fill`.
pub fn fill_slots(nodes: &mut [Node], fill: &impl Fn(usize) -> Node) {
    for n in nodes.iter_mut() {
        match n {
            Node::Slot(i) => *n = fill(*i),
            Node::El(e) => fill_slots(&mut e.children, fill),
            Node::Text(_) => {}
        }
    }
}

/// Turn an arbitrary string into a valid XML ID (an NCName).
pub fn sanitize_id(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        s.insert(0, 'x');
    }
    s
}
