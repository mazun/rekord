//! Tells a discrete GPU from an integrated one, so the Okarina size can be
//! chosen automatically (see `harmonica_model::ModelChoice::Auto`).
//!
//! DirectML runs on adapter 0 (`DirectML::default()`); the DXGI enumeration
//! here uses the high-performance order, so its first non-software adapter is
//! the one the model lands on. `rekord_engine` prints the adapter name next
//! to the chosen size, to check that against a real machine.

/// What the GPU that DirectML would use is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuClass {
    /// Dedicated video memory of at least `DGPU_MIN_VRAM_BYTES`.
    Discrete(String),
    Integrated(String),
    /// No hardware adapter, or not on Windows.
    None,
}

/// Integrated GPUs share system memory and report a few hundred MB at most;
/// discrete cards start at several GB.
const DGPU_MIN_VRAM_BYTES: u64 = 1 << 30;

fn classify(dedicated_vram: u64, name: String) -> GpuClass {
    if dedicated_vram >= DGPU_MIN_VRAM_BYTES {
        GpuClass::Discrete(name)
    } else {
        GpuClass::Integrated(name)
    }
}

#[cfg(windows)]
pub fn probe() -> GpuClass {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory6, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
    };

    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory6>() }) else { return GpuClass::None };
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(index, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        return classify(desc.DedicatedVideoMemory as u64, String::from_utf16_lossy(&desc.Description[..len]));
    }
    GpuClass::None
}

#[cfg(not(windows))]
pub fn probe() -> GpuClass {
    GpuClass::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_class_splits_on_dedicated_memory() {
        assert_eq!(classify(16 << 30, "dGPU".into()), GpuClass::Discrete("dGPU".into()));
        assert_eq!(classify(512 << 20, "iGPU".into()), GpuClass::Integrated("iGPU".into()));
        assert_eq!(classify(1 << 30, "edge".into()), GpuClass::Discrete("edge".into()));
    }
}
