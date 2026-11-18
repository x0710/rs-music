use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

pub const EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a", "aac", "mp4", "oga"];

pub fn scan(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut pending: Vec<_> = paths.into_iter().rev().collect();
    let mut visited = HashSet::new();
    let mut tracks = Vec::new();

    while let Some(path) = pending.pop() {
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                report_scan_failure(&path, "resolve path", &error);
                continue;
            }
        };
        if !visited.insert(path.clone()) {
            continue;
        }
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                let mut children = directory_children(&path);
                children.sort_unstable();
                pending.extend(children.into_iter().rev());
            }
            Ok(metadata) if metadata.is_file() && is_audio(&path) => tracks.push(path),
            Ok(_) => {}
            Err(error) => report_scan_failure(&path, "read metadata", &error),
        }
    }

    tracks
}

fn directory_children(path: &Path) -> Vec<PathBuf> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            report_scan_failure(path, "open directory", &error);
            return Vec::new();
        }
    };
    let mut children = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => match entry.file_type() {
                Ok(kind) if !kind.is_symlink() => children.push(entry.path()),
                Ok(_) => {}
                Err(error) => report_scan_failure(&entry.path(), "read file type", &error),
            },
            Err(error) => report_scan_failure(path, "read directory entry", &error),
        }
    }
    children
}

fn report_scan_failure(path: &Path, operation: &str, error: &std::io::Error) {
    let error = crate::error_details(error);
    tracing::warn!(path = %path.display(), operation, %error, "Could not scan music library");
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            EXTENSIONS
                .iter()
                .any(|supported| ext.eq_ignore_ascii_case(supported))
        })
}
