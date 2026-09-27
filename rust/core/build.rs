//! Puts the ONNX models into core/models/ at build time instead of committing
//! them to the repo:
//!
//! - cqt156.onnx, small_uni_300k.onnx, medium_uni_300k_urmp.onnx,
//!   large_uni_300k_urmp.onnx: Harmonica (instrument-agnostic note
//!   transcription), reimplemented, trained and exported to ONNX in the
//!   separate harmonica-repro project (`runs/p5/onnx/`). They aren't
//!   published anywhere to download from, so they're copied into models/ by
//!   hand; `check_harmonica_models` only checks they're there. The weights
//!   are for non-commercial use only (models/harmonica.LICENSE.txt).
//! - beatthis_small0.onnx + logmel.onnx: Beat This! (MIT, CPJKU) is only
//!   published as a PyTorch checkpoint, so export_beat_models.py downloads and
//!   converts it (and generates the log-mel frontend). This needs Python >= 3.10
//!   on the build machine, only when a file is missing: a throwaway venv under
//!   models/ gets the pinned CPU-only packages below (~1 GB download on the
//!   first build) and is deleted afterwards. Set REKORD_PYTHON to pick the
//!   interpreter.

use std::path::{Path, PathBuf};
use std::process::Command;

const HARMONICA_MODELS: [&str; 4] =
    ["cqt156.onnx", "small_uni_300k.onnx", "medium_uni_300k_urmp.onnx", "large_uni_300k_urmp.onnx"];

const BEAT_MODELS: [&str; 2] = ["beatthis_small0.onnx", "logmel.onnx"];
const TORCH_INDEX: &str = "https://download.pytorch.org/whl/cpu";
const TORCH_REQS: [&str; 2] = ["torch==2.14.0", "torchaudio==2.11.0"];
const OTHER_REQS: [&str; 6] = [
    "beat-this==1.1.0",
    "einops==0.8.2",
    "rotary-embedding-torch==0.9.1",
    "soxr==1.1.0",
    "onnx==1.23.0",
    "onnxscript==0.7.2",
];

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let models_dir = manifest_dir.join("models");
    std::fs::create_dir_all(&models_dir).expect("creating models/ directory");

    check_harmonica_models(&models_dir);
    export_beat_models(&manifest_dir, &models_dir);
}

fn check_harmonica_models(models_dir: &Path) {
    for name in HARMONICA_MODELS {
        let path = models_dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        if !path.exists() {
            panic!(
                "missing {}: copy the Harmonica ONNX files {HARMONICA_MODELS:?} from the \
                 separate harmonica-repro project's runs/p5/onnx/ into {} (they aren't \
                 downloaded; the weights are for non-commercial use only).",
                path.display(),
                models_dir.display()
            );
        }
    }
}

fn export_beat_models(manifest_dir: &Path, models_dir: &Path) {
    let script = manifest_dir.join("export_beat_models.py");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-env-changed=REKORD_PYTHON");
    for name in BEAT_MODELS {
        println!("cargo:rerun-if-changed={}", models_dir.join(name).display());
    }
    if BEAT_MODELS.iter().all(|n| models_dir.join(n).exists()) {
        return;
    }

    let python = find_python().unwrap_or_else(|| {
        panic!(
            "Building the beat-tracking models needs Python >= 3.10 (not found as `py -3`, \
             `python3` or `python`; set REKORD_PYTHON to its path). Alternatively copy \
             {:?} into {} by hand.",
            BEAT_MODELS,
            models_dir.display()
        )
    });

    let venv = models_dir.join(".export-venv");
    let _ = std::fs::remove_dir_all(&venv);
    run(Command::new(&python[0]).args(&python[1..]).arg("-m").arg("venv").arg(&venv), "creating the export venv");
    let py = if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") };

    eprintln!("Installing PyTorch (CPU) and beat_this into {} ...", venv.display());
    let pip = |args: &[&str], what: &str| {
        run(Command::new(&py).args(["-m", "pip", "install", "--disable-pip-version-check", "-q"]).args(args), what)
    };
    // an older bundled pip (e.g. Python 3.11.4's) mis-resolves the PyTorch index's
    // dependency mirrors (typing_extensions name normalization) and fails
    pip(&["--upgrade", "pip"], "upgrading pip");
    pip(&[&["--index-url", TORCH_INDEX][..], &TORCH_REQS[..]].concat(), "installing torch");
    pip(&OTHER_REQS, "installing beat_this / onnx");

    eprintln!("Exporting Beat This! small0 and the log-mel frontend ...");
    run(Command::new(&py).arg(&script).arg(models_dir), "running export_beat_models.py");

    let _ = std::fs::remove_dir_all(&venv);
    for name in BEAT_MODELS {
        assert!(models_dir.join(name).exists(), "export_beat_models.py did not write {name}");
    }
}

fn find_python() -> Option<Vec<String>> {
    let mut candidates: Vec<Vec<String>> = Vec::new();
    if let Ok(p) = std::env::var("REKORD_PYTHON") {
        candidates.push(vec![p]);
    }
    candidates.push(vec!["py".into(), "-3".into()]);
    candidates.push(vec!["python3".into()]);
    candidates.push(vec!["python".into()]);
    let check = "import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)";
    candidates.into_iter().find(|c| {
        Command::new(&c[0]).args(&c[1..]).args(["-c", check]).status().map(|s| s.success()).unwrap_or(false)
    })
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd.status().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(status.success(), "{what} failed ({status})");
}
