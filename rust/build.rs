//! Fetches the basic-pitch ONNX model at build time instead of committing
//! it to the repo (it's Spotify's, not ours -- Apache 2.0 licensed, from
//! https://github.com/spotify/basic-pitch/tree/main/basic_pitch/saved_models/icassp_2022).

use std::io::Write;
use std::path::PathBuf;

const MODEL_URL: &str =
    "https://raw.githubusercontent.com/spotify/basic-pitch/main/basic_pitch/saved_models/icassp_2022/nmp.onnx";

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let models_dir = manifest_dir.join("models");
    let model_path = models_dir.join("nmp.onnx");

    println!("cargo:rerun-if-changed={}", model_path.display());

    if model_path.exists() {
        return;
    }

    std::fs::create_dir_all(&models_dir).expect("creating models/ directory");

    eprintln!("Downloading basic-pitch ONNX model from {MODEL_URL} ...");
    let bytes = ureq::get(MODEL_URL)
        .call()
        .expect("downloading nmp.onnx")
        .body_mut()
        .read_to_vec()
        .expect("reading downloaded model bytes");

    let tmp_path = models_dir.join("nmp.onnx.part");
    std::fs::File::create(&tmp_path)
        .and_then(|mut f| f.write_all(&bytes))
        .expect("writing downloaded model");
    std::fs::rename(&tmp_path, &model_path).expect("finalizing downloaded model file");

    eprintln!("Saved {} ({} bytes)", model_path.display(), bytes.len());
}
