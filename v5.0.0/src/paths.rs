// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Where case files live, `~` expansion, and private, atomically replaced files.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

/// The user's home folder: `USERPROFILE` (or `HOMEDRIVE` plus `HOMEPATH`) on Windows,
/// `HOME` elsewhere, as Python's `Path.home()` looks for it.
pub fn home_dir() -> Option<PathBuf> {
    let set = |name: &str| env::var_os(name).filter(|value| !value.is_empty());
    if cfg!(windows) {
        if let Some(profile) = set("USERPROFILE") {
            return Some(PathBuf::from(profile));
        }
        let drive = set("HOMEDRIVE")?;
        let path = set("HOMEPATH")?;
        let mut joined = drive;
        joined.push(path);
        Some(PathBuf::from(joined))
    } else {
        set("HOME").map(PathBuf::from)
    }
}

/// The default case file: `%LOCALAPPDATA%\osintropy\cases.db` on Windows,
/// `${XDG_DATA_HOME:-~/.local/share}/osintropy/cases.db` elsewhere.
pub fn default_db() -> Option<PathBuf> {
    let set = |name: &str| env::var_os(name).filter(|value| !value.is_empty());
    let root = if cfg!(windows) {
        match set("LOCALAPPDATA") {
            Some(local) => PathBuf::from(local),
            None => home_dir()?.join("AppData").join("Local"),
        }
    } else {
        match set("XDG_DATA_HOME") {
            Some(data) => PathBuf::from(data),
            None => home_dir()?.join(".local").join("share"),
        }
    };
    Some(root.join("osintropy").join("cases.db"))
}

/// Python's `Path.expanduser()` for `~` and `~/...` (`~\...` on Windows).
pub fn expand_user(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = text.strip_prefix('~') else {
        return path.to_path_buf();
    };
    let separator =
        rest.is_empty() || rest.starts_with('/') || (cfg!(windows) && rest.starts_with('\\'));
    if !separator {
        return path.to_path_buf();
    }
    match home_dir() {
        Some(home) => {
            let tail = rest.trim_start_matches(['/', '\\']);
            if tail.is_empty() {
                home
            } else {
                home.join(tail)
            }
        }
        None => path.to_path_buf(),
    }
}

/// The folder a file sits in; `.` for a bare file name.
pub fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Make a path's folder if it is missing.
pub fn ensure_parent(path: &Path) -> io::Result<()> {
    fs::create_dir_all(parent_dir(path))
}

/// Python's `Path.resolve()`: absolute, with links followed where the path exists.
pub fn resolve(path: &Path) -> PathBuf {
    if let Ok(found) = fs::canonicalize(path) {
        return found;
    }
    if let (Some(name), parent) = (path.file_name(), parent_dir(path)) {
        if let Ok(folder) = fs::canonicalize(parent) {
            return folder.join(name);
        }
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Create a new file only this user can read (mode 0600 on Unix; on Windows the folder's
/// permissions apply). An existing file is left alone.
pub fn create_private(path: &Path) -> io::Result<()> {
    match private_options().open(path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err),
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// A new private file with a random name in `dir`.
pub fn create_temp(dir: &Path, prefix: &str, suffix: &str) -> io::Result<(PathBuf, File)> {
    loop {
        let path = dir.join(format!("{prefix}{}{suffix}", Uuid::new_v4().simple()));
        match private_options().open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

/// Write `content` to `target` atomically: a private temporary file in the same folder,
/// flushed to disk, then renamed over the target. Returns the path written.
pub fn save(target: &Path, content: &str) -> io::Result<PathBuf> {
    let target = expand_user(target);
    ensure_parent(&target)?;
    let (temp, mut file) = create_temp(&parent_dir(&target), ".osintropy-", "")?;
    let written = file
        .write_all(content.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| fs::rename(&temp, &target));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map(|()| target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_only_the_current_users_home() {
        let plain = Path::new("cases.db");
        assert_eq!(expand_user(plain), plain);
        assert_eq!(expand_user(Path::new("~bob/x")), Path::new("~bob/x"));
        if let Some(home) = home_dir() {
            assert_eq!(expand_user(Path::new("~")), home);
            assert_eq!(expand_user(Path::new("~/a/b.db")), home.join("a/b.db"));
        }
        assert_eq!(parent_dir(Path::new("x.db")), Path::new("."));
    }

    #[test]
    fn save_replaces_atomically_and_privately() {
        let dir = std::env::temp_dir().join(format!("osintropy-save-{}", Uuid::new_v4().simple()));
        let target = dir.join("nested").join("out.txt");
        save(&target, "one\n").unwrap();
        save(&target, "two\n").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "two\n");
        let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
