// The portable (zip) release: a `portable.txt` beside the exe makes the app keep everything it
// writes -- the downloaded models and the webview's data (localStorage settings included) -- in a
// `data` folder beside the exe instead of under %LOCALAPPDATA%. Such a copy can't run the NSIS
// updater (it would install a second, installed copy), so its update notice links to the release
// page instead.

use std::path::PathBuf;
use std::sync::OnceLock;

const MARKER: &str = "portable.txt";

// The folder the portable copy keeps its data in, or `None` for an installed copy.
pub fn data_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        exe_dir.join(MARKER).is_file().then(|| exe_dir.join("data"))
    })
    .as_ref()
}
