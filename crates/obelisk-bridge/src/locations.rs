//! Every path Obelisk uses, named once, and the removal of an installation.
use std::path::{Path, PathBuf};

/// Where Obelisk keeps everything, derived from its library directory, its
/// application bundle and the user's home.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ObeliskLocations {
    pub library: String,
    pub home: String,
    /// The Cloud archive every storefront shares. The folder keeps its first
    /// name so existing save versions and running Game sessions stay valid.
    pub cloud_archive: String,
    pub steam_sessions: String,
    pub epic_sessions: String,
    pub gog_sessions: String,
    /// The bundled native host every supervisor runs as.
    pub helper: String,
    pub helpers: String,
    /// Valve's Steam components the Steam host loads, which a Steam game's
    /// download, or a launch that misses them, gets from Valve's CDN.
    pub steam_runtime_package: String,
    pub steam_runtime_cache: String,
    /// The Voile runtimes that run Windows games: the pinned release and a
    /// development runtime.
    pub voile: String,
    /// One Wine prefix per Windows installation, named after its folder.
    pub wine_prefixes: String,
    /// Bundled VR Windows runtimes (OpenXR, OpenComposite, SteamVR driver_vrmin).
    pub vr_runtime: String,
    /// Bundled macOS Metal VR streaming renderer helper.
    pub vr_renderer: String,
    /// User-writable VR data directory.
    pub vr_data: String,
}

#[uniffi::export]
pub fn obelisk_locations(library: String, bundle: String, home: String) -> ObeliskLocations {
    let locations =
        game_runtime::Locations::new(Path::new(&library), Path::new(&bundle), Path::new(&home));
    let path = |path: PathBuf| path.to_string_lossy().into_owned();
    ObeliskLocations {
        library: path(locations.library),
        home: path(locations.home),
        cloud_archive: path(locations.cloud_archive),
        steam_sessions: path(locations.steam_sessions),
        epic_sessions: path(locations.epic_sessions),
        gog_sessions: path(locations.gog_sessions),
        helper: path(locations.helper),
        helpers: path(locations.helpers),
        steam_runtime_package: path(locations.steam_runtime_package),
        steam_runtime_cache: path(locations.steam_runtime_cache),
        voile: path(locations.voile),
        wine_prefixes: path(locations.wine_prefixes),
        vr_runtime: path(locations.vr_runtime),
        vr_renderer: path(locations.vr_renderer),
        vr_data: path(locations.vr_data),
    }
}

/// Ends the Windows game processes an Obelisk that died left frozen. Runs
/// once at startup, before any launch.
#[uniffi::export]
pub fn end_orphaned_windows_games(locations: ObeliskLocations) -> u32 {
    game_runtime::wine::end_orphans(Path::new(&locations.voile)) as u32
}

/// The Voile release Windows games run in now, once one is installed. Bug
/// reports name it.
#[uniffi::export]
pub fn installed_voile_release(locations: ObeliskLocations) -> Option<String> {
    game_runtime::voile::installed_release(Path::new(&locations.voile))
}

/// Whether Windows games run in Voile's pre-releases too, as Settings' beta
/// releases choose: the next launch takes the newest release or pre-release,
/// else it leaves a pre-release in place for the newest release. Returns
/// whether the choice was kept.
#[uniffi::export]
pub fn take_voile_prereleases(locations: ObeliskLocations, take: bool) -> bool {
    game_runtime::voile::take_prereleases(Path::new(&locations.voile), take).is_ok()
}

/// Current opt-in and installation state of an optional Proton runtime in
/// `Voile/`.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ProtonRuntimeStatus {
    /// Whether the player opted in to installing and keeping this Proton runtime.
    pub enabled: bool,
    /// Whether this Proton runtime's binaries are currently present on disk.
    pub installed: bool,
    /// Bytes currently occupied on disk (0 if not installed).
    pub installed_bytes: u64,
    /// Compressed archive download size in bytes for the current overlay release.
    pub download_bytes: u64,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ProtonRuntimeError {
    #[error("{message}")]
    Failed { message: String },
}

fn read_proton_experimental_status(voile: &Path) -> ProtonRuntimeStatus {
    ProtonRuntimeStatus {
        enabled: game_runtime::voile::proton_experimental_enabled(voile),
        installed: game_runtime::voile::proton_experimental_installed(voile),
        installed_bytes: game_runtime::voile::proton_experimental_disk_bytes(voile),
        download_bytes: game_runtime::voile::proton_experimental_download_bytes(voile),
    }
}

fn read_proton_ge_status(voile: &Path) -> ProtonRuntimeStatus {
    ProtonRuntimeStatus {
        enabled: game_runtime::voile::proton_ge_enabled(voile),
        installed: game_runtime::voile::proton_ge_installed(voile),
        installed_bytes: game_runtime::voile::proton_ge_disk_bytes(voile),
        download_bytes: game_runtime::voile::proton_ge_download_bytes(voile),
    }
}

/// Reports whether Proton Experimental is opted in, installed on disk, and how
/// much space it uses or requires to download.
#[uniffi::export]
pub fn proton_experimental_status(locations: ObeliskLocations) -> ProtonRuntimeStatus {
    read_proton_experimental_status(Path::new(&locations.voile))
}

/// Persists whether the player opted in to the optional Proton Experimental
/// runtime without removing already-installed files.
#[uniffi::export]
pub fn set_proton_experimental_enabled(
    locations: ObeliskLocations,
    enabled: bool,
) -> Result<ProtonRuntimeStatus, ProtonRuntimeError> {
    let voile = Path::new(&locations.voile);
    game_runtime::voile::set_proton_experimental_enabled(voile, enabled).map_err(|err| {
        ProtonRuntimeError::Failed {
            message: err.to_string(),
        }
    })?;
    Ok(read_proton_experimental_status(voile))
}

/// Opts in to Proton Experimental and downloads it immediately if the Linux
/// engine runtime is already installed.
#[uniffi::export(async_runtime = "tokio")]
pub async fn install_proton_experimental(
    locations: ObeliskLocations,
) -> Result<ProtonRuntimeStatus, ProtonRuntimeError> {
    let voile = Path::new(&locations.voile);
    game_runtime::voile::install_proton_experimental(voile)
        .await
        .map_err(|err| ProtonRuntimeError::Failed {
            message: err.to_string(),
        })?;
    Ok(read_proton_experimental_status(voile))
}

/// Removes the installed Proton Experimental runtime from disk to save space
/// and turns the opt-in preference off.
#[uniffi::export]
pub fn offload_proton_experimental(
    locations: ObeliskLocations,
) -> Result<ProtonRuntimeStatus, ProtonRuntimeError> {
    let voile = Path::new(&locations.voile);
    game_runtime::voile::offload_proton_experimental(voile).map_err(|err| {
        ProtonRuntimeError::Failed {
            message: err.to_string(),
        }
    })?;
    Ok(read_proton_experimental_status(voile))
}

/// Reports whether Proton-GE is opted in, installed on disk, and how much space
/// it uses or requires to download.
#[uniffi::export]
pub fn proton_ge_status(locations: ObeliskLocations) -> ProtonRuntimeStatus {
    read_proton_ge_status(Path::new(&locations.voile))
}

/// Persists whether the player opted in to the optional Proton-GE runtime
/// without removing already-installed files.
#[uniffi::export]
pub fn set_proton_ge_enabled(
    locations: ObeliskLocations,
    enabled: bool,
) -> Result<ProtonRuntimeStatus, ProtonRuntimeError> {
    let voile = Path::new(&locations.voile);
    game_runtime::voile::set_proton_ge_enabled(voile, enabled).map_err(|err| {
        ProtonRuntimeError::Failed {
            message: err.to_string(),
        }
    })?;
    Ok(read_proton_ge_status(voile))
}

/// Opts in to Proton-GE and downloads it immediately if the Linux engine
/// runtime is already installed.
#[uniffi::export(async_runtime = "tokio")]
pub async fn install_proton_ge(
    locations: ObeliskLocations,
) -> Result<ProtonRuntimeStatus, ProtonRuntimeError> {
    let voile = Path::new(&locations.voile);
    game_runtime::voile::install_proton_ge(voile)
        .await
        .map_err(|err| ProtonRuntimeError::Failed {
            message: err.to_string(),
        })?;
    Ok(read_proton_ge_status(voile))
}

/// Removes the installed Proton-GE runtime from disk to save space and turns
/// the opt-in preference off.
#[uniffi::export]
pub fn offload_proton_ge(
    locations: ObeliskLocations,
) -> Result<ProtonRuntimeStatus, ProtonRuntimeError> {
    let voile = Path::new(&locations.voile);
    game_runtime::voile::offload_proton_ge(voile).map_err(|err| ProtonRuntimeError::Failed {
        message: err.to_string(),
    })?;
    Ok(read_proton_ge_status(voile))
}

impl From<&ObeliskLocations> for game_runtime::Locations {
    fn from(locations: &ObeliskLocations) -> Self {
        Self {
            library: (&locations.library).into(),
            home: (&locations.home).into(),
            cloud_archive: (&locations.cloud_archive).into(),
            steam_sessions: (&locations.steam_sessions).into(),
            epic_sessions: (&locations.epic_sessions).into(),
            gog_sessions: (&locations.gog_sessions).into(),
            helper: (&locations.helper).into(),
            helpers: (&locations.helpers).into(),
            steam_runtime_package: (&locations.steam_runtime_package).into(),
            steam_runtime_cache: (&locations.steam_runtime_cache).into(),
            voile: (&locations.voile).into(),
            wine_prefixes: (&locations.wine_prefixes).into(),
            vr_runtime: (&locations.vr_runtime).into(),
            vr_renderer: (&locations.vr_renderer).into(),
            vr_data: (&locations.vr_data).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Voile release laid in `voile` as an install lays it.
    fn lay(voile: &Path, version: &str) {
        let runtime = voile.join(version);
        std::fs::create_dir_all(runtime.join("bin")).unwrap();
        std::fs::write(runtime.join("bin/wine"), "wine").unwrap();
        std::fs::write(
            runtime.join("runtime.json"),
            format!(r#"{{"version": "{version}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn beta_releases_choose_the_voile_release_windows_games_run_in() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().to_string_lossy().into_owned();
        let locations = obelisk_locations(library, "/Obelisk.app".into(), "/home".into());
        let voile = Path::new(&locations.voile);
        lay(voile, "v1.0.0-2");
        lay(voile, "v1.0.0-3");
        let entry = |version: &str| {
            format!(
                r#"{{"version": "{version}", "contract": 1, "url": "https://example.invalid/{version}", "size": 3, "sha256": "{}"}}"#,
                "a".repeat(64)
            )
        };
        std::fs::write(
            voile.join("feed.json"),
            format!(
                r#"{{"format": 1, "releases": [{}], "prereleases": [{}]}}"#,
                entry("v1.0.0-2"),
                entry("v1.0.0-3")
            ),
        )
        .unwrap();
        assert_eq!(
            installed_voile_release(locations.clone()).as_deref(),
            Some("v1.0.0-2")
        );
        assert!(take_voile_prereleases(locations.clone(), true));
        assert_eq!(
            installed_voile_release(locations.clone()).as_deref(),
            Some("v1.0.0-3")
        );
        assert!(take_voile_prereleases(locations.clone(), false));
        assert_eq!(
            installed_voile_release(locations).as_deref(),
            Some("v1.0.0-2")
        );
    }

    #[test]
    fn locations_name_every_folder_once() {
        let locations = obelisk_locations("/library".into(), "/Obelisk.app".into(), "/home".into());
        assert_eq!(locations.cloud_archive, "/library/SteamCloud");
        assert_eq!(locations.steam_sessions, "/library/SteamSessions");
        assert_eq!(locations.epic_sessions, "/library/EpicSessions");
        assert_eq!(locations.gog_sessions, "/library/GOGSessions");
        assert_eq!(
            locations.helpers,
            "/Obelisk.app/Contents/Helpers/SteamNative"
        );
        assert_eq!(
            locations.helper,
            "/Obelisk.app/Contents/Helpers/SteamNative/obelisk-steam-host"
        );
        assert_eq!(
            locations.steam_runtime_package,
            format!(
                "/home/Library/Caches/com.zagrios.obelisk/SteamRuntime/valve-{}.zip",
                game_runtime::package::manifest().version
            )
        );
        assert_eq!(
            locations.steam_runtime_cache,
            "/home/Library/Caches/com.zagrios.obelisk/SteamRuntime"
        );
        assert_eq!(locations.voile, "/library/Voile");
        assert_eq!(locations.wine_prefixes, "/library/Prefixes");
        let runtime = game_runtime::Locations::from(&locations);
        assert_eq!(runtime.home, Path::new("/home"));
        assert_eq!(
            game_runtime::wine::prefix(
                &runtime.wine_prefixes,
                Path::new("/library/Games/Steam/steam-42-windows")
            )
            .unwrap(),
            Path::new("/library/Prefixes/steam-42-windows")
        );
    }

    #[test]
    fn proton_experimental_status_toggle_and_offload_bridge() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().to_string_lossy().into_owned();
        let locations = obelisk_locations(library, "/Obelisk.app".into(), "/home".into());
        let voile = Path::new(&locations.voile);

        let initial = proton_experimental_status(locations.clone());
        assert!(!initial.enabled);
        assert!(!initial.installed);
        assert_eq!(initial.installed_bytes, 0);
        assert!(initial.download_bytes > 0);

        let enabled = set_proton_experimental_enabled(locations.clone(), true).unwrap();
        assert!(enabled.enabled);
        assert!(!enabled.installed);

        // Simulate an installed Proton Experimental runtime in Voile/current.
        let current = voile.join("current/linux");
        std::fs::create_dir_all(current.join("engine")).unwrap();
        std::fs::write(current.join("engine/fexdarwin-run"), "fex").unwrap();
        std::fs::create_dir_all(current.join("wine/bin")).unwrap();
        std::fs::write(current.join("wine/bin/wine"), "wine").unwrap();
        std::fs::create_dir_all(current.join("wine-proton-experimental/bin")).unwrap();
        std::fs::write(
            current.join("wine-proton-experimental/bin/wine"),
            vec![0u8; 2048],
        )
        .unwrap();

        let installed = proton_experimental_status(locations.clone());
        assert!(installed.enabled);
        assert!(installed.installed);
        assert_eq!(installed.installed_bytes, 2048);

        let offloaded = offload_proton_experimental(locations.clone()).unwrap();
        assert!(!offloaded.enabled);
        assert!(!offloaded.installed);
        assert_eq!(offloaded.installed_bytes, 0);

        // Proton-GE opt-in and offload works independently.
        assert!(!proton_ge_status(locations.clone()).enabled);
        assert!(set_proton_ge_enabled(locations.clone(), true).unwrap().enabled);
        std::fs::create_dir_all(current.join("wine-proton-ge/bin")).unwrap();
        std::fs::write(current.join("wine-proton-ge/bin/wine"), vec![0u8; 3072]).unwrap();
        let ge = proton_ge_status(locations.clone());
        assert!(ge.enabled);
        assert!(ge.installed);
        assert_eq!(ge.installed_bytes, 3072);
        let ge_offloaded = offload_proton_ge(locations).unwrap();
        assert!(!ge_offloaded.enabled);
        assert!(!ge_offloaded.installed);
        assert_eq!(ge_offloaded.installed_bytes, 0);
    }
}
