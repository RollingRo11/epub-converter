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

/// Scroll once through the page (so content that appears on scroll gets
/// created), wait for fonts, and mark the figures to photograph. Returns how
/// many were marked. Images are waited for per figure, just before its photo.
const PREPARE: &str = r#"(async () => {
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  for (let y = 0; y < document.documentElement.scrollHeight; y += innerHeight) {
    scrollTo({top: y, behavior: 'instant'});
    await sleep(25);
  }
  scrollTo({top: 0, behavior: 'instant'});
  if (document.fonts) await document.fonts.ready;
  await sleep(300);
  let n = 0;
  for (const f of document.querySelectorAll('figure')) {
    if (f.parentElement && f.parentElement.closest('figure')) continue;
    const r = f.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) continue;
    const custom = [...f.querySelectorAll('*')].some(e => e.tagName.includes('-'));
    if (f.querySelector('canvas,svg,button,input,select,video,iframe') || custom || f.querySelectorAll('img').length > 4) {
      f.dataset.epubshot = String(n++);
    }
  }
  // Canvas widgets outside figures (graph viewers, plots): photograph the
  // widget's container, unless that would also cover up the essay's prose.
  const prose = el => [...el.querySelectorAll('p')].reduce((s, p) => s + p.innerText.length, 0);
  const WIDGET = '[class*="mount"],[class*="widget"],[class*="graph"],[class*="viz"],[class*="chart"],[class*="plot"],[class*="interactive"]';
  for (const c of document.querySelectorAll('canvas')) {
    if (c.closest('[data-epubshot]')) continue;
    let t = c.closest(WIDGET) || c.parentElement;
    if (!t || prose(t) > 200) t = c;
    if (t.closest('[data-epubshot]') || t.querySelector('[data-epubshot]')) continue;
    const r = t.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) continue;
    t.dataset.epubshot = String(n++);
  }
  return String(n);
})()"#;

/// Bring figure N into view with its captions hidden; returns its box.
fn shot_js(n: usize) -> String {
    format!(
        r#"(async () => {{
  const f = document.querySelector('[data-epubshot="{n}"]');
  // 'instant' overrides CSS smooth scrolling, which would move the page
  // during the photo.
  f.scrollIntoView({{block: 'center', behavior: 'instant'}});
  for (const c of f.querySelectorAll('figcaption')) {{
    if (c.closest('figure') !== f) continue;
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
  return JSON.stringify([r.left + scrollX, r.top + scrollY, r.width, r.height, r.top >= 0 && r.bottom <= innerHeight]);
}})()"#
    )
}

/// Swap each photographed figure's contents for its picture plus its captions,
/// and return the page as rendered.
const FINISH: &str = r#"(() => {
  for (const f of document.querySelectorAll('[data-epubshot]')) {
    const caps = [...f.querySelectorAll('figcaption')].filter(c => c.closest('figure') === f);
    caps.forEach(c => { c.style.display = c.dataset.epubDisplay || ''; });
    const img = document.createElement('img');
    img.src = 'epubshot:' + f.dataset.epubshot;
    img.width = Math.round(Number(f.dataset.epubW) || 0);
    img.alt = f.getAttribute('aria-label') || 'Figure';
    if (f.tagName === 'CANVAS') f.replaceWith(img);
    else f.replaceChildren(img, ...caps);
  }
  return '<!DOCTYPE html>' + document.documentElement.outerHTML;
})()"#;

pub fn render(url: &str, exe: &Path, log: &dyn Fn(String)) -> Result<Rendered> {
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

    let count: usize = eval_string(&tab, PREPARE)?
        .parse()
        .context("figure count")?;
    log(format!("photographing {count} interactive figures"));

    let mut shots = Vec::with_capacity(count);
    for n in 0..count {
        let (x, y, w, h, on_screen): (f64, f64, f64, f64, bool) =
            serde_json::from_str(&eval_string(&tab, &shot_js(n))?)?;
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

    let html = eval_string(&tab, FINISH)?;
    Ok(Rendered {
        html,
        shots,
        browser: exe.to_path_buf(),
    })
}
