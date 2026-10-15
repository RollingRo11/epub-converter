// MathJax entry point bundled into assets/mathjax.js and evaluated inside the
// QuickJS runtime embedded in the Rust binary.
//
// It mirrors md2kindle's setup (mathjax-full 3.2.2, TeX input with the full
// package set, SVG output, lite adaptor), with two changes so that failures are
// reported instead of drawn:
//   * `noerrors` / `noundefined` are left out, and formatError throws, so a bad
//     or unknown macro is an exception the caller can fall back from.
//   * fontCache is 'none', so every SVG is self-contained (no shared <defs>),
//     which lets the Rust side compose several SVGs into one image.
//
// Exposed globals: __renderTex(tex, display) and __renderMathml(mml, display),
// each returning the SVG string or throwing.

import { mathjax } from 'mathjax-full/js/mathjax.js';
import { TeX } from 'mathjax-full/js/input/tex.js';
import { MathML } from 'mathjax-full/js/input/mathml.js';
import { SVG } from 'mathjax-full/js/output/svg.js';
import { liteAdaptor } from 'mathjax-full/js/adaptors/liteAdaptor.js';
import { RegisterHTMLHandler } from 'mathjax-full/js/handlers/html.js';
import { AllPackages } from 'mathjax-full/js/input/tex/AllPackages.js';

const adaptor = liteAdaptor();
RegisterHTMLHandler(adaptor);

const SKIP = new Set(['noerrors', 'noundefined']);
const packages = AllPackages.filter((p) => !SKIP.has(p));

// Common LaTeX macros that MathJax lacks. Anything else unknown throws and the
// caller falls back to LaTeXML's MathML, which has every macro expanded.
const macros = {
  bm: ['\\boldsymbol{#1}', 1],
  mathbbm: ['\\mathbb{#1}', 1],
  bold: ['\\mathbf{#1}', 1],
  textsc: ['\\text{#1}', 1],
};

const tex = new TeX({
  packages,
  macros,
  formatError(_jax, err) {
    throw err;
  },
});
const mml = new MathML({
  parseError(node) {
    throw new Error('MathML parse error: ' + node);
  },
});

const texDoc = mathjax.document('', { InputJax: tex, OutputJax: new SVG({ fontCache: 'none' }) });
const mmlDoc = mathjax.document('', { InputJax: mml, OutputJax: new SVG({ fontCache: 'none' }) });

function render(doc, source, display) {
  let node;
  try {
    node = doc.convert(source, { display: !!display, em: 16, ex: 8, containerWidth: 80 * 16 });
  } catch (err) {
    // MathJax throws its own error objects; hand Rust a plain message.
    throw new Error(String((err && err.message) || err));
  }
  const svg = adaptor.firstChild(node);
  const out = adaptor.outerHTML(svg);
  if (out.indexOf('data-mml-node="merror"') !== -1) {
    throw new Error('MathJax produced an error node');
  }
  return out;
}

globalThis.__renderTex = (source, display) => render(texDoc, source, display);
globalThis.__renderMathml = (source, display) => render(mmlDoc, source, display);
