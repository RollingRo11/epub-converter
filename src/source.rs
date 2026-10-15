//! Where the paper comes from: an arXiv id / URL, or a local HTML file.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use rayon::prelude::*;
use url::Url;

const USER_AGENT: &str = concat!(
    "epub-converter/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/RollingRo11/epub-converter)"
);
const MAX_BYTES: u64 = 200 * 1024 * 1024;

pub enum Input {
    Arxiv(String),
    Url(Url),
    File(PathBuf),
}

pub struct Paper {
    pub html: String,
    pub base: Url,
    pub arxiv_id: Option<String>,
}

fn is_new_id(s: &str) -> bool {
    // 2501.14926, 2501.14926v4, 0704.0001
    let (core, ver) = match s.find('v') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let Some((a, b)) = core.split_once('.') else {
        return false;
    };
    a.len() == 4
        && (4..=5).contains(&b.len())
        && a.chars().chain(b.chars()).all(|c| c.is_ascii_digit())
        && ver.is_none_or(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
}

fn is_old_id(s: &str) -> bool {
    // hep-th/9901001, math.GT/0309136, with optional vN
    let Some((arch, num)) = s.split_once('/') else {
        return false;
    };
    let num = num.split('v').next().unwrap_or("");
    !arch.is_empty()
        && arch
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '.')
        && num.len() == 7
        && num.chars().all(|c| c.is_ascii_digit())
}

pub fn arxiv_id(s: &str) -> Option<String> {
    let s = s.trim();
    let s = s
        .strip_prefix("arXiv:")
        .or_else(|| s.strip_prefix("arxiv:"))
        .unwrap_or(s);
    let s = s
        .strip_prefix("10.48550/arXiv.")
        .or_else(|| s.strip_prefix("https://doi.org/10.48550/arXiv."))
        .unwrap_or(s);
    let s = s.trim_end_matches('/').trim_end_matches(".pdf");
    (is_new_id(s) || is_old_id(s)).then(|| s.to_string())
}

pub fn parse_input(raw: &str) -> Result<Input> {
    if let Some(id) = arxiv_id(raw) {
        return Ok(Input::Arxiv(id));
    }
    if let Ok(url) = Url::parse(raw) {
        if matches!(url.scheme(), "http" | "https") {
            let host = url.host_str().unwrap_or("");
            if matches!(host, "arxiv.org" | "www.arxiv.org" | "export.arxiv.org") {
                let path = url.path().trim_start_matches('/');
                for prefix in ["abs/", "pdf/", "html/", "format/"] {
                    if let Some(rest) = path.strip_prefix(prefix) {
                        // /html/2501.14926v4/ or /html/2501.14926v4/index.html
                        let candidate = rest.trim_end_matches(".pdf");
                        let mut parts = candidate.split('/');
                        let first = parts.next().unwrap_or("");
                        let second = parts.next().unwrap_or("");
                        if let Some(id) = arxiv_id(first) {
                            return Ok(Input::Arxiv(id));
                        }
                        if let Some(id) = arxiv_id(&format!("{first}/{second}")) {
                            return Ok(Input::Arxiv(id));
                        }
                    }
                }
            }
            return Ok(Input::Url(url));
        }
        if url.scheme() == "file"
            && let Ok(p) = url.to_file_path()
        {
            return Ok(Input::File(p));
        }
    }
    let p = PathBuf::from(raw);
    if p.exists() {
        return Ok(Input::File(p));
    }
    bail!("`{raw}` is not an arXiv id, an arXiv/HTML URL, or an existing file")
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_global(Some(Duration::from_secs(90)))
            .max_redirects(10)
            .build()
            .into()
    })
}

/// GET with retries on network errors, 429 and 5xx. Returns body and final URL.
fn get(url: &str) -> Result<(Vec<u8>, String), String> {
    use ureq::ResponseExt;
    let mut last = String::new();
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500 << attempt));
        }
        match agent().get(url).call() {
            Ok(res) => {
                let final_url = res.get_uri().to_string();
                return res
                    .into_body()
                    .with_config()
                    .limit(MAX_BYTES)
                    .read_to_vec()
                    .map(|b| (b, final_url))
                    .map_err(|e| format!("{url}: {e}"));
            }
            Err(ureq::Error::StatusCode(code)) if code == 429 || code >= 500 => {
                last = format!("{url}: HTTP {code}");
            }
            Err(ureq::Error::StatusCode(code)) => return Err(format!("{url}: HTTP {code}")),
            Err(e) => last = format!("{url}: {e}"),
        }
    }
    Err(last)
}

fn decode_data_uri(uri: &str) -> Result<Vec<u8>, String> {
    let rest = uri.strip_prefix("data:").ok_or("not a data URI")?;
    let (meta, data) = rest.split_once(',').ok_or("malformed data URI")?;
    if meta.ends_with(";base64") {
        let cleaned: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD
            .decode(cleaned)
            .map_err(|e| format!("bad base64 in data URI: {e}"))
    } else {
        Ok(percent_decode(data))
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

pub fn fetch_paper(input: &Input, base_override: Option<&str>) -> Result<Paper> {
    let (html, base, arxiv_id) = match input {
        Input::Arxiv(id) => {
            let url = format!("https://arxiv.org/html/{id}");
            // ar5iv (arXiv's older LaTeXML mirror) sometimes has papers that
            // arxiv.org/html lacks.
            let (body, final_url) = match get(&url) {
                Ok(r) => r,
                Err(e) if e.ends_with("HTTP 404") => get(&format!("https://ar5iv.labs.arxiv.org/html/{id}"))
                    .ok()
                    .filter(|(b, _)| String::from_utf8_lossy(b).contains("ltx_document"))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "arXiv has no HTML version of {id} (only LaTeX submissions get one, and some \
                             conversions fail). The PDF can be sent to a Kindle directly."
                        )
                    })?,
                Err(e) => bail!(e),
            };
            let base = Url::parse(&final_url).context("bad final URL")?;
            // Version actually served (e.g. 2501.14926 -> 2501.14926v4).
            let served = base
                .path_segments()
                .and_then(|mut s| s.find(|seg| arxiv_id(seg).is_some()))
                .map(str::to_string)
                .unwrap_or_else(|| id.clone());
            (
                String::from_utf8_lossy(&body).into_owned(),
                base,
                Some(served),
            )
        }
        Input::Url(url) => {
            let (body, final_url) = get(url.as_str()).map_err(anyhow::Error::msg)?;
            (
                String::from_utf8_lossy(&body).into_owned(),
                Url::parse(&final_url)?,
                None,
            )
        }
        Input::File(path) => {
            let body =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let abs = std::fs::canonicalize(path)?;
            let base = Url::from_file_path(&abs)
                .map_err(|_| anyhow::anyhow!("bad path {}", abs.display()))?;
            (String::from_utf8_lossy(&body).into_owned(), base, None)
        }
    };
    let base = match base_override {
        Some(b) => Url::parse(b).with_context(|| format!("bad --base-url {b}"))?,
        None => base,
    };
    Ok(Paper {
        html,
        base,
        arxiv_id,
    })
}

/// Title and authors from the arXiv API (authoritative, unlike the byline
/// heuristics). None on any failure: the HTML's own metadata is used instead.
pub fn arxiv_metadata(id: &str) -> Option<(String, Vec<String>)> {
    let url = format!("https://export.arxiv.org/api/query?id_list={id}");
    let (body, _) = get(&url).ok()?;
    let text = String::from_utf8(body).ok()?;
    let doc = resvg::usvg::roxmltree::Document::parse(&text).ok()?;
    let entry = doc.descendants().find(|n| n.has_tag_name("entry"))?;
    fn child_text(n: resvg::usvg::roxmltree::Node, tag: &str) -> Option<String> {
        let t = n.children().find(|c| c.has_tag_name(tag))?.text()?;
        Some(t.split_whitespace().collect::<Vec<_>>().join(" "))
    }
    let title = child_text(entry, "title").filter(|t| !t.is_empty())?;
    let authors: Vec<String> = entry
        .children()
        .filter(|c| c.has_tag_name("author"))
        .filter_map(|a| child_text(a, "name"))
        .filter(|n| !n.is_empty())
        .collect();
    Some((title, authors))
}

fn read_local(url: &Url) -> Result<Vec<u8>, String> {
    let path: PathBuf = url
        .to_file_path()
        .map_err(|_| format!("bad file URL {url}"))?;
    std::fs::read(Path::new(&path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Fetch every image, in parallel (a handful of connections, to be polite).
/// One file by URL: http(s), file:// or data:.
pub fn fetch(u: &str) -> Result<Vec<u8>, String> {
    if u.starts_with("data:") {
        return decode_data_uri(u);
    }
    let url = Url::parse(u).map_err(|e| format!("{u}: {e}"))?;
    match url.scheme() {
        "file" => read_local(&url),
        "http" | "https" => get(u).map(|(b, _)| b),
        s => Err(format!("{u}: unsupported scheme {s}")),
    }
}

pub fn fetch_images(urls: &[String]) -> Vec<Result<Vec<u8>, String>> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .expect("thread pool");
    pool.install(|| urls.par_iter().map(|u| fetch(u)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(arxiv_id("2501.14926v4").as_deref(), Some("2501.14926v4"));
        assert_eq!(arxiv_id("arXiv:2501.14926").as_deref(), Some("2501.14926"));
        assert_eq!(
            arxiv_id("hep-th/9901001").as_deref(),
            Some("hep-th/9901001")
        );
        assert_eq!(
            arxiv_id("10.48550/arXiv.2410.01383").as_deref(),
            Some("2410.01383")
        );
        assert!(arxiv_id("2501.149").is_none());
        for u in [
            "https://arxiv.org/abs/2501.14926v4",
            "https://arxiv.org/pdf/2501.14926v4.pdf",
            "https://arxiv.org/html/2501.14926v4",
            "https://arxiv.org/html/2501.14926v4/",
        ] {
            match parse_input(u).unwrap() {
                Input::Arxiv(id) => assert_eq!(id, "2501.14926v4"),
                _ => panic!("{u}"),
            }
        }
    }

    #[test]
    fn data_uri() {
        assert_eq!(
            decode_data_uri("data:text/plain;base64,aGk=").unwrap(),
            b"hi"
        );
        assert_eq!(decode_data_uri("data:text/plain,a%20b").unwrap(), b"a b");
    }
}
