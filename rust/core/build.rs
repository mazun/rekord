//! Puts the ONNX models into core/models/ at build time instead of committing
//! them to the repo:
//!
//! - cqt156.onnx, small_uni_300k.onnx, medium_uni_300k_urmp.onnx,
//!   large_uni_300k_urmp.onnx, mix_large_v1.onnx (the 3-part vocal/bass/other
//!   model): Harmonica (instrument-agnostic note
//!   transcription), reimplemented, trained and exported to ONNX in the
//!   separate harmonica-repro project. Downloaded from the Hugging Face
//!   model repo mazun/rekord at a pinned revision and checked against
//!   pinned SHA-256s (`fetch_harmonica_models`). That repo is private for
//!   now, so a read-access token is needed (HF_TOKEN, or huggingface's own
//!   token file). The weights are for non-commercial use only, see the
//!   model card at https://huggingface.co/mazun/rekord.
//! - beatthis_small0.onnx + logmel.onnx: Beat This! (MIT, CPJKU) is only
//!   published as a PyTorch checkpoint, so export_beat_models.py downloads and
//!   converts it (and generates the log-mel frontend). This needs Python >= 3.10
//!   on the build machine, only when a file is missing: a throwaway venv under
//!   models/ gets the pinned CPU-only packages below (~1 GB download on the
//!   first build) and is deleted afterwards. Set REKORD_PYTHON to pick the
//!   interpreter.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};
use ureq::config::RedirectAuthHeaders;

const HF_REPO: &str = "mazun/rekord";
/// A commit hash, not a branch, so an old checkout keeps building the weights
/// it was tested against. New weights: upload them first, then update this
/// and the hashes below.
const HF_REVISION: &str = "6ebdd5568f689173a4ed470994ab34628c8ba7ac";

/// (file name, lowercase hex SHA-256).
const HARMONICA_MODELS: [(&str, &str); 5] = [
    ("cqt156.onnx", "ff88505975e9d6c3e3c413aff5748abe711dfe3e9aee1bdf57bb637e28cf1c6f"),
    ("small_uni_300k.onnx", "780ace3e0e619665b7975f387f7b06016b104b7133eb5734af1704e6377146dc"),
    ("medium_uni_300k_urmp.onnx", "d408a313c0fdfd6ece7189b7a32b726899efcd0051baf599671b57736bbe1eeb"),
    ("large_uni_300k_urmp.onnx", "c455139320d69062269f8e0ab723938065e2d56d3539cad6f8f362f35dcca888"),
    ("mix_large_v1.onnx", "bdd74c8aa2a4e8035b0c8403a3467a73caea8beb5c9f57947502f56bcf0109af"),
];

// ureq's read_to_vec() defaults to a 10MB cap; cqt156.onnx alone is ~17MB.
const MAX_MODEL_BYTES: u64 = 64 * 1024 * 1024;

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

    fetch_harmonica_models(&models_dir);
    export_beat_models(&manifest_dir, &models_dir);
}

fn fetch_harmonica_models(models_dir: &Path) {
    println!("cargo:rerun-if-env-changed=HF_TOKEN");
    println!("cargo:rerun-if-env-changed=HF_HOME");
    let token = hf_token();

    for (name, sha256) in HARMONICA_MODELS {
        let path = models_dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());

        if let Ok(bytes) = std::fs::read(&path) {
            if sha256_hex(&bytes) == sha256 {
                continue;
            }
            println!("cargo:warning={name} does not match the pinned model; re-downloading");
        }

        download_model(models_dir, name, sha256, token.as_deref());
    }
}

fn download_model(models_dir: &Path, name: &str, sha256: &str, token: Option<&str>) {
    let url = format!("https://huggingface.co/{HF_REPO}/resolve/{HF_REVISION}/{name}");
    eprintln!("Downloading {name} from {url} ...");

    let mut req = ureq::get(&url);
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    // HF's `resolve` redirects big files to a signed CDN URL on another host,
    // which rejects a second auth mechanism: never forward Authorization on
    // redirect (this is ureq's own default; pinned explicitly since it matters
    // here).
    let req = req.config().redirect_auth_headers(RedirectAuthHeaders::Never).build();

    let bytes = req
        .call()
        .and_then(|mut resp| resp.body_mut().with_config().limit(MAX_MODEL_BYTES).read_to_vec())
        .unwrap_or_else(|e| {
            panic!(
                "downloading {name} from {url}: {e}\n\
                 The repo is private for now: set HF_TOKEN to a read-access token for \
                 {HF_REPO} (or have huggingface's token file), or copy the file from \
                 harmonica-repro's runs/p5/onnx/ into models/ (it's then checked against \
                 the pinned SHA-256)."
            )
        });

    let actual = sha256_hex(&bytes);
    assert_eq!(
        actual, sha256,
        "downloaded {name} does not match its pinned sha256 (expected {sha256}, got {actual}); not writing it"
    );

    let path = models_dir.join(name);
    let tmp_path = models_dir.join(format!("{name}.part"));
    std::fs::File::create(&tmp_path)
        .and_then(|mut f| f.write_all(&bytes))
        .unwrap_or_else(|e| panic!("writing {}: {e}", tmp_path.display()));
    std::fs::rename(&tmp_path, &path).unwrap_or_else(|e| panic!("finalizing {}: {e}", path.display()));

    eprintln!("Saved {} ({} bytes)", path.display(), bytes.len());
}

/// Looked up the way huggingface_hub does: HF_TOKEN env var, else
/// $HF_HOME/token, else ~/.cache/huggingface/token. Never logged.
fn hf_token() -> Option<String> {
    if let Ok(t) = std::env::var("HF_TOKEN") {
        let t = t.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }

    let token_path = match std::env::var("HF_HOME") {
        Ok(hf_home) => PathBuf::from(hf_home).join("token"),
        Err(_) => {
            let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            PathBuf::from(std::env::var(home_var).ok()?).join(".cache").join("huggingface").join("token")
        }
    };
    let t = std::fs::read_to_string(token_path).ok()?;
    let t = t.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
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
