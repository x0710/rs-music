use crate::path_error;
use directories::ProjectDirs;
use std::{
    fs, io,
    path::Path,
    time::{Duration, SystemTime},
};

pub fn clear_recent_cache(range: Option<Duration>) -> io::Result<usize> {
    let dirs = ProjectDirs::from("", "", "rs-music")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Cache directory unavailable"))?;
    clear_cache_in(dirs.cache_dir(), range, SystemTime::now())
}

fn clear_cache_in(root: &Path, range: Option<Duration>, now: SystemTime) -> io::Result<usize> {
    let mut removed = clear_entries(&root.join("audio"), range, now, Some("audio"))?;
    removed += clear_entries(&root.join("discovery-v1"), range, now, None)?;
    let lx_cache = root.join("lx-init-v1");
    if let Some(scripts) = read_cache_directory(&lx_cache)? {
        for script in scripts {
            let directory = script
                .map_err(|error| path_error("Read LX cache entry", &lx_cache, error))?
                .path();
            if directory
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_digest)
            {
                removed += clear_entries(&directory, range, now, Some("json"))?;
            }
        }
    }
    Ok(removed)
}

fn read_cache_directory(path: &Path) -> io::Result<Option<fs::ReadDir>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::read_dir(path).map(Some),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
    .map_err(|error| path_error("Open cache directory", path, error))
}

fn clear_entries(
    directory: &Path,
    range: Option<Duration>,
    now: SystemTime,
    extension: Option<&str>,
) -> io::Result<usize> {
    let Some(entries) = read_cache_directory(directory)? else {
        return Ok(0);
    };
    let mut count = 0;
    for entry in entries {
        let entry = entry.map_err(|error| path_error("Read cache entry", directory, error))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != extension
            || !path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(is_digest)
        {
            continue;
        }
        // DirEntry metadata does not follow symlinks.
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(path_error("Read cache metadata", &path, error)),
        };
        if !metadata.is_file() {
            continue;
        }
        if let Some(range) = range {
            let modified = metadata
                .modified()
                .map_err(|error| path_error("Read cache timestamp", &path, error))?;
            if !now.duration_since(modified).is_ok_and(|age| age <= range) {
                continue;
            }
        }
        match fs::remove_file(&path) {
            Ok(()) => count += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(path_error("Remove cache entry", &path, error)),
        }
    }
    Ok(count)
}

fn is_digest(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_entries_written_within_the_selected_period() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let now = SystemTime::now();
        let audio = root.join("audio");
        let discovery = root.join("discovery-v1");
        let lx = root.join("lx-init-v1").join("a".repeat(64));
        let recent_audio = audio.join(format!("{}.audio", "a".repeat(64)));
        let old_audio = audio.join(format!("{}.audio", "b".repeat(64)));
        let boundary_discovery = discovery.join("c".repeat(64));
        let recent_lx = lx.join(format!("{}.json", "c".repeat(64)));
        let saved_song = root.join("Music").join(format!("{}.audio", "d".repeat(64)));
        let unknown = audio.join("unfinished.tmp");
        for (path, minutes_ago) in [
            (&recent_audio, 2),
            (&old_audio, 12),
            (&boundary_discovery, 5),
            (&recent_lx, 4),
            (&saved_song, 2),
            (&unknown, 2),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::File::create(path)
                .unwrap()
                .set_modified(now - Duration::from_secs(minutes_ago * 60))
                .unwrap();
        }
        assert_eq!(
            clear_cache_in(root, Some(Duration::from_secs(5 * 60)), now).unwrap(),
            3
        );
        assert!(!recent_audio.exists() && !boundary_discovery.exists() && !recent_lx.exists());
        assert!(old_audio.exists() && saved_song.exists() && unknown.exists());
        assert_eq!(clear_cache_in(root, None, now).unwrap(), 1);
        assert!(!old_audio.exists());
        assert!(saved_song.exists() && unknown.exists());
    }
}
