use std::fs::{self, File, Metadata};
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub(crate) enum LockAttemptError {
    WouldBlock,
    Io(io::Error),
}

pub(crate) fn try_lock_exclusive(file: &File) -> Result<(), LockAttemptError> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(LockAttemptError::WouldBlock),
        Err(std::fs::TryLockError::Error(error)) => Err(LockAttemptError::Io(error)),
    }
}

pub(crate) fn is_link_or_reparse(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(crate) fn is_platform_root_alias(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(relative) = path.strip_prefix("/").ok() else {
            return false;
        };
        let expected = Path::new("/private").join(relative);
        return fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
            && fs::canonicalize(path).is_ok_and(|canonical| canonical == expected);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

#[derive(Debug)]
pub(crate) enum PathBoundaryError {
    BaseLink(PathBuf),
    BaseNotDirectory(PathBuf),
    Escape,
    ComponentLink(PathBuf),
    Inspection(PathBuf, io::Error),
}

pub(crate) fn validate_contained_path(base: &Path, path: &Path) -> Result<(), PathBoundaryError> {
    if !base.is_absolute() || !path.is_absolute() {
        return Err(PathBoundaryError::Escape);
    }

    for ancestor in base.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                if !is_platform_root_alias(ancestor) {
                    return Err(PathBoundaryError::BaseLink(ancestor.to_path_buf()));
                }
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(PathBoundaryError::BaseNotDirectory(ancestor.to_path_buf()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(PathBoundaryError::Inspection(ancestor.to_path_buf(), error));
            }
        }
    }

    let relative = path
        .strip_prefix(base)
        .map_err(|_| PathBoundaryError::Escape)?;
    let mut current = base.to_path_buf();
    for component in relative.components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(part) => current.push(part),
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(PathBoundaryError::Escape);
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                return Err(PathBoundaryError::ComponentLink(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(PathBoundaryError::Inspection(current, error)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::process::Command;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const LOCK_CHILD_ENV: &str = "APPSDK_PLATFORM_LOCK_CHILD";
    const LOCK_READY_ENV: &str = "APPSDK_PLATFORM_LOCK_READY";
    const LOCK_RELEASE_ENV: &str = "APPSDK_PLATFORM_LOCK_RELEASE";

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "appsdk-platform-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn try_lock_is_nonblocking_and_releases_on_close() {
        let root = temp_root("lock-release");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("lock");
        let first = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        try_lock_exclusive(&first).unwrap();

        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(matches!(
            try_lock_exclusive(&second),
            Err(LockAttemptError::WouldBlock)
        ));
        drop(first);
        try_lock_exclusive(&second).unwrap();

        drop(second);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn try_lock_child_process_holder() {
        let Ok(path) = std::env::var(LOCK_CHILD_ENV) else {
            return;
        };
        let ready = std::env::var(LOCK_READY_ENV).unwrap();
        let release = std::env::var(LOCK_RELEASE_ENV).unwrap();
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        try_lock_exclusive(&file).unwrap();
        fs::write(ready, b"ready").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while !Path::new(&release).exists() {
            assert!(Instant::now() < deadline, "lock child release timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn try_lock_conflicts_across_processes() {
        let root = temp_root("lock-process");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("lock");
        let ready = root.join("ready");
        let release = root.join("release");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::tests::try_lock_child_process_holder",
                "--nocapture",
            ])
            .env(LOCK_CHILD_ENV, &path)
            .env(LOCK_READY_ENV, &ready)
            .env(LOCK_RELEASE_ENV, &release)
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "lock child ready timed out");
            std::thread::sleep(Duration::from_millis(10));
        }

        let contender = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(matches!(
            try_lock_exclusive(&contender),
            Err(LockAttemptError::WouldBlock)
        ));
        fs::write(&release, b"release").unwrap();
        assert!(child.wait().unwrap().success());
        try_lock_exclusive(&contender).unwrap();

        drop(contender);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn contained_path_accepts_missing_descendants_and_rejects_escape() {
        let root = temp_root("contained");
        let base = root.join("base");
        fs::create_dir_all(&base).unwrap();

        validate_contained_path(&base, &base.join("missing").join("leaf")).unwrap();
        assert!(matches!(
            validate_contained_path(&base, &base.join("..").join("outside")),
            Err(PathBoundaryError::Escape)
        ));
        assert!(matches!(
            validate_contained_path(&base, Path::new("relative")),
            Err(PathBoundaryError::Escape)
        ));

        fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn contained_path_rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let root = temp_root("contained-symlink");
        let base = root.join("base");
        let outside = root.join("outside");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, base.join("link")).unwrap();

        assert!(matches!(
            validate_contained_path(&base, &base.join("link").join("leaf")),
            Err(PathBoundaryError::ComponentLink(_))
        ));

        fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn contained_path_rejects_symlinked_base_ancestor() {
        use std::os::unix::fs::symlink;

        let root = temp_root("contained-base-symlink");
        let real = root.join("real");
        let linked = root.join("linked");
        fs::create_dir_all(real.join("base")).unwrap();
        symlink(&real, &linked).unwrap();

        assert!(matches!(
            validate_contained_path(
                &linked.join("base"),
                &linked.join("base").join("leaf")
            ),
            Err(PathBoundaryError::BaseLink(path)) if path == linked
        ));

        fs::remove_dir_all(root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn contained_path_rejects_reparse_components() {
        use std::os::windows::fs::symlink_dir;

        let root = temp_root("contained-reparse");
        let base = root.join("base");
        let outside = root.join("outside");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink_dir(&outside, base.join("link")).unwrap();

        assert!(matches!(
            validate_contained_path(&base, &base.join("link").join("leaf")),
            Err(PathBoundaryError::ComponentLink(_))
        ));

        fs::remove_dir_all(root).ok();
    }
}
