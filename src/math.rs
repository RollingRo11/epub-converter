//! TeX / MathML -> SVG with MathJax, run inside embedded QuickJS runtimes.
//!
//! Each worker thread owns one QuickJS runtime with the MathJax bundle loaded
//! (`assets/mathjax.js`, built from `mathjax/entry.js`). Jobs are pulled from a
//! shared index, so all equations of a paper are typeset in parallel.
//!
//! Order of attempts per equation:
//!   1. the original TeX (LaTeXML keeps it in `alttext`), as md2kindle does;
//!   2. LaTeXML's presentation MathML, which has the author's own macros already
//!      expanded, for TeX that MathJax cannot parse (custom macros etc.).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, anyhow};
use rquickjs::{CatchResultExt, Context, Function, Runtime};

static BUNDLE: &str = include_str!("../assets/mathjax.js");

#[derive(Debug, Clone)]
pub struct MathJob {
    pub tex: String,
    pub mathml: Option<String>,
    pub display: bool,
}

/// One typeset equation. Geometry is in MathJax's internal units
/// (1000 per em of the math font), taken from the SVG's viewBox.
#[derive(Debug, Clone)]
pub struct MathSvg {
    pub svg: String,
    pub min_x: f64,
    pub width: f64,
    /// Height above the baseline.
    pub ascent: f64,
    /// Depth below the baseline.
    pub descent: f64,
    /// True when the TeX failed and the MathML fallback was used.
    pub from_mathml: bool,
    /// Why the TeX failed, when the MathML fallback was used.
    pub tex_error: Option<String>,
}

/// MathJax's ex is the TeX font's x-height: 442 units.
pub const UNITS_PER_EX: f64 = 442.0;

struct Engine {
    // Field order matters: the context must drop before the runtime.
    ctx: Context,
    _rt: Runtime,
}

impl Engine {
    fn new() -> Result<Self> {
        let rt = Runtime::new()?;
        rt.set_max_stack_size(64 * 1024 * 1024);
        let ctx = Context::full(&rt)?;
        ctx.with(|ctx| -> Result<()> {
            ctx.eval::<(), _>(BUNDLE)
                .catch(&ctx)
                .map_err(|e| anyhow!("loading MathJax: {e}"))?;
            Ok(())
        })?;
        Ok(Self { ctx, _rt: rt })
    }

    fn call(&self, func: &str, source: &str, display: bool) -> Result<String, String> {
        self.ctx.with(|ctx| {
            let f: Function = ctx.globals().get(func).map_err(|e| e.to_string())?;
            f.call::<_, String>((source, display))
                .catch(&ctx)
                // The message is the first line; the rest is a JS stack trace.
                .map_err(|e| {
                    e.to_string()
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
        })
    }

    fn render(&self, job: &MathJob, preamble: &str) -> Result<MathSvg, String> {
        let tex = if preamble.is_empty() {
            job.tex.clone()
        } else {
            format!("{preamble}{}", job.tex)
        };
        let tex_err = match self.call("__renderTex", &tex, job.display) {
            Ok(svg) => match parse_svg(svg, false) {
                Ok(m) => return Ok(m),
                Err(e) => e,
            },
            Err(e) => e,
        };
        if let Some(mml) = &job.mathml
            && let Ok(svg) = self.call("__renderMathml", mml, job.display)
            && let Ok(mut m) = parse_svg(svg, true)
        {
            m.tex_error = Some(tex_err);
            return Ok(m);
        }
        Err(tex_err)
    }
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn parse_svg(svg: String, from_mathml: bool) -> Result<MathSvg, String> {
    let open_end = svg.find('>').ok_or("no <svg> tag")?;
    let open = &svg[..open_end];
    // A width that isn't in ex (e.g. "100%" from \tag) can't be sized as an image.
    if !attr(open, "width").is_some_and(|w| w.ends_with("ex")) {
        return Err("equation has no intrinsic width".into());
    }
    let vb: Vec<f64> = attr(open, "viewBox")
        .ok_or("no viewBox")?
        .split_whitespace()
        .map(|v| v.parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("bad viewBox: {e}"))?;
    if vb.len() != 4 || !vb.iter().all(|v| v.is_finite()) {
        return Err("bad viewBox".into());
    }
    let (min_x, min_y, width, height) = (vb[0], vb[1], vb[2], vb[3]);
    Ok(MathSvg {
        min_x,
        width: width.max(1.0),
        ascent: -min_y,
        descent: height + min_y,
        svg,
        from_mathml,
        tex_error: None,
    })
}

/// Typeset every job, in parallel. The result vector is index-aligned with `jobs`.
/// `preamble` (a page's own `\newcommand`s, say) is put in front of every
/// job's TeX: each worker runs its own MathJax, so definitions can't be shared.
pub fn render_all(jobs: &[MathJob], preamble: &str) -> Result<Vec<Result<MathSvg, String>>> {
    if jobs.is_empty() {
        return Ok(Vec::new());
    }
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
    // Loading MathJax costs a little per runtime, so don't spin up idle workers.
    let workers = cpus.min(jobs.len().div_ceil(40)).max(1);

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<MathSvg, String>>>> = Mutex::new(vec![None; jobs.len()]);
    let init_error: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    std::thread::scope(|scope| {
        for i in 0..workers {
            let (next, results, init_error) = (&next, &results, &init_error);
            std::thread::Builder::new()
                .name(format!("mathjax-{i}"))
                .stack_size(128 * 1024 * 1024)
                .spawn_scoped(scope, move || {
                    let engine = match Engine::new() {
                        Ok(e) => e,
                        Err(e) => {
                            init_error.lock().unwrap().get_or_insert(e);
                            return;
                        }
                    };
                    loop {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(idx) else { break };
                        let r = engine.render(job, preamble);
                        results.lock().unwrap()[idx] = Some(r);
                    }
                })
                .expect("spawning MathJax worker");
        }
    });

    let results = results.into_inner().unwrap();
    if results.iter().any(Option::is_none) {
        let err = init_error.into_inner().unwrap();
        return Err(err.unwrap_or_else(|| anyhow!("MathJax workers stopped early")));
    }
    Ok(results.into_iter().map(Option::unwrap).collect())
}
