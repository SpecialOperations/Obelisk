//! SteamVR, OpenXR, and OpenVR (OpenComposite) runtime integration for Windows
//! games on Apple Silicon Macs, based on VRmin by thevermin
//! (https://github.com/thevermin/vrmin, GNU GPL v3.0).
//!
//! Inside a Rosetta Wine prefix this module provisions `C:\questvrbridge\`:
//! - OpenXR runtime (`questvrbridge_openxr.dll` / `questvrbridge_openxr32.dll`)
//! - OpenComposite (`vrclient_x64.dll` / `vrclient.dll`) for OpenVR → OpenXR
//! - VRmin's SteamVR driver (`driver_vrmin.dll`) for real Valve SteamVR
//! - `openvrpaths.vrpath` and `HKLM\Software\Khronos\OpenXR\1\ActiveRuntime`
//! - Metal GPU LUID synchronization in `system.reg` for zero-copy DXMT frames
//! - Real SteamVR (`vrstartup.exe`) startup and graceful `vrmonitor.exe` exit.

use crate::{Locations, launch::Error, wine};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Maximum directory entries inspected when scanning an installation for VR APIs.
const MAX_SCAN_ENTRIES: usize = 8_000;
/// Maximum recursion depth when scanning an installation for VR APIs.
const MAX_SCAN_DEPTH: usize = 5;
/// How long we wait for Valve SteamVR's `driver_vrmin` to activate controllers.
const STEAMVR_READY_TIMEOUT: Duration = Duration::from_secs(35);
/// How long graceful `taskkill /IM vrmonitor.exe` gets before falling back.
const STEAMVR_QUIT_TIMEOUT: Duration = Duration::from_secs(6);

/// How a Windows game connects to the Quest headset via Obelisk's VR renderer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VrMode {
    /// Flat desktop play (no VR runtime provisioned).
    #[default]
    Off,
    /// Auto-detect OpenXR vs OpenVR from the game's installation.
    Auto,
    /// Native OpenXR via Obelisk's bundled OpenXR runtime (`questvrbridge_openxr.dll`).
    OpenXr,
    /// OpenVR / SteamVR translated to OpenXR via OpenComposite (`vrclient_x64.dll`).
    OpenVr,
    /// Real Valve SteamVR (`vrstartup.exe` / `vrserver.exe`) with `driver_vrmin`.
    SteamVr,
}

impl VrMode {
    pub fn is_enabled(self) -> bool {
        self != Self::Off
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::OpenXr => "openxr",
            Self::OpenVr => "openvr",
            Self::SteamVr => "steamvr",
        }
    }
}

/// Concrete runtime path resolved for a single launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedVrRuntime {
    Off,
    OpenXr,
    OpenVr,
    SteamVr,
}

impl ResolvedVrRuntime {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::OpenXr => "openxr",
            Self::OpenVr => "openvr",
            Self::SteamVr => "steamvr",
        }
    }
}

/// Game engine hints discovered in an installation folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrEngine {
    #[default]
    Unknown,
    Unity,
    Unreal,
}

/// VR APIs and graphics hints discovered in a game's installation folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VrDetection {
    pub has_openxr: bool,
    pub has_openvr: bool,
    pub has_oculus: bool,
    pub has_vulkan: bool,
    pub has_d3d12: bool,
    pub engine: VrEngine,
}

impl VrDetection {
    pub fn is_vr_game(&self) -> bool {
        self.has_openxr || self.has_openvr || self.has_oculus
    }

    /// Resolves a player's chosen `mode` using the APIs detected in the game.
    pub fn resolve(&self, mode: VrMode, steamvr_available: bool) -> ResolvedVrRuntime {
        match mode {
            VrMode::Off => ResolvedVrRuntime::Off,
            VrMode::OpenXr => ResolvedVrRuntime::OpenXr,
            VrMode::OpenVr => ResolvedVrRuntime::OpenVr,
            VrMode::SteamVr => {
                if steamvr_available {
                    ResolvedVrRuntime::SteamVr
                } else {
                    ResolvedVrRuntime::OpenVr
                }
            }
            VrMode::Auto => {
                if self.has_openxr {
                    ResolvedVrRuntime::OpenXr
                } else if self.has_openvr {
                    ResolvedVrRuntime::OpenVr
                } else if self.has_oculus {
                    ResolvedVrRuntime::OpenXr
                } else {
                    ResolvedVrRuntime::OpenVr
                }
            }
        }
    }

    /// DirectX 11 flags recommended for Unity / Unreal VR titles unless the
    /// player or profile already picked a rendering API.
    pub fn dx11_arguments(&self, existing: &[String]) -> Vec<String> {
        let lower: Vec<String> = existing.iter().map(|a| a.to_ascii_lowercase()).collect();
        let has_any = |flags: &[&str]| lower.iter().any(|arg| flags.contains(&arg.as_str()));
        match self.engine {
            VrEngine::Unity => {
                if has_any(&[
                    "-force-d3d11",
                    "-force-d3d12",
                    "-force-vulkan",
                    "-force-glcore",
                    "-force-opengl",
                ]) {
                    Vec::new()
                } else {
                    vec!["-force-d3d11".into()]
                }
            }
            VrEngine::Unreal => {
                if has_any(&["-dx11", "-d3d11", "-dx12", "-d3d12", "-vulkan"]) {
                    Vec::new()
                } else {
                    vec!["-dx11".into()]
                }
            }
            VrEngine::Unknown => Vec::new(),
        }
    }
}

/// Availability of Obelisk's bundled VR payload and Valve's SteamVR installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VrRuntimeStatus {
    pub renderer_bundled: bool,
    pub openxr_bundled: bool,
    pub opencomposite_bundled: bool,
    pub steamvr_driver_bundled: bool,
    pub steamvr_path: Option<PathBuf>,
}

/// Scans `install` (bounded depth and entry count, never following symlinks)
/// for OpenXR, OpenVR, Oculus, Unity, Unreal, Vulkan, and DirectX 12 files.
pub fn detect(install: &Path) -> VrDetection {
    let mut found = VrDetection::default();
    let Ok(root) = install.canonicalize() else {
        return found;
    };
    let mut stack = vec![(root, 0usize)];
    let mut inspected = 0usize;

    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            inspected += 1;
            if inspected > MAX_SCAN_ENTRIES {
                return found;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if kind.is_dir() {
                if depth < MAX_SCAN_DEPTH && !name.starts_with('.') {
                    stack.push((entry.path(), depth + 1));
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            match name.as_str() {
                "openxr_loader.dll" | "unityopenxr.dll" => found.has_openxr = true,
                "openvr_api.dll" | "xrsdkopenvr.dll" => found.has_openvr = true,
                "ovrplugin.dll"
                | "libovrrt64_1.dll"
                | "libovrrt32_1.dll"
                | "oculusxrplugin.dll" => found.has_oculus = true,
                "unityplayer.dll" => found.engine = VrEngine::Unity,
                "vulkan-1.dll" => found.has_vulkan = true,
                "d3d12core.dll" => found.has_d3d12 = true,
                _ => {
                    if name.ends_with("-win64-shipping.exe") && found.engine == VrEngine::Unknown {
                        found.engine = VrEngine::Unreal;
                    }
                }
            }
        }
    }
    found
}

/// Inspects Obelisk's bundled VR files and looks for Valve's SteamVR installation.
pub fn runtime_status(locations: &Locations, custom_steamvr: Option<&Path>) -> VrRuntimeStatus {
    let vr = &locations.vr_runtime;
    VrRuntimeStatus {
        renderer_bundled: locations.vr_renderer.is_file(),
        openxr_bundled: vr.join("openxr/questvrbridge_openxr.dll").is_file()
            && vr.join("openxr/questvrbridge_openxr.json").is_file(),
        opencomposite_bundled: vr.join("opencomposite/vrclient_x64.dll").is_file(),
        steamvr_driver_bundled: vr
            .join("steamvr/driver_vrmin/bin/win64/driver_vrmin.dll")
            .is_file(),
        steamvr_path: find_steamvr(locations, custom_steamvr),
    }
}

/// Locates Valve's SteamVR installation (`bin/win64/vrstartup.exe`) from an
/// explicit override, Obelisk's saved VR config, Obelisk's Games folder, or
/// common Steam / VRmin locations on the Mac.
pub fn find_steamvr(locations: &Locations, custom_path: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = custom_path {
        candidates.push(path.to_owned());
    }
    if let Ok(saved) = fs::read_to_string(locations.vr_data.join("steamvr-path.txt")) {
        let trimmed = saved.trim();
        if !trimmed.is_empty() {
            candidates.push(PathBuf::from(trimmed));
        }
    }
    let games_root = fs::read_to_string(locations.library.join("games-folder.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("path")?.as_str().map(PathBuf::from))
        .unwrap_or_else(|| locations.library.join("Games"));
    candidates.push(games_root.join("Steam/SteamVR"));
    candidates.push(games_root.join("Steam/steamapps/common/SteamVR"));
    candidates.push(games_root.join("SteamVR"));
    candidates.push(
        locations
            .home
            .join("Library/Application Support/Steam/steamapps/common/SteamVR"),
    );
    candidates.push(locations.home.join(
        "Library/Application Support/QuestVRBridge/prefijo/drive_c/Program Files (x86)/Steam/steamapps/common/SteamVR",
    ));

    candidates
        .into_iter()
        .find(|dir| dir.join("bin/win64/vrstartup.exe").is_file())
}

/// Provisions a Rosetta Wine prefix for VR play: copies Obelisk's OpenXR,
/// OpenComposite, and `driver_vrmin` payloads into `C:\questvrbridge\`, writes
/// `openvrpaths.vrpath` and OpenXR registry entries, syncs the Metal GPU LUID
/// for zero-copy DXMT sharing, writes `lanzando.txt` for the Quest loading UI,
/// and starts Valve's SteamVR (`vrstartup.exe`) when in `SteamVr` mode.
pub(crate) fn prepare_prefix(
    config: &wine::Config,
    locations: &Locations,
    install: &Path,
    environment: &HashMap<String, String>,
) -> Result<ResolvedVrRuntime, Error> {
    if !config.settings.vr_mode.is_enabled() {
        return Ok(ResolvedVrRuntime::Off);
    }

    let payload = &locations.vr_runtime;
    if !payload.join("openxr/questvrbridge_openxr.dll").is_file() {
        return Ok(ResolvedVrRuntime::Off);
    }

    let steamvr_dir = find_steamvr(locations, None);
    let detection = detect(install);
    let resolved = detection.resolve(config.settings.vr_mode, steamvr_dir.is_some());

    let bridge_dir = config.prefix.join("drive_c/questvrbridge");
    let oc_bin_dir = bridge_dir.join("opencomposite/bin");
    let openvr_cfg_dir = bridge_dir.join("openvr/config");
    let openvr_log_dir = bridge_dir.join("openvr/logs");
    fs::create_dir_all(&oc_bin_dir)?;
    fs::create_dir_all(&openvr_cfg_dir)?;
    fs::create_dir_all(&openvr_log_dir)?;

    // 1. Copy OpenXR 64-bit & 32-bit runtime DLLs and manifests.
    for file in [
        "questvrbridge_openxr.dll",
        "questvrbridge_openxr.json",
        "questvrbridge_openxr32.dll",
        "questvrbridge_openxr32.json",
    ] {
        let src = payload.join("openxr").join(file);
        if src.is_file() {
            copy_if_changed(&src, &bridge_dir.join(file))?;
        }
    }

    // 2. Copy OpenComposite DLLs (64-bit and 32-bit).
    for file in ["vrclient_x64.dll", "vrclient.dll"] {
        let src = payload.join("opencomposite").join(file);
        if src.is_file() {
            copy_if_changed(&src, &oc_bin_dir.join(file))?;
        }
    }

    // 3. Copy VRmin's SteamVR driver.
    let driver_src = payload.join("steamvr/driver_vrmin");
    let driver_dst = bridge_dir.join("driver_vrmin");
    if driver_src.join("bin/win64/driver_vrmin.dll").is_file() {
        fs::create_dir_all(driver_dst.join("bin/win64"))?;
        copy_if_changed(
            &driver_src.join("driver.vrdrivermanifest"),
            &driver_dst.join("driver.vrdrivermanifest"),
        )?;
        copy_if_changed(
            &driver_src.join("bin/win64/driver_vrmin.dll"),
            &driver_dst.join("bin/win64/driver_vrmin.dll"),
        )?;
    }

    // 4. Write openvrpaths.vrpath and OpenXR registry entries.
    let vrpath_dir = wine::profile(&config.prefix).join("AppData/Local/openvr");
    fs::create_dir_all(&vrpath_dir)?;

    let mode_marker = bridge_dir.join(".obelisk_vr_mode");
    let previous_mode = fs::read_to_string(&mode_marker).unwrap_or_default();

    match resolved {
        ResolvedVrRuntime::OpenXr | ResolvedVrRuntime::OpenVr => {
            write_openvrpaths(&vrpath_dir, r"C:\questvrbridge\opencomposite", None)?;
            if previous_mode.trim() != "openxr" {
                set_openxr_runtime(
                    config,
                    &locations.home,
                    environment,
                    r"C:\questvrbridge\questvrbridge_openxr.json",
                    r"C:\questvrbridge\questvrbridge_openxr32.json",
                );
                let _ = fs::write(&mode_marker, "openxr");
            }
        }
        ResolvedVrRuntime::SteamVr => {
            if let Some(ref steamvr) = steamvr_dir {
                let steamvr_win = wine::windows_path(steamvr);
                write_steamvr_settings(&openvr_cfg_dir.join("steamvr.vrsettings"))?;
                disable_steamvr_desktop_preload(steamvr);
                write_openvrpaths(
                    &vrpath_dir,
                    &steamvr_win,
                    Some(r"C:\questvrbridge\driver_vrmin"),
                )?;
                let mode_tag = format!("steamvr:{steamvr_win}");
                if previous_mode.trim() != mode_tag {
                    let xr64 = format!(r"{steamvr_win}\steamxr_win64.json");
                    let xr32 = format!(r"{steamvr_win}\steamxr_win32.json");
                    set_openxr_runtime(config, &locations.home, environment, &xr64, &xr32);
                    let _ = fs::write(&mode_marker, &mode_tag);
                }
            }
        }
        ResolvedVrRuntime::Off => return Ok(ResolvedVrRuntime::Off),
    }

    // 5. Sync Wine's PCI adapter LUID with Metal's IOAccelerator registryID so
    //    DXMT can share GPU textures directly with ObeliskVRRenderer.
    sync_gpu_luid_dxmt(config, &locations.home, environment);

    // 6. Write lanzando.txt so the Quest headset shows the loading banner.
    write_launching_banner(&locations.vr_data, install, resolved);

    // 7. For real Valve SteamVR mode, launch vrstartup.exe in the game's prefix
    //    and wait for driver_vrmin to activate the HMD and both controllers.
    if resolved == ResolvedVrRuntime::SteamVr
        && let Some(ref steamvr) = steamvr_dir
    {
        start_steamvr(config, &locations.home, environment, steamvr);
    }

    Ok(resolved)
}

/// Provisions a Linux-engine (`voile-lkd` / Proton Experimental / Proton-GE)
/// Wine prefix for VR play so Easy Anti-Cheat titles like VRChat run in VR
/// while keeping Linux EAC active.
pub(crate) fn prepare_linux_prefix(
    config: &wine::Config,
    locations: &Locations,
    install: &Path,
) -> Result<ResolvedVrRuntime, Error> {
    if !config.settings.vr_mode.is_enabled() {
        return Ok(ResolvedVrRuntime::Off);
    }

    let payload = &locations.vr_runtime;
    if !payload.join("openxr/questvrbridge_openxr.dll").is_file() {
        return Ok(ResolvedVrRuntime::Off);
    }

    let detection = detect(install);
    // On the Linux engine, OpenXR and OpenComposite run directly inside the
    // guest prefix and connect to ObeliskVRRenderer on 127.0.0.1:9002 + shared
    // memory mapped from `<prefix>/drive_c/questvrbridge/cuadros_<pid>.bin`.
    let resolved = detection.resolve(config.settings.vr_mode, false);

    let bridge_dir = config.prefix.join("drive_c/questvrbridge");
    let oc_bin_dir = bridge_dir.join("opencomposite/bin");
    let openvr_cfg_dir = bridge_dir.join("openvr/config");
    let openvr_log_dir = bridge_dir.join("openvr/logs");
    fs::create_dir_all(&oc_bin_dir)?;
    fs::create_dir_all(&openvr_cfg_dir)?;
    fs::create_dir_all(&openvr_log_dir)?;

    for file in [
        "questvrbridge_openxr.dll",
        "questvrbridge_openxr.json",
        "questvrbridge_openxr32.dll",
        "questvrbridge_openxr32.json",
    ] {
        let src = payload.join("openxr").join(file);
        if src.is_file() {
            copy_if_changed(&src, &bridge_dir.join(file))?;
        }
    }

    for file in ["vrclient_x64.dll", "vrclient.dll"] {
        let src = payload.join("opencomposite").join(file);
        if src.is_file() {
            copy_if_changed(&src, &oc_bin_dir.join(file))?;
        }
    }

    let vrpath_dir = wine::profile(&config.prefix).join("AppData/Local/openvr");
    fs::create_dir_all(&vrpath_dir)?;
    write_openvrpaths(&vrpath_dir, r"C:\questvrbridge\opencomposite", None)?;

    // Register OpenXR ActiveRuntime directly in system.reg so no extra guest
    // wineboot/reg invocation is needed before EAC starts.
    let system_reg = config.prefix.join("system.reg");
    if system_reg.is_file() {
        upsert_openxr_system_reg(
            &system_reg,
            r"C:\questvrbridge\questvrbridge_openxr.json",
            r"C:\questvrbridge\questvrbridge_openxr32.json",
        )?;
    }

    // Tell ObeliskVRRenderer where `/home/user/prefix` lives on the macOS host
    // so it can mmap `cuadros_<pid>.bin` directly into Metal shared memory.
    let _ = fs::create_dir_all(&locations.vr_data);
    let _ = fs::write(
        locations.vr_data.join("linux-prefix-map.txt"),
        config.prefix.to_string_lossy().as_bytes(),
    );

    write_launching_banner(&locations.vr_data, install, resolved);
    Ok(resolved)
}

/// Inserts or updates `ActiveRuntime` under `Software\\Khronos\\OpenXR\\1` and
/// `Software\\Wow6432Node\\Khronos\\OpenXR\\1` in a Wine `system.reg` file.
fn upsert_openxr_system_reg(
    system_reg: &Path,
    runtime64: &str,
    runtime32: &str,
) -> std::io::Result<()> {
    let mut text = fs::read_to_string(system_reg)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for (section, value) in [
        (r"[Software\\Khronos\\OpenXR\\1]", runtime64),
        (r"[Software\\Wow6432Node\\Khronos\\OpenXR\\1]", runtime32),
    ] {
        let escaped = value.replace('\\', "\\\\");
        let value_line = format!("\"ActiveRuntime\"=\"{escaped}\"");
        text = upsert_reg_section(&text, section, &value_line, now);
    }
    obelisk_core::write_atomic(system_reg, text.as_bytes())
}

fn upsert_reg_section(text: &str, section_prefix: &str, value_line: &str, now: u64) -> String {
    let prefix_lower = section_prefix.trim_end_matches(']').to_ascii_lowercase();
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut start = None;
    for (idx, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with(&prefix_lower)
            && lower.as_bytes().get(prefix_lower.len()) == Some(&b']')
        {
            start = Some(idx);
            break;
        }
    }
    if let Some(sec_idx) = start {
        let mut end = sec_idx + 1;
        while end < lines.len() && !lines[end].starts_with('[') {
            end += 1;
        }
        for line in &mut lines[sec_idx + 1..end] {
            if line.to_ascii_lowercase().starts_with("\"activeruntime\"=") {
                *line = value_line.to_owned();
                let mut out = lines.join("\n");
                out.push('\n');
                return out;
            }
        }
        lines.insert(sec_idx + 1, value_line.to_owned());
        let mut out = lines.join("\n");
        out.push('\n');
        return out;
    }
    let mut out = text.to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("\n{section_prefix} {now}\n{value_line}\n"));
    out
}

/// Returns `true` when `ObeliskVRRenderer` is currently running, verified by
/// reading `<vr_data>/renderer.pid` and checking that the process is still alive.
pub(crate) fn is_renderer_active(vr_data: &Path) -> bool {
    let Ok(text) = fs::read_to_string(vr_data.join("renderer.pid")) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return false;
    };
    pid_is_alive(pid)
}

fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Removes Obelisk's VR runtime hooks (`openvrpaths.vrpath`, OpenXR
/// `ActiveRuntime` entries pointing to `questvrbridge`, and `.obelisk_vr_mode`)
/// from a Wine prefix so launching in Desktop mode never triggers VR init.
pub(crate) fn disable_in_prefix(config: &wine::Config) {
    let vrpath = wine::profile(&config.prefix).join("AppData/Local/openvr/openvrpaths.vrpath");
    if let Ok(text) = fs::read_to_string(&vrpath)
        && text.to_ascii_lowercase().contains("questvrbridge")
    {
        let _ = fs::remove_file(&vrpath);
    }

    let system_reg = config.prefix.join("system.reg");
    if system_reg.is_file() {
        let _ = remove_openxr_system_reg(&system_reg);
    }

    let mode_marker = config.prefix.join("drive_c/questvrbridge/.obelisk_vr_mode");
    let _ = fs::remove_file(&mode_marker);
}

fn remove_openxr_system_reg(system_reg: &Path) -> std::io::Result<()> {
    let text = fs::read_to_string(system_reg)?;
    let mut changed = false;
    let mut in_openxr = false;
    let mut kept: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.starts_with('[') {
            let lower = line.to_ascii_lowercase();
            in_openxr = lower.starts_with(r"[software\\khronos\\openxr\\1]")
                || lower.starts_with(r"[software\\wow6432node\\khronos\\openxr\\1]");
        } else if in_openxr {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("\"activeruntime\"=") && lower.contains("questvrbridge") {
                changed = true;
                continue;
            }
        }
        kept.push(line);
    }
    if changed {
        let mut out = kept.join("\n");
        out.push('\n');
        obelisk_core::write_atomic(system_reg, out.as_bytes())?;
    }
    Ok(())
}

/// Gracefully asks SteamVR's `vrmonitor.exe` to close before `wineserver -k`
/// terminates the prefix, avoiding SteamVR Safe Mode on the next launch.
pub(crate) fn stop_steamvr_gracefully(config: &wine::Config) {
    if !config.settings.vr_mode.is_enabled() {
        return;
    }
    let driver_log = config.prefix.join("drive_c/questvrbridge/driver_vrmin.log");
    if !driver_log.is_file() {
        return;
    }
    let wine = config.runtime.join("bin/wine");
    if !wine.is_file() {
        return;
    }
    let mut command = Command::new(wine);
    command
        .args(["taskkill", "/IM", "vrmonitor.exe"])
        .env_clear()
        .env("WINEPREFIX", &config.prefix)
        .env("WINEMSYNC", "1")
        .env("WINEDEBUG", "-all")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Ok(child) = command.spawn() {
        let _ = wine::wait(child, STEAMVR_QUIT_TIMEOUT);
    }
}

fn copy_if_changed(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let (Ok(sm), Ok(dm)) = (fs::metadata(src), fs::metadata(dst))
        && sm.len() == dm.len()
        && let (Ok(sb), Ok(db)) = (fs::read(src), fs::read(dst))
        && sb == db
    {
        return Ok(());
    }
    fs::copy(src, dst)?;
    Ok(())
}

fn escape_json_win_path(path: &str) -> String {
    path.replace('\\', "\\\\")
}

fn write_openvrpaths(
    dir: &Path,
    runtime_win: &str,
    external_driver_win: Option<&str>,
) -> std::io::Result<()> {
    let rt = escape_json_win_path(runtime_win);
    let ext = match external_driver_win {
        Some(drv) => format!(r#"[ "{}" ]"#, escape_json_win_path(drv)),
        None => "null".to_owned(),
    };
    let content = format!(
        "{{\n\t\"config\" : [ \"C:\\\\\\\\questvrbridge\\\\\\\\openvr\\\\\\\\config\" ],\n\t\"external_drivers\" : {ext},\n\t\"jsonid\" : \"vrpathreg\",\n\t\"log\" : [ \"C:\\\\\\\\questvrbridge\\\\\\\\openvr\\\\\\\\logs\" ],\n\t\"runtime\" : [ \"{rt}\" ],\n\t\"version\" : 1\n}}\n"
    );
    fs::write(dir.join("openvrpaths.vrpath"), content)
}

fn set_openxr_runtime(
    config: &wine::Config,
    home: &Path,
    environment: &HashMap<String, String>,
    runtime64: &str,
    runtime32: &str,
) {
    let wine = config.runtime.join("bin/wine");
    for (key, value) in [
        (r"HKLM\Software\Khronos\OpenXR\1", runtime64),
        (r"HKLM\Software\Wow6432Node\Khronos\OpenXR\1", runtime32),
    ] {
        let _ = Command::new(&wine)
            .args([
                "reg",
                "add",
                key,
                "/v",
                "ActiveRuntime",
                "/t",
                "REG_SZ",
                "/d",
                value,
                "/f",
            ])
            .env_clear()
            .envs(crate::native::environment(home))
            .envs(environment)
            .env("WINEDEBUG", "-all")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Reads Metal's primary `IOAccelerator` `registryID` and writes the matching
/// byte-swapped LUID to Wine's PCI adapter registry keys when they differ.
fn sync_gpu_luid_dxmt(
    config: &wine::Config,
    home: &Path,
    environment: &HashMap<String, String>,
) {
    let Some(reg_id) = metal_gpu_registry_id() else {
        return;
    };
    let system_reg = config.prefix.join("system.reg");
    let Ok(contents) = fs::read_to_string(&system_reg) else {
        return;
    };
    let want = format_luid_hex(reg_id);
    let keys = mismatched_luid_keys(&contents, &want);
    if keys.is_empty() {
        return;
    }
    let reg_file = config.prefix.join("drive_c/questvrbridge/luid.reg");
    let mut out = String::from("REGEDIT4\r\n");
    for key in keys {
        out.push_str(&format!(
            "\r\n[HKEY_LOCAL_MACHINE\\{key}]\r\n@=hex(ffff0009):{want}\r\n"
        ));
    }
    if fs::write(&reg_file, out).is_err() {
        return;
    }
    let _ = Command::new(config.runtime.join("bin/wine"))
        .args(["regedit", "/S", r"C:\questvrbridge\luid.reg"])
        .env_clear()
        .envs(crate::native::environment(home))
        .envs(environment)
        .env("WINEDEBUG", "-all")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn metal_gpu_registry_id() -> Option<u64> {
    let output = Command::new("/usr/sbin/ioreg")
        .args(["-r", "-c", "IOAccelerator", "-d", "1", "-w0"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let idx = stdout.find("id 0x")?;
    let hex: String = stdout[idx + 5..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    u64::from_str_radix(&hex, 16).ok()
}

fn format_luid_hex(registry_id: u64) -> String {
    registry_id
        .to_be_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Finds Wine `system.reg` PCI LUID keys whose value differs from `want_hex`.
fn mismatched_luid_keys(system_reg: &str, want_hex: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let lines: Vec<&str> = system_reg.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix('[')
            && let Some(end) = rest.find(']')
        {
            let section = &rest[..end];
            let lower = section.to_ascii_lowercase();
            if lower.starts_with(r"system\\controlset001\\enum\\pci\\")
                && lower.ends_with(
                    r"\\properties\\{60b193cb-5276-4d0f-96fc-f173abad3ec6}\\0002",
                )
            {
                let mut j = i + 1;
                while j < lines.len() && lines[j].starts_with('#') {
                    j += 1;
                }
                if j < lines.len()
                    && let Some(hex) = lines[j].strip_prefix("@=hex(ffff0009):")
                    && hex.trim().to_ascii_lowercase() != want_hex
                {
                    keys.push(section.replace("\\\\", "\\"));
                }
            }
        }
        i += 1;
    }
    keys
}

/// Writes VRmin's safe SteamVR configuration so SteamVR loads `driver_vrmin`
/// cleanly inside Wine without safe-mode blocks or broken desktop overlays.
fn write_steamvr_settings(path: &Path) -> std::io::Result<()> {
    let mut root: serde_json::Value = fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(obj) = root.as_object_mut() else {
        return Ok(());
    };

    let fixed = serde_json::json!({
        "driver_null": { "enable": false },
        "driver_lighthouse": { "enable": false },
        "driver_oculus": { "enable": false },
        "driver_oculus_legacy": { "enable": false },
        "driver_vrlink": { "enable": false },
        "driver_vrmin": { "enable": true, "blocked_by_safe_mode": false },
        "dashboard": { "useNewDesktop": false, "showDesktop": false },
        "collisionBounds": { "CollisionBoundsColorGammaA": 0, "CollisionBoundsFadeDistance": 0 },
        "steamvr": {
            "requireHmd": true,
            "forcedDriver": "vrmin",
            "activateMultipleDrivers": false,
            "enableHomeApp": false,
            "showMirrorView": false,
            "startMonitorFromAppLaunch": false,
            "startOverlayAppsFromDashboard": false,
            "allowDisplayLockedMode": true,
            "supersampleManualOverride": true,
            "supersampleScale": 1.0
        }
    });

    if let Some(fixed_map) = fixed.as_object() {
        for (section, values) in fixed_map {
            let entry = obj
                .entry(section.clone())
                .or_insert_with(|| serde_json::json!({}));
            if let (Some(dest), Some(src)) = (entry.as_object_mut(), values.as_object()) {
                for (k, v) in src {
                    dest.insert(k.clone(), v.clone());
                }
            }
        }
    }

    let formatted = serde_json::to_string_pretty(&root).unwrap_or_else(|_| "{}".into());
    fs::write(path, formatted)
}

fn disable_steamvr_desktop_preload(steamvr: &Path) {
    let path = steamvr.join("resources/webhelperoverlays.json");
    let Ok(contents) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return;
    };
    if let Some(desktop) = root
        .get_mut("settings_desktop")
        .and_then(|v| v.as_object_mut())
        && desktop.get("preload").and_then(|v| v.as_bool()) == Some(true)
    {
        desktop.insert("preload".into(), serde_json::Value::Bool(false));
        if let Ok(text) = serde_json::to_string_pretty(&root) {
            let _ = fs::write(&path, text);
        }
    }
}

fn start_steamvr(
    config: &wine::Config,
    home: &Path,
    environment: &HashMap<String, String>,
    steamvr: &Path,
) {
    let driver_log = config.prefix.join("drive_c/questvrbridge/driver_vrmin.log");
    let _ = fs::remove_file(&driver_log);
    let startup = steamvr.join("bin/win64/vrstartup.exe");
    if !startup.is_file() {
        return;
    }
    let _ = Command::new(config.runtime.join("bin/wine"))
        .arg(wine::windows_path(&startup))
        .env_clear()
        .envs(crate::native::environment(home))
        .envs(environment)
        .env("WINEDEBUG", "-all")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    let deadline = Instant::now() + STEAMVR_READY_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(log) = fs::read_to_string(&driver_log)
            && log.contains("Right controller activated")
        {
            break;
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn write_launching_banner(vr_data: &Path, install: &Path, runtime: ResolvedVrRuntime) {
    if fs::create_dir_all(vr_data).is_err() {
        return;
    }
    let folder = install
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "VR Game".into());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let line = format!("{folder}\t{folder}\t{now}\t{}\t{folder}\n", runtime.name());
    if let Ok(mut file) = fs::File::create(vr_data.join("lanzando.txt")) {
        let _ = file.write_all(line.as_bytes());
    }
    let _ = fs::write(vr_data.join("active-game.txt"), folder.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_openxr_openvr_unity_and_unreal_markers() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("Game_Data/Plugins/x86_64");
        fs::create_dir_all(&plugins).unwrap();
        fs::write(dir.path().join("UnityPlayer.dll"), b"").unwrap();
        fs::write(plugins.join("openvr_api.dll"), b"").unwrap();

        let detected = detect(dir.path());
        assert!(detected.is_vr_game());
        assert!(detected.has_openvr);
        assert!(!detected.has_openxr);
        assert_eq!(detected.engine, VrEngine::Unity);
        assert_eq!(detected.resolve(VrMode::Auto, false), ResolvedVrRuntime::OpenVr);
        assert_eq!(detected.dx11_arguments(&[]), vec!["-force-d3d11"]);
        assert!(detected.dx11_arguments(&["-force-vulkan".into()]).is_empty());
    }

    #[test]
    fn resolves_steamvr_with_fallback_when_uninstalled() {
        let detected = VrDetection {
            has_openvr: true,
            ..Default::default()
        };
        assert_eq!(
            detected.resolve(VrMode::SteamVr, true),
            ResolvedVrRuntime::SteamVr
        );
        assert_eq!(
            detected.resolve(VrMode::SteamVr, false),
            ResolvedVrRuntime::OpenVr
        );
    }

    #[test]
    fn formats_and_finds_mismatched_metal_luid_keys() {
        let want = format_luid_hex(0x0000_0001_0000_04ab);
        assert_eq!(want, "00,00,00,01,00,00,04,ab");

        let reg = concat!(
            "[System\\\\ControlSet001\\\\Enum\\\\PCI\\\\VEN_106B&DEV_0000\\\\0000\\\\Properties\\\\{60B193CB-5276-4D0F-96FC-F173ABAD3EC6}\\\\0002] 123\n",
            "#time=123\n",
            "@=hex(ffff0009):11,22,33,44,55,66,77,88\n",
        );
        let mismatched = mismatched_luid_keys(reg, &want);
        assert_eq!(mismatched.len(), 1);
        assert!(mismatched[0].starts_with(r"System\ControlSet001\Enum\PCI\"));
    }

    #[test]
    fn writes_valid_openvrpaths_and_steamvr_settings() {
        let dir = tempfile::tempdir().unwrap();
        write_openvrpaths(
            dir.path(),
            r"Z:\Games\SteamVR",
            Some(r"C:\questvrbridge\driver_vrmin"),
        )
        .unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join("openvrpaths.vrpath")).unwrap())
                .unwrap();
        assert_eq!(parsed["runtime"][0], r"Z:\Games\SteamVR");
        assert_eq!(parsed["external_drivers"][0], r"C:\questvrbridge\driver_vrmin");

        let settings_path = dir.path().join("steamvr.vrsettings");
        write_steamvr_settings(&settings_path).unwrap();
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(settings["steamvr"]["forcedDriver"], "vrmin");
        assert_eq!(settings["driver_vrmin"]["enable"], true);
        assert_eq!(settings["driver_vrmin"]["blocked_by_safe_mode"], false);
    }

    #[test]
    fn renderer_active_checks_live_pid_and_disable_cleans_prefix() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_renderer_active(dir.path()));

        fs::write(dir.path().join("renderer.pid"), "99999999\n").unwrap();
        assert!(!is_renderer_active(dir.path()));

        fs::write(
            dir.path().join("renderer.pid"),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
        assert!(is_renderer_active(dir.path()));

        let system_reg = dir.path().join("system.reg");
        fs::write(&system_reg, "WINE REGISTRY Version 2\n").unwrap();
        upsert_openxr_system_reg(
            &system_reg,
            r"C:\questvrbridge\questvrbridge_openxr.json",
            r"C:\questvrbridge\questvrbridge_openxr32.json",
        )
        .unwrap();
        let after_add = fs::read_to_string(&system_reg).unwrap();
        assert!(after_add.contains("questvrbridge_openxr.json"));

        remove_openxr_system_reg(&system_reg).unwrap();
        let after_remove = fs::read_to_string(&system_reg).unwrap();
        assert!(!after_remove.contains("questvrbridge_openxr.json"));
    }
}
