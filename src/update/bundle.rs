//! Installing a macOS app bundle from a release zip.
//!
//! The code signature seals the whole bundle, so an update replaces all of
//! `Sonora.app`, never only its executable. The release is unpacked and checked
//! in a staging folder next to the installed bundle, on the same volume, then
//! the two are exchanged. Any failure before the exchange leaves the installed
//! app untouched.
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Staging folders start with this, so a later install can clear ones left by
/// a crash.
const STAGING_PREFIX: &str = ".sonora-update.";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const TRANSLOCATED: &str = "Move Sonora to Applications to enable updates";

type Step = Arc<dyn Fn(&Path, &Path) -> Result<()> + Send + Sync>;
type Check = Arc<dyn Fn(&Path) -> Result<()> + Send + Sync>;

/// The platform steps of a bundle install. Tests replace them to drive the
/// install on any platform.
#[derive(Clone)]
pub struct BundleTools {
    /// Unpacks the zip (first path) into an empty folder (second path).
    pub extract: Step,
    /// Rejects a staged bundle that macOS wouldn't run as Sonora.
    pub verify: Check,
    /// Exchanges the staged bundle (first path) with the installed one
    /// (second). Afterwards the previous bundle is inside the staged bundle's
    /// folder, which the install deletes.
    pub swap: Step,
}

impl BundleTools {
    /// `ditto`, `codesign` and `plutil`, and an atomic exchange of the bundles.
    #[cfg(target_os = "macos")]
    pub fn macos() -> Self {
        Self {
            extract: Arc::new(|zip, into| {
                run(
                    "/usr/bin/ditto",
                    &[
                        "-x".as_ref(),
                        "-k".as_ref(),
                        zip.as_os_str(),
                        into.as_os_str(),
                    ],
                )
            }),
            verify: Arc::new(verify_signed_bundle),
            swap: Arc::new(swap_dirs),
        }
    }
}

/// The bundle an executable belongs to, when this copy can replace it. `Err`
/// carries the reason to show, or `None` when the build simply can't update.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn locate(executable: &Path) -> Result<PathBuf, Option<String>> {
    let bundle = executable
        .parent()
        .filter(|dir| dir.file_name().is_some_and(|name| name == "MacOS"))
        .and_then(Path::parent)
        .filter(|dir| dir.file_name().is_some_and(|name| name == "Contents"))
        .and_then(Path::parent)
        .filter(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .filter(|_| executable.file_name().is_some_and(|name| name == "sonora"))
        .ok_or(None)?;
    // macOS runs an app opened from a quarantined download from a read-only
    // random path; replacing that copy would change nothing.
    if executable
        .components()
        .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return Err(Some(TRANSLOCATED.to_owned()));
    }
    let parent = bundle.parent().ok_or(None)?;
    if tempfile::tempfile_in(parent).is_err() {
        return Err(Some(format!(
            "Sonora can't update itself because it can't write to {}",
            parent.display()
        )));
    }
    Ok(bundle.to_owned())
}

/// Replaces `bundle` with the app in `zip`.
pub fn install(bundle: &Path, zip: &[u8], tools: &BundleTools) -> Result<()> {
    let parent = bundle.parent().context("The installed app has no folder")?;
    clear_stale(parent);
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(parent)
        .context("Couldn't write next to the installed app")?;
    let archive = staging.path().join("update.zip");
    std::fs::write(&archive, zip).context("Couldn't save the update")?;
    let unpacked = staging.path().join("unpacked");
    std::fs::create_dir(&unpacked)?;
    (tools.extract)(&archive, &unpacked).context("Couldn't unpack the update")?;
    let staged = single_app(&unpacked)?;
    ensure!(
        is_executable(&staged.join("Contents/MacOS/sonora")),
        "The update has no Sonora executable"
    );
    (tools.verify)(&staged).context("The update failed verification")?;
    (tools.swap)(&staged, bundle).context("Couldn't replace the installed app")?;
    // The staging folder now holds the previous app. Failing to delete it
    // doesn't undo the update, and the next install clears it.
    let _ = staging.close();
    Ok(())
}

/// Removes staging folders an interrupted install left behind.
fn clear_stale(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn single_app(dir: &Path) -> Result<PathBuf> {
    let mut apps = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "app") && path.is_dir());
    match (apps.next(), apps.next()) {
        (Some(app), None) => Ok(app),
        _ => bail!("The update doesn't contain exactly one app"),
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    metadata.is_file()
}

/// Exchanges two folders on the same volume. Uses an atomic exchange where the
/// file system supports one; otherwise moves `installed` aside, moves `staged`
/// in, and moves the previous folder back if that fails.
pub fn swap_dirs(staged: &Path, installed: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    match exchange(staged, installed) {
        Ok(()) => return Ok(()),
        Err(error) if !matches!(error.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL)) => {
            return Err(error.into());
        }
        Err(_) => {}
    }
    let aside = staged
        .parent()
        .context("The staged app has no folder")?
        .join(".previous.app");
    std::fs::rename(installed, &aside).context("Couldn't move the installed app aside")?;
    if let Err(error) = std::fs::rename(staged, installed) {
        std::fs::rename(&aside, installed).context("Couldn't restore the installed app")?;
        return Err(error.into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both pointers are NUL-terminated strings that outlive the call,
    // and renamex_np doesn't keep them.
    let result = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "app.sonora.player";

#[cfg(target_os = "macos")]
fn verify_signed_bundle(app: &Path) -> Result<()> {
    run(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            app.as_os_str(),
        ],
    )
    .context("Its code signature is invalid")?;
    let plist = app.join("Contents/Info.plist");
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .context("Couldn't read its Info.plist")?;
    ensure!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == BUNDLE_ID,
        "It isn't Sonora"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<()> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("Couldn't run {program}"))?;
    ensure!(
        output.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(root: &Path, folder: &str) -> PathBuf {
        let executable = root.join(folder).join("Sonora.app/Contents/MacOS/sonora");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"sonora").unwrap();
        executable
    }

    #[test]
    fn only_a_writable_bundle_outside_translocation_can_update() {
        let dir = tempfile::tempdir().unwrap();
        let installed = app(dir.path(), "Applications");
        assert_eq!(
            locate(&installed),
            Ok(dir.path().join("Applications/Sonora.app"))
        );

        let translocated = app(dir.path(), "AppTranslocation/1234/d");
        assert_eq!(locate(&translocated), Err(Some(TRANSLOCATED.to_owned())));

        // A bare binary, or one that isn't Sonora's executable, isn't a bundle.
        assert_eq!(locate(&dir.path().join("sonora")), Err(None));
        let other = dir
            .path()
            .join("Applications/Sonora.app/Contents/MacOS/helper");
        assert_eq!(locate(&other), Err(None));
    }

    #[cfg(unix)]
    #[test]
    fn a_bundle_in_a_read_only_folder_says_why_it_cannot_update() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = app(dir.path(), "locked");
        let locked = dir.path().join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = locate(&executable);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let reason = result.unwrap_err().expect("a reason");
        assert!(reason.contains("can't write to"), "{reason}");
    }

    #[test]
    fn swapping_moves_the_previous_bundle_into_the_staging_folder() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("Sonora.app");
        std::fs::create_dir(&installed).unwrap();
        std::fs::write(installed.join("version"), "old").unwrap();
        let staging = dir.path().join(".sonora-update.x");
        let staged = staging.join("Sonora.app");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("version"), "new").unwrap();

        swap_dirs(&staged, &installed).unwrap();
        assert_eq!(
            std::fs::read_to_string(installed.join("version")).unwrap(),
            "new"
        );
        let previous: Vec<_> = std::fs::read_dir(&staging)
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path().join("version")).unwrap())
            .collect();
        assert_eq!(previous, ["old"]);
    }

    #[test]
    fn a_failed_swap_leaves_the_installed_bundle_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("Sonora.app");
        std::fs::create_dir(&installed).unwrap();
        std::fs::write(installed.join("version"), "old").unwrap();
        let staged = dir.path().join(".sonora-update.x/Sonora.app");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        // The staged bundle is missing, so moving it in fails.
        assert!(swap_dirs(&staged, &installed).is_err());
        assert_eq!(
            std::fs::read_to_string(installed.join("version")).unwrap(),
            "old"
        );
    }
}
