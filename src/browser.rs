//! Browser mode, for interactive web pages.
//!
//! Some essays draw their figures with JavaScript (canvases, tabbed image
//! grids, sliders). Their HTML alone can't show them, so this opens the page in
//! a hidden Chromium-family browser (Chrome, Chromium, Brave, Edge), lets it
//! run, and takes a picture of each interactive figure without its caption.
//! The figure's contents are then replaced by `<img src="epubshot:N">` (served
//! from the screenshots) plus the original caption, and the rendered page is
//! converted like any other web page, so text stays text.
//!
//! The browser runs with a throwaway profile: it never sees the user's own
//! browsing data.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use headless_chrome::protocol::cdp::{Emulation, Page};
use headless_chrome::{Browser, LaunchOptions, Tab};

/// Viewport width in CSS px: the desktop layout these essays are designed for.
const VIEW_WIDTH: u32 = 1000;
/// Screenshots are taken at 2x for sharpness on 300 ppi screens.
const SCALE: f64 = 2.0;

pub struct Rendered {
    pub html: String,
    pub shots: Vec<Vec<u8>>,
    pub browser: PathBuf,
}

/// The first installed Chromium-family browser.
pub fn find_browser(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.exists().then(|| p.to_path_buf());
    }
    if let Ok(p) = headless_chrome::browser::default_executable() {
        return Some(p);
    }
    let candidates: &[&str] = &[
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
        "/usr/bin/brave-browser",
        "/usr/bin/brave",
        "/usr/bin/vivaldi",
        r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ];
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
        // macOS per-user installs live in ~/Applications.
        if let (Some(h), Some(rest)) = (&home, c.strip_prefix("/Applications/")) {
            let p = h.join("Applications").join(rest);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn eval_string(tab: &Tab, js: &str) -> Result<String> {
    let v = tab.evaluate(js, true).map_err(|e| anyhow!("{e}"))?;
    v.value
        .as_ref()
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("page script returned no text ({:?})", v.description))
}

/// What counts as a figure: real <figure>s, Distill's <d-figure>, and the
/// <div class="figure"> many hand-built sites use.
const FIG: &str = "figure, d-figure, .figure";

/// Scroll once through the page (so content that appears on scroll gets
/// created), wait for fonts, and mark what to photograph. Returns how many
/// were marked. Images are waited for per figure, just before its photo.
///
/// Photographed: figures with interactive or drawn content (canvas, SVG,
/// controls, script-filled placeholders, grids of images, token
/// highlighting), and such widgets outside figures. Real prose is never
/// photographed: in a figure with more than a few sentences, only its
/// interactive parts are. Blocks that are mostly equations stay typeset math.
fn prepare_js() -> String {
    format!(
        r#"(async () => {{
  const FIG = '{FIG}';
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  for (let y = 0; y < document.documentElement.scrollHeight; y += innerHeight) {{
    scrollTo({{top: y, behavior: 'instant'}});
    await sleep(25);
  }}
  scrollTo({{top: 0, behavior: 'instant'}});
  if (document.fonts) await document.fonts.ready;
  await sleep(300);

  // Real prose: a paragraph of sentences. A token viewer's "paragraph" has an
  // element per word (each token highlighted), so it doesn't count.
  const realProse = p => {{
    if (p.closest('figcaption')) return false;
    const words = p.innerText.split(/\s+/).filter(Boolean).length;
    if (words < 8) return false;
    const els = [...p.querySelectorAll('*')].filter(e => !e.closest('.katex, mjx-container')).length;
    return els / words < 0.5;
  }};
  const prose = el => [...el.querySelectorAll('p')].filter(realProse).reduce((s, p) => s + p.innerText.length, 0);
  // Drawn SVG figures, not math glyphs or icons (in buttons/links, or tiny).
  const drawnSvg = el => [...el.querySelectorAll('svg')].some(s => {{
    if (s.closest('.katex, mjx-container, button, a')) return false;
    const r = s.getBoundingClientRect();
    return r.width >= 24 && r.height >= 24;
  }});
  const custom = el => [...el.querySelectorAll('*')].some(e =>
    e.tagName.includes('-') && !e.tagName.startsWith('MJX') && !['D-MATH', 'D-CITE', 'D-FOOTNOTE'].includes(e.tagName));
  // Token-activation viewers: many small spans shaded by inline styles.
  const highlighted = el => el.querySelectorAll('[style*="background"]').length >= 20;
  const placeholder = el => [...el.querySelectorAll('[class*="mount"]')].some(m => !/\beq-/.test(m.className));
  const mathOnly = el => el.querySelector('.katex, mjx-container') && !el.querySelector('canvas,button,input,select') && !drawnSvg(el);
  const visible = el => {{ const r = el.getBoundingClientRect(); return r.width >= 2 && r.height >= 2; }};
  const free = el => !el.closest('[data-epubshot]') && !el.querySelector('[data-epubshot]');

  const interactive = el => el.querySelector('canvas,button,input,select,video,iframe') || drawnSvg(el) ||
    custom(el) || placeholder(el) || highlighted(el) || el.querySelectorAll('img').length > 4;
  const controlsOnly = el => el.querySelector('button,input,select') &&
    !el.querySelector('canvas,img,video,iframe,[class*="mount"]') && !drawnSvg(el) && !highlighted(el) &&
    [...el.querySelectorAll('*')].filter(e => !e.closest('button,select')).map(e => e.children.length ? '' : e.innerText || '').join('').replace(/[\s\d\/]/g, '').length < 30;
  let n = 0;
  // In a figure that also holds real prose (a walkthrough beside a graph),
  // photograph only the interactive parts, so the paragraphs stay text.
  // Headings, real paragraphs and plain lists stay text; every other part
  // (graph, token viewer, chart labels) is photographed.
  const markParts = el => {{
    for (const c of el.children) {{
      if (c.tagName === 'FIGCAPTION' || !visible(c)) continue;
      // A part holding the caption is split further, so the caption stays text.
      if (c.querySelector('figcaption')) {{ markParts(c); continue; }}
      // Navigation bars (prev/next buttons, "1 / 13" counters) mean nothing
      // on paper: drop them.
      if (controlsOnly(c)) {{ c.dataset.epubdrop = '1'; continue; }}
      const pr = prose(c);
      if (pr === 0) {{
        const textBlock = /^H[1-6]$/.test(c.tagName) || (/^(UL|OL|P)$/.test(c.tagName) && !interactive(c));
        if (!textBlock && !mathOnly(c)) c.dataset.epubshot = String(n++);
      }} else if (!(c.tagName === 'P' || (pr >= 0.7 * c.innerText.length && !interactive(c)))) {{
        markParts(c);
      }}
    }}
  }};
  for (const f of document.querySelectorAll(FIG)) {{
    if (f.parentElement && f.parentElement.closest(FIG)) continue;
    if (!visible(f) || !interactive(f) || mathOnly(f)) continue;
    // A few sentences of instructions are fine inside the photo.
    if (prose(f) <= 300) f.dataset.epubshot = String(n++);
    else markParts(f);
  }}
  // Widgets outside figures: canvases (photographed with their container
  // unless that holds prose) and script-filled placeholders.
  const WIDGET = '[class*="mount"],[class*="widget"],[class*="graph"],[class*="viz"],[class*="chart"],[class*="plot"],[class*="interactive"]';
  for (const c of document.querySelectorAll('canvas, [class*="mount"]')) {{
    if (!free(c)) continue;
    let t = c.tagName === 'CANVAS' ? (c.closest(WIDGET) || c.parentElement || c) : c;
    if (prose(t) > 200) t = c;
    if (!free(t) || !visible(t) || prose(t) > 200 || mathOnly(t)) continue;
    if (t !== c && /\beq-/.test(String(t.className))) continue;
    t.dataset.epubshot = String(n++);
  }}
  return String(n);
}})()"#
    )
}

/// The captions of a figure (not of figures nested inside it).
fn captions_js() -> String {
    format!("[...f.querySelectorAll('figcaption')].filter(c => c.closest('{FIG}') === f)")
}

/// Bring item N into view with its captions hidden; returns its box.
fn shot_js(n: usize) -> String {
    let caps = captions_js();
    format!(
        r#"(async () => {{
  const f = document.querySelector('[data-epubshot="{n}"]');
  // 'instant' overrides CSS smooth scrolling, which would move the page
  // during the photo.
  f.scrollIntoView({{block: 'center', behavior: 'instant'}});
  for (const c of {caps}) {{
    c.dataset.epubDisplay = c.style.display;
    c.style.display = 'none';
  }}
  const t0 = Date.now();
  while (Date.now() - t0 < 5000 && [...f.querySelectorAll('img')].some(i => !i.complete)) {{
    await new Promise(r => setTimeout(r, 50));
  }}
  await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
  await new Promise(r => setTimeout(r, 120));
  const r = f.getBoundingClientRect();
  f.dataset.epubW = String(r.width);
  const label = f.tagName.toLowerCase() + (f.id ? '#' + f.id : '') + (f.className ? '.' + String(f.className).trim().split(/\s+/).slice(0, 2).join('.') : '');
  return JSON.stringify([r.left + scrollX, r.top + scrollY, r.width, r.height, r.top >= 0 && r.bottom <= innerHeight, label]);
}})()"#
    )
}

/// Swap each photographed item for its picture (plus its captions, as a real
/// <figure>), and return the page as rendered.
fn finish_js() -> String {
    let caps = captions_js();
    format!(
        r#"(() => {{
  // Inside figures that stay as HTML, drop what the browser doesn't show:
  // "window too narrow" notices, inactive tabs, alternate layouts. (Not
  // captions, and not equations' hidden source, which the converter reads.)
  for (const f of document.querySelectorAll('{FIG}')) {{
    if (f.closest('[data-epubshot]')) continue;
    for (const e of [...f.querySelectorAll('*')]) {{
      if (!e.isConnected || e.closest('figcaption, math, .katex, mjx-container, script, style')) continue;
      if (e.getClientRects().length === 0 && !['BR', 'WBR', 'SOURCE'].includes(e.tagName)) e.remove();
    }}
  }}
  for (const d of document.querySelectorAll('[data-epubdrop]')) d.remove();
  for (const f of document.querySelectorAll('[data-epubshot]')) {{
    const caps = {caps};
    caps.forEach(c => {{ c.style.display = c.dataset.epubDisplay || ''; }});
    const img = document.createElement('img');
    img.src = 'epubshot:' + f.dataset.epubshot;
    img.width = Math.round(Number(f.dataset.epubW) || 0);
    img.alt = f.getAttribute('aria-label') || 'Figure';
    if (f.tagName === 'FIGURE') {{
      f.replaceChildren(img, ...caps);
    }} else {{
      const fig = document.createElement('figure');
      if (f.id) fig.id = f.id;
      fig.append(img, ...caps);
      f.replaceWith(fig);
    }}
  }}
  return '<!DOCTYPE html>' + document.documentElement.outerHTML;
}})()"#
    )
}

/// `log` gets progress lines; `detail` gets one line per photo (for --verbose).
pub fn render(
    url: &str,
    exe: &Path,
    log: &dyn Fn(String),
    detail: &dyn Fn(String),
) -> Result<Rendered> {
    let options = LaunchOptions::default_builder()
        .path(Some(exe.to_path_buf()))
        .headless(true)
        .window_size(Some((VIEW_WIDTH, 1400)))
        .idle_browser_timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| anyhow!("browser options: {e}"))?;
    let browser = Browser::new(options).map_err(|e| anyhow!("starting {}: {e}", exe.display()))?;
    let tab = browser.new_tab().map_err(|e| anyhow!("{e}"))?;
    tab.set_default_timeout(Duration::from_secs(90));

    tab.call_method(Emulation::SetDeviceMetricsOverride {
        width: VIEW_WIDTH,
        height: 1400,
        device_scale_factor: SCALE,
        mobile: false,
        scale: None,
        screen_width: None,
        screen_height: None,
        position_x: None,
        position_y: None,
        dont_set_visible_size: None,
        screen_orientation: None,
        viewport: None,
        display_feature: None,
        device_posture: None,
    })
    .map_err(|e| anyhow!("{e}"))?;
    // Light theme: dark-mode figures would print as dark blocks on e-ink.
    tab.call_method(Emulation::SetEmulatedMedia {
        media: None,
        features: Some(vec![Emulation::MediaFeature {
            name: "prefers-color-scheme".into(),
            value: "light".into(),
        }]),
    })
    .map_err(|e| anyhow!("{e}"))?;

    tab.navigate_to(url)
        .and_then(|t| t.wait_until_navigated())
        .map_err(|e| anyhow!("loading {url}: {e}"))?;

    let count: usize = eval_string(&tab, &prepare_js())?
        .parse()
        .context("figure count")?;
    log(format!(
        "photographing {count} interactive figures and widgets"
    ));

    let mut shots = Vec::with_capacity(count);
    for n in 0..count {
        let (x, y, w, h, on_screen, label): (f64, f64, f64, f64, bool, String) =
            serde_json::from_str(&eval_string(&tab, &shot_js(n))?)?;
        detail(format!("photo {n}: {label} ({w:.0}x{h:.0})"));
        let data = tab
            .call_method(Page::CaptureScreenshot {
                format: Some(Page::CaptureScreenshotFormatOption::Png),
                quality: None,
                clip: Some(Page::Viewport {
                    x,
                    y,
                    width: w.max(1.0),
                    height: h.max(1.0),
                    scale: 1.0,
                }),
                from_surface: Some(true),
                // Capturing beyond the window is much slower: only for figures
                // taller than the window.
                capture_beyond_viewport: Some(!on_screen),
                optimize_for_speed: None,
            })
            .map_err(|e| anyhow!("screenshot of figure {n}: {e}"))?
            .data;
        use base64::Engine as _;
        shots.push(base64::engine::general_purpose::STANDARD.decode(data)?);
    }

    let html = eval_string(&tab, &finish_js())?;
    Ok(Rendered {
        html,
        shots,
        browser: exe.to_path_buf(),
    })
}
