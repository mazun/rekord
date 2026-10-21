//! Where the ONNX models are read from. By default that is core/models/ of this checkout,
//! where build.rs puts them; the app's release builds download them at run time into their
//! own data directory instead and point here with `set_dir` before loading any model.

use std::path::PathBuf;
use std::sync::OnceLock;

include!("model_manifest.rs");

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Every model file, for a downloader to fetch.
pub fn all() -> impl Iterator<Item = &'static ModelFile> {
    OKARINA_FILES
        .iter()
        .chain(std::iter::once(&DRUM_FILE))
        .chain(BEAT_FILES.iter())
}

/// Reads the models from `dir` from now on. Only the first call takes effect.
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

pub fn path(name: &str) -> PathBuf {
    match DIR.get() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/models")).join(name),
    }
}
