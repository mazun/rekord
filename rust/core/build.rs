//! Puts the ONNX models into core/models/ at build time instead of committing
//! them to the repo:
//!
//! - cqt156.onnx, okarina_s.onnx, okarina_m.onnx, okarina_l.onnx (the S/M/L
//!   note models with three vocal/bass/other parts): Okarina, an unofficial
//!   derivative of Harmonica (instrument-agnostic note transcription),
//!   trained and exported to ONNX in the separate harmonica-repro project
//!   (its internal file names are mix_{small,medium,large}_v4.onnx).
//!   Downloaded from the Hugging Face
//!   model repo mazun/rekord at a pinned revision and checked against
//!   pinned SHA-256s (`fetch_okarina_models`). That repo is private for
//!   now, so a read-access token is needed (HF_TOKEN, or huggingface's own
//!   token file). The weights are for non-commercial use only, see the
//!   model card at https://huggingface.co/mazun/rekord.
//! - adtof_frame_rnn.onnx: ADTOF Frame_RNN (Zehren et al., CC BY-NC-SA 4.0) for
//!   drum transcription. The authors only publish a TensorFlow checkpoint; this
//!   is a community conversion (thebluescreen/adtof-drums-onnx on Hugging
//!   Face, public, with the madmom spectrogram built into the graph), fetched
//!   from there at a pinned commit and SHA-256 (`fetch_drum_model`) rather than
//!   re-hosted. No token is sent to that repo.
//! - beatthis_small0.onnx + logmel.onnx: Beat This! (MIT, CPJKU) is only
//!   published as a PyTorch checkpoint, so export_beat_models.py downloads and
//!   converts it (and generates the log-mel frontend). This needs Python >= 3.10
//!   on the build machine, only when a file is missing: a throwaway venv under
//!   models/ gets the pinned CPU-only packages below (~1 GB download on the
//!   first build) and is deleted afterwards. Set REKORD_PYTHON to pick the
//!   interpreter.
//!
//! The pinned repos, commits and hashes are in src/model_manifest.rs, which
//! the app's release builds also use to download the models on first launch.
//! Those builds don't read core/models/, so the release CI sets
//! REKORD_SKIP_MODELS=1 to skip all of this (and the token and Python it needs).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};
use ureq::config::RedirectAuthHeaders;

include!("src/model_manifest.rs");

// ureq's read_to_vec() defaults to a 10MB cap; cqt156.onnx alone is ~17MB.
const MAX_MODEL_BYTES: u64 = 64 * 1024 * 1024;

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
    println!("cargo:rerun-if-env-changed=REKORD_SKIP_MODELS");
    if std::env::var_os("REKORD_SKIP_MODELS").is_some() {
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let models_dir = manifest_dir.join("models");
    std::fs::create_dir_all(&models_dir).expect("creating models/ directory");

    fetch_okarina_models(&models_dir);
    fetch_drum_model(&models_dir);
    export_beat_models(&manifest_dir, &models_dir);
}

fn fetch_okarina_models(models_dir: &Path) {
    println!("cargo:rerun-if-env-changed=HF_TOKEN");
    println!("cargo:rerun-if-env-changed=HF_HOME");
    let token = hf_token();

    for file in &OKARINA_FILES {
        fetch_model(models_dir, file, token.as_deref());
    }
}

fn fetch_drum_model(models_dir: &Path) {
    fetch_model(models_dir, &DRUM_FILE, None);
}

/// Downloads `file` unless models/ already has it. `token` is only for the private repo.
fn fetch_model(models_dir: &Path, file: &ModelFile, token: Option<&str>) {
    let path = models_dir.join(file.name);
    println!("cargo:rerun-if-changed={}", path.display());

    if let Ok(bytes) = std::fs::read(&path) {
        if sha256_hex(&bytes) == file.sha256 {
            return;
        }
        println!(
            "cargo:warning={} does not match the pinned model; re-downloading",
            file.name
        );
    }

    download_model(models_dir, file, token);
}

fn download_model(models_dir: &Path, file: &ModelFile, token: Option<&str>) {
    let (name, sha256) = (file.name, file.sha256);
    let url = file.url();
    eprintln!("Downloading {name} from {url} ...");

    let mut req = ureq::get(&url);
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    // HF's `resolve` redirects big files to a signed CDN URL on another host,
    // which rejects a second auth mechanism: never forward Authorization on
    // redirect (this is ureq's own default; pinned explicitly since it matters
    // here).
    let req = req
        .config()
        .redirect_auth_headers(RedirectAuthHeaders::Never)
        .build();

    let bytes = req
        .call()
        .and_then(|mut resp| {
            resp.body_mut()
                .with_config()
                .limit(MAX_MODEL_BYTES)
                .read_to_vec()
        })
        .unwrap_or_else(|e| {
            panic!(
                "downloading {name} from {url}: {e}\n\
                 {HF_REPO} is private for now: set HF_TOKEN to a read-access token for it \
                 (or have huggingface's token file), or copy the file from \
                 harmonica-repro's runs/p5/onnx/ into models/ (it's then checked against \
                 the pinned SHA-256). {DRUM_REPO} is public and needs no token."
            )
        });

    assert_eq!(
        bytes.len() as u64,
        file.size,
        "downloaded {name} does not have its pinned size; not writing it"
    );
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
    std::fs::rename(&tmp_path, &path)
        .unwrap_or_else(|e| panic!("finalizing {}: {e}", path.display()));

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
            PathBuf::from(std::env::var(home_var).ok()?)
                .join(".cache")
                .join("huggingface")
                .join("token")
        }
    };
    let t = std::fs::read_to_string(token_path).ok()?;
    let t = t.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn export_beat_models(manifest_dir: &Path, models_dir: &Path) {
    let script = manifest_dir.join("export_beat_models.py");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-env-changed=REKORD_PYTHON");
    let beat_models = BEAT_FILES.map(|f| f.name);
    for name in beat_models {
        println!("cargo:rerun-if-changed={}", models_dir.join(name).display());
    }
    if beat_models.iter().all(|n| models_dir.join(n).exists()) {
        return;
    }

    let python = find_python().unwrap_or_else(|| {
        panic!(
            "Building the beat-tracking models needs Python >= 3.10 (not found as `py -3`, \
             `python3` or `python`; set REKORD_PYTHON to its path). Alternatively copy \
             {:?} into {} by hand.",
            beat_models,
            models_dir.display()
        )
    });

    let venv = models_dir.join(".export-venv");
    let _ = std::fs::remove_dir_all(&venv);
    run(
        Command::new(&python[0])
            .args(&python[1..])
            .arg("-m")
            .arg("venv")
            .arg(&venv),
        "creating the export venv",
    );
    let py = if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    };

    eprintln!(
        "Installing PyTorch (CPU) and beat_this into {} ...",
        venv.display()
    );
    let pip = |args: &[&str], what: &str| {
        run(
            Command::new(&py)
                .args(["-m", "pip", "install", "--disable-pip-version-check", "-q"])
                .args(args),
            what,
        )
    };
    // an older bundled pip (e.g. Python 3.11.4's) mis-resolves the PyTorch index's
    // dependency mirrors (typing_extensions name normalization) and fails
    pip(&["--upgrade", "pip"], "upgrading pip");
    pip(
        &[&["--index-url", TORCH_INDEX][..], &TORCH_REQS[..]].concat(),
        "installing torch",
    );
    pip(&OTHER_REQS, "installing beat_this / onnx");

    eprintln!("Exporting Beat This! small0 and the log-mel frontend ...");
    run(
        Command::new(&py).arg(&script).arg(models_dir),
        "running export_beat_models.py",
    );

    let _ = std::fs::remove_dir_all(&venv);
    for name in beat_models {
        assert!(
            models_dir.join(name).exists(),
            "export_beat_models.py did not write {name}"
        );
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
        Command::new(&c[0])
            .args(&c[1..])
            .args(["-c", check])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd.status().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(status.success(), "{what} failed ({status})");
}
