//! Obelisk FX automatic Linux-engine runtime overlay.
//!
//! Instead of requiring users to run a terminal script after installing
//! Obelisk FX, this module fetches the runtime overlay published on
//! `SpecialOperations/voile-lkd` (MetalFX Spatial + 2× Frame Generation +
//! Mic Voice Isolation + native clipboard sync + native macOS Full Screen `wsd`,
//! patched `fexdarwin-run`, Neptune 512 MiB / shared-texture / coalescing SHM
//! server & guest DLLs, and Proton-RTSP 11.0 + GStreamer 1.26.2 Wine media +
//! clipboard pack), verifies SHA-256 checksums and Mach-O code
//! signatures, clones the newest installed upstream Voile into `Voile/current`
//! using APFS copy-on-write, applies the overlay, and syncs updated DLLs into
//! existing Linux prefixes.
use super::{DEVELOPMENT, Failure, Install, UNFINISHED, has_wine, newest, Offer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

const RELEASES_API: &str = "https://api.github.com/repos/SpecialOperations/voile-lkd/releases/latest";
const USER_AGENT: &str = "Obelisk-FX";
/// Cached overlay release metadata inside `Voile/`.
pub(super) const FEED_FILE: &str = "fx-overlay-feed.json";
/// Marker file placed inside `Voile/current/` when managed by Obelisk FX.
/// A `Voile/current/` directory without this marker is treated as a manual
/// developer runtime and left untouched.
const STAMP_FILE: &str = ".obelisk-fx-overlay.json";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct OverlayAsset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct OverlayRelease {
    pub tag: String,
    pub wsd: OverlayAsset,
    pub fexdarwin_run: Option<OverlayAsset>,
    pub virgl_render_server: Option<OverlayAsset>,
    pub libvirglrenderer: Option<OverlayAsset>,
    #[serde(default)]
    pub neptune_windows: Option<OverlayAsset>,
    #[serde(default)]
    pub gst_rtsp_root: Option<OverlayAsset>,
    pub wine_media_clipboard: Option<OverlayAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct OverlayStamp {
    tag: String,
    base_voile: String,
}

/// Built-in pinned fallback for `v0.1.6-metalfx` so Obelisk FX can download
/// and verify the runtime overlay directly from GitHub release assets even if
/// GitHub's REST API is rate-limited.
fn built_in() -> OverlayRelease {
    const BASE: &str =
        "https://github.com/SpecialOperations/voile-lkd/releases/download/v0.1.6-metalfx";
    OverlayRelease {
        tag: "v0.1.6-metalfx".into(),
        wsd: OverlayAsset {
            url: format!("{BASE}/wsd-arm64"),
            size: 249_568,
            sha256: "b6b8287231c85c9c373986afa979af68c8325a2c02c35c68bffd5005c7212d92".into(),
        },
        fexdarwin_run: Some(OverlayAsset {
            url: format!("{BASE}/fexdarwin-run-arm64"),
            size: 4_131_696,
            sha256: "846c34092ad1fcb587701785404c70cf213a28581b4d439c57b7e7ea5ea3e3f8".into(),
        }),
        virgl_render_server: Some(OverlayAsset {
            url: format!("{BASE}/virgl_render_server-arm64"),
            size: 1_634_688,
            sha256: "7f456756f910467b297458ed15b9ecccd91e480afd5f0b4688ffa22e13d2c55b".into(),
        }),
        libvirglrenderer: Some(OverlayAsset {
            url: format!("{BASE}/libvirglrenderer.1.dylib-arm64"),
            size: 2_066_464,
            sha256: "6c482e83d45da214d8951a216b125d01c14e3476535df34fd18e49c5d4bb724e".into(),
        }),
        neptune_windows: Some(OverlayAsset {
            url: format!("{BASE}/neptune-windows-x86_64.tar.gz"),
            size: 1_631_643,
            sha256: "9e2044e95a497403ee95ded4fb7fe16c037d0f6d4aafe36fe47ab48253456ef0".into(),
        }),
        gst_rtsp_root: Some(OverlayAsset {
            url: format!("{BASE}/gst-rtsp-root-x86_64.tar.gz"),
            size: 1_401_960,
            sha256: "18b43c40335c67d4046500e277b683846261e22c283fbd0eedae693d4783590f".into(),
        }),
        wine_media_clipboard: Some(OverlayAsset {
            url: format!("{BASE}/wine-media-clipboard-x86_64.tar.gz"),
            size: 5_300_072,
            sha256: "ad5b05bd8870694b249741ae56d0db839a35e52569ca658543509ae62d897958".into(),
        }),
    }
}

fn tag_version_key(tag: &str) -> Vec<u64> {
    let trimmed = tag.trim_start_matches('v');
    let core = trimmed.split('-').next().unwrap_or(trimmed);
    core.split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

fn disabled() -> bool {
    std::env::var_os("OBELISK_FX_DISABLE_OVERLAY")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Whether `Voile/current` exists and was created/managed by Obelisk FX.
pub(crate) fn is_managed(voile_root: &Path) -> bool {
    read_stamp(&voile_root.join(DEVELOPMENT)).is_some()
}

fn read_stamp(current_dir: &Path) -> Option<OverlayStamp> {
    let bytes = fs::read(current_dir.join(STAMP_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn read_cached_feed(voile_root: &Path) -> Option<OverlayRelease> {
    let bytes = fs::read(voile_root.join(FEED_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The overlay release Obelisk FX wants to have installed: the cached latest
/// release from `SpecialOperations/voile-lkd` when newer than the built-in pin,
/// else the built-in pin.
pub(crate) fn wanted(voile_root: &Path) -> OverlayRelease {
    let fallback = built_in();
    match read_cached_feed(voile_root) {
        Some(cached)
            if is_valid_release(&cached)
                && tag_version_key(&cached.tag) > tag_version_key(&fallback.tag) =>
        {
            cached
        }
        _ => fallback,
    }
}

fn is_valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_valid_asset(asset: &OverlayAsset) -> bool {
    asset.url.starts_with("https://") && asset.size > 0 && is_valid_sha256(&asset.sha256)
}

fn is_valid_release(rel: &OverlayRelease) -> bool {
    !rel.tag.is_empty()
        && is_valid_asset(&rel.wsd)
        && rel.fexdarwin_run.as_ref().is_none_or(is_valid_asset)
        && rel.virgl_render_server.as_ref().is_none_or(is_valid_asset)
        && rel.libvirglrenderer.as_ref().is_none_or(is_valid_asset)
        && rel.neptune_windows.as_ref().is_none_or(is_valid_asset)
        && rel.gst_rtsp_root.as_ref().is_none_or(is_valid_asset)
        && rel.wine_media_clipboard.as_ref().is_none_or(is_valid_asset)
}

/// Whether the Obelisk FX runtime overlay should be installed or updated in
/// `voile_root`.
///
/// - Respects manual developer `Voile/current` installations (no stamp file).
/// - Returns `true` when no `Voile/current` exists yet (once a base Voile
///   release is installed or being installed), or when a managed `Voile/current`
///   is behind the wanted overlay tag or behind the newest installed base Voile.
pub(crate) fn needs_install(voile_root: &Path) -> bool {
    if disabled() {
        return false;
    }
    let current = voile_root.join(DEVELOPMENT);
    let base_name = newest(voile_root, &Offer::read(voile_root))
        .and_then(|(_, path)| path.file_name().map(|n| n.to_string_lossy().into_owned()));

    if has_wine(&current) {
        let Some(stamp) = read_stamp(&current) else {
            // Manual developer runtime (e.g., install-dev.sh or manual script): never clobber.
            return false;
        };
        let wanted = wanted(voile_root);
        let base_matches = base_name.as_deref().is_none_or(|b| b == stamp.base_voile);
        let wsd_present = current.join("linux/engine/wsd").is_file();
        !(stamp.tag == wanted.tag && base_matches && wsd_present)
    } else {
        // No Voile/current yet: apply overlay as soon as a base Voile with Linux engine exists.
        base_name.is_some_and(|name| {
            voile_root
                .join(name)
                .join("linux/engine/wsd")
                .is_file()
        })
    }
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// Refreshes `Voile/fx-overlay-feed.json` from `SpecialOperations/voile-lkd`
/// latest release. Silently keeps the previous cache or built-in fallback on
/// network errors or rate limits.
pub(crate) async fn refresh(voile_root: &Path) {
    if disabled() {
        return;
    }
    let Ok(client) = reqwest::Client::builder()
        .https_only(true)
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(12))
        .build()
    else {
        return;
    };
    let Ok(resp) = client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    else {
        return;
    };
    let Ok(resp) = resp.error_for_status() else {
        return;
    };
    let Ok(gh) = resp.json::<GitHubRelease>().await else {
        return;
    };
    // Skip re-fetching .sha256 sidecars when the cached feed already matches this tag.
    if read_cached_feed(voile_root).is_some_and(|c| c.tag == gh.tag_name && is_valid_release(&c)) {
        return;
    }

    let Some(wsd) = resolve_asset(&client, &gh.assets, "wsd-arm64").await else {
        return;
    };
    let fexdarwin_run = resolve_asset(&client, &gh.assets, "fexdarwin-run-arm64").await;
    let virgl_render_server =
        resolve_asset(&client, &gh.assets, "virgl_render_server-arm64").await;
    let libvirglrenderer =
        resolve_asset(&client, &gh.assets, "libvirglrenderer.1.dylib-arm64").await;
    let neptune_windows =
        resolve_asset(&client, &gh.assets, "neptune-windows-x86_64.tar.gz").await;
    let gst_rtsp_root =
        resolve_asset(&client, &gh.assets, "gst-rtsp-root-x86_64.tar.gz").await;
    let wine_media_clipboard =
        resolve_asset(&client, &gh.assets, "wine-media-clipboard-x86_64.tar.gz").await;

    let release = OverlayRelease {
        tag: gh.tag_name,
        wsd,
        fexdarwin_run,
        virgl_render_server,
        libvirglrenderer,
        neptune_windows,
        gst_rtsp_root,
        wine_media_clipboard,
    };
    if !is_valid_release(&release) {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&release) {
        let _ = crate::remote::keep(&voile_root.join(FEED_FILE), &bytes);
    }
}

async fn resolve_asset(
    client: &reqwest::Client,
    assets: &[GitHubAsset],
    name: &str,
) -> Option<OverlayAsset> {
    let bin = assets.iter().find(|a| a.name == name)?;
    let sha_name = format!("{name}.sha256");
    let sha_asset = assets.iter().find(|a| a.name == sha_name)?;
    let text = client
        .get(&sha_asset.browser_download_url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    let sha256 = text.split_whitespace().next()?.to_ascii_lowercase();
    if !is_valid_sha256(&sha256) {
        return None;
    }
    Some(OverlayAsset {
        url: bin.browser_download_url.clone(),
        size: bin.size,
        sha256,
    })
}

/// Checks whether the base Voile runtime's `runtime.json` matches the Neptune
/// commit our patched `virgl_render_server` / `libvirglrenderer.1.dylib` target.
fn matches_neptune(base_dir: &Path) -> bool {
    let Ok(bytes) = fs::read(base_dir.join("runtime.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    let sha = json
        .get("linux")
        .and_then(|l| l.get("neptune"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    sha.starts_with("265be8a") || sha.starts_with("be783d9")
}

fn has_compatible_wine_ffmpeg(base_dir: &Path) -> bool {
    base_dir.join("linux/wine/lib/wine/x86_64-windows").is_dir()
        && base_dir
            .join("linux/root/usr/lib/x86_64-linux-gnu/libavcodec.so.61")
            .exists()
}

/// Downloads, verifies, and applies the Obelisk FX runtime overlay into
/// `Voile/current`, cloning the newest installed base `Voile/v*` release via
/// APFS copy-on-write and syncing updated Wine DLLs into existing Linux
/// prefixes.
///
/// If a managed `Voile/current` already exists and the network is unavailable
/// when checking/downloading a newer overlay, this returns `Ok(())` so offline
/// play keeps working with the previously installed overlay.
pub(crate) async fn install(
    voile_root: &Path,
    prefixes_root: &Path,
    progress: &Install,
) -> Result<(), Failure> {
    if !needs_install(voile_root) {
        return Ok(());
    }
    let Some((_, base_dir)) = newest(voile_root, &Offer::read(voile_root)) else {
        return Ok(());
    };
    let base_version = base_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or(Failure::Storage)?;
    if !base_dir.join("linux/engine/wsd").is_file() {
        return Ok(());
    }

    let overlay = wanted(voile_root);
    let already_has_managed = is_managed(voile_root);

    let neptune_ok = matches_neptune(&base_dir);
    let want_neptune = neptune_ok
        && overlay.virgl_render_server.is_some()
        && overlay.libvirglrenderer.is_some();
    let want_neptune_windows =
        neptune_ok && base_dir.join("linux/windows").is_dir() && overlay.neptune_windows.is_some();
    let wine_compat = has_compatible_wine_ffmpeg(&base_dir);
    let want_gst_root = wine_compat && overlay.gst_rtsp_root.is_some();
    let want_wine = wine_compat && overlay.wine_media_clipboard.is_some();

    let mut total = overlay.wsd.size;
    if let Some(a) = &overlay.fexdarwin_run {
        total += a.size;
    }
    if want_neptune {
        total += overlay.virgl_render_server.as_ref().unwrap().size;
        total += overlay.libvirglrenderer.as_ref().unwrap().size;
    }
    if want_neptune_windows {
        total += overlay.neptune_windows.as_ref().unwrap().size;
    }
    if want_gst_root {
        total += overlay.gst_rtsp_root.as_ref().unwrap().size;
    }
    if want_wine {
        total += overlay.wine_media_clipboard.as_ref().unwrap().size;
    }
    progress.set_progress(0, total);

    let work = tempfile::Builder::new()
        .prefix(UNFINISHED)
        .tempdir_in(voile_root)?;

    let mut received = 0_u64;
    let downloaded = match download_all(
        &overlay,
        want_neptune,
        want_neptune_windows,
        want_gst_root,
        want_wine,
        work.path(),
        progress,
        &mut received,
        total,
    )
    .await
    {
        Ok(files) => files,
        Err(Failure::Download) if already_has_managed => {
            // Keep offline play working with the existing managed overlay.
            return Ok(());
        }
        Err(err) => return Err(err),
    };

    let voile_root = voile_root.to_owned();
    let prefixes_root = prefixes_root.to_owned();
    let tag = overlay.tag.clone();
    tokio::task::spawn_blocking(move || {
        apply_staged(
            &voile_root,
            &prefixes_root,
            &base_dir,
            &base_version,
            &tag,
            &downloaded,
        )
    })
    .await
    .map_err(|_| Failure::Storage)?
}

struct DownloadedFiles {
    wsd: PathBuf,
    fexdarwin_run: Option<PathBuf>,
    virgl_render_server: Option<PathBuf>,
    libvirglrenderer: Option<PathBuf>,
    neptune_windows: Option<PathBuf>,
    gst_rtsp_root: Option<PathBuf>,
    wine_media_clipboard: Option<PathBuf>,
}

#[allow(clippy::too_many_arguments)]
async fn download_all(
    overlay: &OverlayRelease,
    want_neptune: bool,
    want_neptune_windows: bool,
    want_gst_root: bool,
    want_wine: bool,
    dir: &Path,
    progress: &Install,
    received: &mut u64,
    total: u64,
) -> Result<DownloadedFiles, Failure> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| Failure::Download)?;

    let wsd = download_asset(&client, &overlay.wsd, dir, "wsd", progress, received, total).await?;
    let fexdarwin_run = match &overlay.fexdarwin_run {
        Some(a) => Some(
            download_asset(&client, a, dir, "fexdarwin-run", progress, received, total).await?,
        ),
        None => None,
    };
    let (virgl_render_server, libvirglrenderer) = if want_neptune {
        let srv = download_asset(
            &client,
            overlay.virgl_render_server.as_ref().unwrap(),
            dir,
            "virgl_render_server",
            progress,
            received,
            total,
        )
        .await?;
        let lib = download_asset(
            &client,
            overlay.libvirglrenderer.as_ref().unwrap(),
            dir,
            "libvirglrenderer.1.dylib",
            progress,
            received,
            total,
        )
        .await?;
        (Some(srv), Some(lib))
    } else {
        (None, None)
    };
    let neptune_windows = if want_neptune_windows {
        Some(
            download_asset(
                &client,
                overlay.neptune_windows.as_ref().unwrap(),
                dir,
                "neptune-windows.tar.gz",
                progress,
                received,
                total,
            )
            .await?,
        )
    } else {
        None
    };
    let gst_rtsp_root = if want_gst_root {
        Some(
            download_asset(
                &client,
                overlay.gst_rtsp_root.as_ref().unwrap(),
                dir,
                "gst-rtsp-root.tar.gz",
                progress,
                received,
                total,
            )
            .await?,
        )
    } else {
        None
    };
    let wine_media_clipboard = if want_wine {
        Some(
            download_asset(
                &client,
                overlay.wine_media_clipboard.as_ref().unwrap(),
                dir,
                "wine-media-clipboard.tar.gz",
                progress,
                received,
                total,
            )
            .await?,
        )
    } else {
        None
    };

    Ok(DownloadedFiles {
        wsd,
        fexdarwin_run,
        virgl_render_server,
        libvirglrenderer,
        neptune_windows,
        gst_rtsp_root,
        wine_media_clipboard,
    })
}

async fn download_asset(
    client: &reqwest::Client,
    asset: &OverlayAsset,
    dir: &Path,
    filename: &str,
    progress: &Install,
    received: &mut u64,
    total: u64,
) -> Result<PathBuf, Failure> {
    let mut response = progress
        .cancellable(client.get(&asset.url).send())
        .await?
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| Failure::Download)?;
    let path = dir.join(filename);
    let mut file = fs::File::create(&path)?;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    while let Some(bytes) = progress
        .cancellable(response.chunk())
        .await?
        .map_err(|_| Failure::Download)?
    {
        size += bytes.len() as u64;
        if size > asset.size {
            return Err(Failure::Damaged);
        }
        digest.update(&bytes);
        file.write_all(&bytes)?;
        *received += bytes.len() as u64;
        progress.set_progress(*received, total);
    }
    if size != asset.size || format!("{:x}", digest.finalize()) != asset.sha256 {
        return Err(Failure::Damaged);
    }
    Ok(path)
}

fn run_cmd(program: &str, args: &[&Path]) -> Result<(), Failure> {
    let status = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    status.success().then_some(()).ok_or(Failure::Damaged)
}

fn install_macho(src: &Path, dst: &Path, verify_codesign: bool) -> Result<(), Failure> {
    fs::set_permissions(src, fs::Permissions::from_mode(0o755))?;
    if verify_codesign {
        let status = std::process::Command::new("/usr/bin/codesign")
            .arg("--verify")
            .arg(src)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if !status.success() {
            return Err(Failure::Damaged);
        }
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dst)?;
    fs::set_permissions(dst, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn apply_staged(
    voile_root: &Path,
    prefixes_root: &Path,
    base_dir: &Path,
    base_version: &str,
    tag: &str,
    files: &DownloadedFiles,
) -> Result<(), Failure> {
    apply_staged_inner(voile_root, prefixes_root, base_dir, base_version, tag, files, true)
}

fn apply_staged_inner(
    voile_root: &Path,
    prefixes_root: &Path,
    base_dir: &Path,
    base_version: &str,
    tag: &str,
    files: &DownloadedFiles,
    verify_codesign: bool,
) -> Result<(), Failure> {
    let staging_parent = tempfile::Builder::new()
        .prefix(UNFINISHED)
        .tempdir_in(voile_root)?;
    let staged = staging_parent.path().join("current");

    // Clone base Voile with APFS copy-on-write (instant, near-zero extra disk space).
    let status = std::process::Command::new("/bin/cp")
        .arg("-cRP")
        .arg(base_dir)
        .arg(&staged)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        // Fallback if filesystem does not support APFS clonefile (e.g. non-APFS volume).
        run_cmd("/bin/cp", &[Path::new("-RP"), base_dir, &staged])?;
    }

    install_macho(&files.wsd, &staged.join("linux/engine/wsd"), verify_codesign)?;
    if let Some(fex) = &files.fexdarwin_run {
        install_macho(fex, &staged.join("linux/engine/fexdarwin-run"), verify_codesign)?;
    }
    if let (Some(srv), Some(lib)) = (&files.virgl_render_server, &files.libvirglrenderer) {
        install_macho(
            srv,
            &staged.join("linux/graphics/libexec/virgl_render_server"),
            verify_codesign,
        )?;
        install_macho(
            lib,
            &staged.join("linux/graphics/lib/libvirglrenderer.1.dylib"),
            verify_codesign,
        )?;
    }
    if let Some(tarball) = &files.neptune_windows {
        let unpack = tempfile::Builder::new()
            .prefix(UNFINISHED)
            .tempdir_in(voile_root)?;
        let status = std::process::Command::new("/usr/bin/tar")
            .arg("-xzf")
            .arg(tarball)
            .arg("-C")
            .arg(unpack.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if !status.success() {
            return Err(Failure::Damaged);
        }
        let win_dst = staged.join("linux/windows");
        fs::create_dir_all(&win_dst)?;
        copy_dir_files(unpack.path(), &win_dst);
    }
    if let Some(tarball) = &files.gst_rtsp_root {
        let unpack = tempfile::Builder::new()
            .prefix(UNFINISHED)
            .tempdir_in(voile_root)?;
        let status = std::process::Command::new("/usr/bin/tar")
            .arg("-xzf")
            .arg(tarball)
            .arg("-C")
            .arg(unpack.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if !status.success() {
            return Err(Failure::Damaged);
        }
        let root_dst = staged.join("linux/root");
        run_cmd("/usr/bin/ditto", &[unpack.path(), &root_dst])?;
        // Bump mtimes on plugins that dlopen libsoup-3.0.so.0 so any cached GStreamer
        // registry re-probes them immediately.
        for plugin in ["libgstsoup.so", "libgstadaptivedemux2.so"] {
            let p = root_dst
                .join("usr/lib/x86_64-linux-gnu/gstreamer-1.0")
                .join(plugin);
            if p.is_file() {
                let _ = std::process::Command::new("/usr/bin/touch").arg(&p).status();
            }
        }
    }

    let mut unpacked_wine: Option<tempfile::TempDir> = None;
    if let Some(tarball) = &files.wine_media_clipboard {
        let unpack = tempfile::Builder::new()
            .prefix(UNFINISHED)
            .tempdir_in(voile_root)?;
        let status = std::process::Command::new("/usr/bin/tar")
            .arg("-xzf")
            .arg(tarball)
            .arg("-C")
            .arg(unpack.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if !status.success() {
            return Err(Failure::Damaged);
        }
        let wine_dst = staged.join("linux/wine");
        run_cmd("/usr/bin/ditto", &[unpack.path(), &wine_dst])?;
        // Bump wine.inf mtime so Wine automatically runs wineboot -u on existing prefixes.
        let wine_inf = wine_dst.join("share/wine/wine.inf");
        if wine_inf.is_file() {
            let _ = std::process::Command::new("/usr/bin/touch")
                .arg(&wine_inf)
                .status();
        }
        unpacked_wine = Some(unpack);
    }

    let stamp = OverlayStamp {
        tag: tag.to_owned(),
        base_voile: base_version.to_owned(),
    };
    fs::write(
        staged.join(STAMP_FILE),
        serde_json::to_vec_pretty(&stamp).map_err(|_| Failure::Storage)?,
    )?;

    // Swap staged directory into Voile/current cleanly.
    let current = voile_root.join(DEVELOPMENT);
    let old = if current.exists() {
        let old = staging_parent.path().join("old-current");
        fs::rename(&current, &old)?;
        Some(old)
    } else {
        None
    };
    if let Err(err) = fs::rename(&staged, &current) {
        if let Some(old) = old {
            let _ = fs::rename(old, &current);
        }
        return Err(err.into());
    }
    drop(staging_parent);

    // Sync updated DLLs directly into existing Linux prefixes so they take effect immediately.
    if let Some(unpack) = unpacked_wine {
        sync_linux_prefixes(prefixes_root, unpack.path());
    }
    Ok(())
}

fn sync_linux_prefixes(prefixes_root: &Path, wine_pack: &Path) {
    let Ok(entries) = fs::read_dir(prefixes_root) else {
        return;
    };
    let src64 = wine_pack.join("lib/wine/x86_64-windows");
    let src32 = wine_pack.join("lib/wine/i386-windows");
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with("-linux") {
            continue;
        }
        // Clear any stale GStreamer plugin registry cache so newly added libsoup/TLS/plugins are probed.
        let gst_cache = entry.path().join(".cache/gstreamer-1.0");
        if let Ok(cache_entries) = fs::read_dir(&gst_cache) {
            for cache_file in cache_entries.flatten() {
                let fname = cache_file.file_name();
                let s = fname.to_string_lossy();
                if s.starts_with("registry") && s.ends_with(".bin") {
                    let _ = fs::remove_file(cache_file.path());
                }
            }
        }
        let prefix = entry.path().join("prefix");
        let sys32 = prefix.join("drive_c/windows/system32");
        if sys32.is_dir() {
            copy_dir_files(&src64, &sys32);
        }
        let syswow64 = prefix.join("drive_c/windows/syswow64");
        if syswow64.is_dir() {
            copy_dir_files(&src32, &syswow64);
        }
    }
}

fn copy_dir_files(src: &Path, dst: &Path) {
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let _ = fs::copy(&path, dst.join(entry.file_name()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_base_voile(root: &Path, version: &str) -> PathBuf {
        let base = root.join(version);
        fs::create_dir_all(base.join("bin")).unwrap();
        fs::write(base.join("bin/wine"), "wine").unwrap();
        fs::create_dir_all(base.join("linux/engine")).unwrap();
        fs::write(base.join("linux/engine/wsd"), "stock-wsd").unwrap();
        fs::write(base.join("linux/engine/fexdarwin-run"), "stock-fex").unwrap();
        fs::create_dir_all(base.join("linux/wine/bin")).unwrap();
        fs::write(base.join("linux/wine/bin/wine"), "wine").unwrap();
        fs::create_dir_all(base.join("linux/wine/lib/wine/x86_64-windows")).unwrap();
        fs::create_dir_all(base.join("linux/wine/share/wine")).unwrap();
        fs::write(base.join("linux/wine/share/wine/wine.inf"), "orig").unwrap();
        fs::create_dir_all(base.join("linux/graphics/libexec")).unwrap();
        fs::create_dir_all(base.join("linux/graphics/lib")).unwrap();
        fs::create_dir_all(base.join("linux/root/usr/lib/x86_64-linux-gnu")).unwrap();
        fs::write(
            base.join("linux/root/usr/lib/x86_64-linux-gnu/libavcodec.so.61"),
            "ffmpeg",
        )
        .unwrap();
        fs::write(
            base.join("runtime.json"),
            format!(
                r#"{{"version":"{version}","contract":1,"linux":{{"neptune":"265be8af1786"}}}}"#
            ),
        )
        .unwrap();
        base
    }

    #[test]
    fn built_in_release_has_valid_urls_and_checksums() {
        let rel = built_in();
        assert!(is_valid_release(&rel));
        assert_eq!(rel.tag, "v0.1.6-metalfx");
        assert!(rel.neptune_windows.is_some());
        assert!(rel.gst_rtsp_root.is_some());
    }

    #[test]
    fn needs_install_respects_manual_dev_runtime_and_detects_updates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Voile");
        fake_base_voile(&root, "v26.3.0-26");

        // No current yet -> needs install.
        assert!(needs_install(&root));

        // Manual current without stamp -> leave untouched.
        fs::create_dir_all(root.join("current/bin")).unwrap();
        fs::write(root.join("current/bin/wine"), "dev-wine").unwrap();
        assert!(!needs_install(&root));
        assert!(!is_managed(&root));

        // Managed current with old tag -> needs update.
        fs::create_dir_all(root.join("current/linux/engine")).unwrap();
        fs::write(root.join("current/linux/engine/wsd"), "old-wsd").unwrap();
        fs::write(
            root.join("current").join(STAMP_FILE),
            r#"{"tag":"v0.1.2-metalfx","base_voile":"v26.3.0-26"}"#,
        )
        .unwrap();
        assert!(is_managed(&root));
        assert!(needs_install(&root));

        // Managed current matching wanted tag & base_voile -> up to date.
        fs::write(
            root.join("current").join(STAMP_FILE),
            format!(
                r#"{{"tag":"{}","base_voile":"v26.3.0-26"}}"#,
                built_in().tag
            ),
        )
        .unwrap();
        assert!(!needs_install(&root));

        // Newer upstream base Voile installed -> needs re-apply on top of newer base.
        fake_base_voile(&root, "v26.3.0-27");
        assert!(needs_install(&root));
    }

    #[test]
    fn apply_staged_clones_overlays_and_syncs_existing_linux_prefixes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Voile");
        let prefixes = tmp.path().join("Prefixes");
        let base = fake_base_voile(&root, "v26.3.0-26");

        let sys32 = prefixes.join("steam-438100-windows-linux/prefix/drive_c/windows/system32");
        let syswow64 = prefixes.join("steam-438100-windows-linux/prefix/drive_c/windows/syswow64");
        fs::create_dir_all(&sys32).unwrap();
        fs::create_dir_all(&syswow64).unwrap();

        let dl = tmp.path().join("dl");
        fs::create_dir_all(&dl).unwrap();
        let wsd = dl.join("wsd");
        let fex = dl.join("fexdarwin-run");
        fs::write(&wsd, "new-wsd").unwrap();
        fs::write(&fex, "new-fex").unwrap();

        // Build a tiny fake wine-media-clipboard tarball.
        let pack_src = tmp.path().join("pack-src");
        fs::create_dir_all(pack_src.join("lib/wine/x86_64-windows")).unwrap();
        fs::create_dir_all(pack_src.join("lib/wine/i386-windows")).unwrap();
        fs::create_dir_all(pack_src.join("share/wine")).unwrap();
        fs::write(pack_src.join("lib/wine/x86_64-windows/winedmo.dll"), "dmo64").unwrap();
        fs::write(pack_src.join("lib/wine/i386-windows/winedmo.dll"), "dmo32").unwrap();
        fs::write(pack_src.join("share/wine/wine.inf"), "updated-inf").unwrap();
        let tarball = dl.join("wine-media-clipboard.tar.gz");
        let status = std::process::Command::new("/usr/bin/tar")
            .args(["-czf", tarball.to_str().unwrap(), "-C", pack_src.to_str().unwrap(), "."])
            .status()
            .unwrap();
        assert!(status.success());

        // Build a tiny fake neptune-windows tarball.
        let npt_src = tmp.path().join("npt-src");
        fs::create_dir_all(&npt_src).unwrap();
        fs::write(npt_src.join("d3d11.dll"), "neptune-d3d11").unwrap();
        let npt_tarball = dl.join("neptune-windows.tar.gz");
        let status = std::process::Command::new("/usr/bin/tar")
            .args([
                "-czf",
                npt_tarball.to_str().unwrap(),
                "-C",
                npt_src.to_str().unwrap(),
                "d3d11.dll",
            ])
            .status()
            .unwrap();
        assert!(status.success());

        let gst_cache = prefixes.join("steam-438100-windows-linux/.cache/gstreamer-1.0");
        fs::create_dir_all(&gst_cache).unwrap();
        let stale_reg = gst_cache.join("registry.x86_64.bin");
        fs::write(&stale_reg, "stale").unwrap();

        // Build a tiny fake gst-rtsp-root tarball.
        let gst_src = tmp.path().join("gst-src");
        fs::create_dir_all(gst_src.join("usr/lib/x86_64-linux-gnu/gio/modules")).unwrap();
        fs::write(
            gst_src.join("usr/lib/x86_64-linux-gnu/gio/modules/libgiognutls.so"),
            "tls",
        )
        .unwrap();
        let gst_tarball = dl.join("gst-rtsp-root.tar.gz");
        let status = std::process::Command::new("/usr/bin/tar")
            .args(["-czf", gst_tarball.to_str().unwrap(), "-C", gst_src.to_str().unwrap(), "."])
            .status()
            .unwrap();
        assert!(status.success());

        let files = DownloadedFiles {
            wsd,
            fexdarwin_run: Some(fex),
            virgl_render_server: None,
            libvirglrenderer: None,
            neptune_windows: Some(npt_tarball),
            gst_rtsp_root: Some(gst_tarball),
            wine_media_clipboard: Some(tarball),
        };
        apply_staged_inner(&root, &prefixes, &base, "v26.3.0-26", "v0.1.6-metalfx", &files, false)
            .unwrap();

        assert_eq!(
            fs::read_to_string(root.join("current/linux/engine/wsd")).unwrap(),
            "new-wsd"
        );
        assert_eq!(
            fs::read_to_string(root.join("current/linux/engine/fexdarwin-run")).unwrap(),
            "new-fex"
        );
        assert_eq!(
            fs::read_to_string(root.join("current/linux/windows/d3d11.dll")).unwrap(),
            "neptune-d3d11"
        );
        assert_eq!(
            fs::read_to_string(
                root.join("current/linux/root/usr/lib/x86_64-linux-gnu/gio/modules/libgiognutls.so")
            )
            .unwrap(),
            "tls"
        );
        assert!(!stale_reg.exists());
        assert_eq!(
            fs::read_to_string(root.join("current/linux/wine/lib/wine/x86_64-windows/winedmo.dll"))
                .unwrap(),
            "dmo64"
        );
        assert_eq!(fs::read_to_string(sys32.join("winedmo.dll")).unwrap(), "dmo64");
        assert_eq!(fs::read_to_string(syswow64.join("winedmo.dll")).unwrap(), "dmo32");
        assert!(!needs_install(&root));
    }
}
