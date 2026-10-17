//! arXiv papers and web pages to EPUB. The `epub-converter` binary is a thin CLI over
//! these modules; `examples/device_test.rs` uses them to build reader tests.

pub mod browser;
pub mod convert;
pub mod cover;
pub mod epub;
pub mod layout;
pub mod math;
pub mod raster;
pub mod source;
#[cfg(test)]
mod tests;
pub mod tree;
