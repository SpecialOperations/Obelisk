//! Virtual reality detection and runtime inspection across UniFFI.
use crate::ObeliskLocations;
use game_runtime::vr;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum VrGameEngine {
    Unknown,
    Unity,
    Unreal,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct VrGameDetection {
    pub is_vr: bool,
    pub has_openxr: bool,
    pub has_openvr: bool,
    pub has_oculus: bool,
    pub has_vulkan: bool,
    pub has_d3d12: bool,
    pub engine: VrGameEngine,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct VrRuntimeInfo {
    pub renderer_bundled: bool,
    pub openxr_bundled: bool,
    pub opencomposite_bundled: bool,
    pub steamvr_driver_bundled: bool,
    pub steamvr_path: Option<String>,
}

/// Scans an installed game folder for VR APIs (OpenXR, OpenVR, Oculus) and engine hints.
#[uniffi::export]
pub fn detect_game_vr(install_path: String) -> VrGameDetection {
    let detected = vr::detect(Path::new(&install_path));
    VrGameDetection {
        is_vr: detected.is_vr_game(),
        has_openxr: detected.has_openxr,
        has_openvr: detected.has_openvr,
        has_oculus: detected.has_oculus,
        has_vulkan: detected.has_vulkan,
        has_d3d12: detected.has_d3d12,
        engine: match detected.engine {
            vr::VrEngine::Unknown => VrGameEngine::Unknown,
            vr::VrEngine::Unity => VrGameEngine::Unity,
            vr::VrEngine::Unreal => VrGameEngine::Unreal,
        },
    }
}

/// Inspects Obelisk's bundled VR payload and Valve's SteamVR installation.
#[uniffi::export]
pub fn vr_runtime_info(
    locations: ObeliskLocations,
    custom_steamvr_path: Option<String>,
) -> VrRuntimeInfo {
    let locations: game_runtime::Locations = (&locations).into();
    let custom = custom_steamvr_path
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(Path::new);
    let status = vr::runtime_status(&locations, custom);
    VrRuntimeInfo {
        renderer_bundled: status.renderer_bundled,
        openxr_bundled: status.openxr_bundled,
        opencomposite_bundled: status.opencomposite_bundled,
        steamvr_driver_bundled: status.steamvr_driver_bundled,
        steamvr_path: status
            .steamvr_path
            .map(|p| p.to_string_lossy().into_owned()),
    }
}
