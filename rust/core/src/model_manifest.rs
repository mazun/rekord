// The pinned ONNX model files. Shared, via `include!`, by build.rs (which puts them into
// core/models/ for a checkout) and `model_files` (from which the app's release builds fetch
// them into their own data directory on first launch).

/// The Hugging Face model repo with Re:Kord's own copies of the models.
pub const HF_REPO: &str = "mazun/rekord";
/// A commit hash, not a branch, so an old checkout keeps building the weights
/// it was tested against. New weights: upload them first, then update this
/// and the hashes below.
pub const HF_REVISION: &str = "79ba1fe33427af235e2f5932a95224a8d77249ba";

/// The community ONNX conversion of ADTOF Frame_RNN (public repo, pinned like `HF_REVISION`).
pub const DRUM_REPO: &str = "thebluescreen/adtof-drums-onnx";
pub const DRUM_REVISION: &str = "d0e809bb1c1b6fe8a4a00c576852117a0636ab5a";

/// A model file on the Hugging Face Hub at a pinned commit.
pub struct ModelFile {
    pub name: &'static str,
    pub repo: &'static str,
    pub revision: &'static str,
    /// Lowercase hex SHA-256.
    pub sha256: &'static str,
    pub size: u64,
}

impl ModelFile {
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, self.name
        )
    }
}

const fn hf(name: &'static str, sha256: &'static str, size: u64) -> ModelFile {
    ModelFile {
        name,
        repo: HF_REPO,
        revision: HF_REVISION,
        sha256,
        size,
    }
}

/// The note models: the CQT front end and Okarina S/M/L.
pub const OKARINA_FILES: [ModelFile; 4] = [
    hf(
        "cqt156.onnx",
        "ff88505975e9d6c3e3c413aff5748abe711dfe3e9aee1bdf57bb637e28cf1c6f",
        17_479_436,
    ),
    hf(
        "okarina_s.onnx",
        "44ec3f8331b0f899b17437c37c4d4a0159fb46cc18f2c58bbdbcfb4bd195c00f",
        700_600,
    ),
    hf(
        "okarina_m.onnx",
        "c014fa25e9027e049c96734d36b589a66940d3b7f194f5020c5ae950e6a9319c",
        2_896_039,
    ),
    hf(
        "okarina_l.onnx",
        "7d35fdfc82980ade2359769bd09d10f4b3e74b529f8468a9c0c7ca39ff6f2916",
        13_141_883,
    ),
];

pub const DRUM_FILE: ModelFile = ModelFile {
    name: "adtof_frame_rnn.onnx",
    repo: DRUM_REPO,
    revision: DRUM_REVISION,
    sha256: "189ca3c2bc7339a5f0fdc0bb11069d4bcc4fafabd142e945228c368c379b6227",
    size: 2_171_762,
};

/// Beat This! small0 and its log-mel frontend. A checkout exports these itself (build.rs runs
/// export_beat_models.py); the copies the app downloads are the ones in `HF_REPO`.
pub const BEAT_FILES: [ModelFile; 2] = [
    hf(
        "beatthis_small0.onnx",
        "c2c0778c9243c8b3ea62c0d00750b555581c29b91591893d690a76b48d6496dd",
        10_555_794,
    ),
    hf(
        "logmel.onnx",
        "140a676b72c056a447ecff773ea7e9079b1a8065d3bd05102947e0d9fa9b3c8b",
        4_467_499,
    ),
];
