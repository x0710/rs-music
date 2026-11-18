use crate::path_error;
use directories::{ProjectDirs, UserDirs};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata},
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const CACHE_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

pub fn default_music_directory() -> Option<PathBuf> {
    UserDirs::new().map(|user| {
        user.audio_dir()
            .map_or_else(|| user.home_dir().join("Music"), Path::to_path_buf)
    })
}

#[derive(Clone)]
pub struct AudioCache {
    cache_dir: PathBuf,
    music_dir: Option<PathBuf>,
}

impl AudioCache {
    pub fn system() -> Option<Self> {
        let dirs = ProjectDirs::from("", "", "rs-music")?;
        Some(Self {
            cache_dir: dirs.cache_dir().join("audio"),
            music_dir: default_music_directory(),
        })
    }

    pub fn prune_expired(&self) -> io::Result<()> {
        let entries = match fs::read_dir(&self.cache_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(path_error(
                    "Open audio cache directory",
                    &self.cache_dir,
                    error,
                ));
            }
        };
        for entry in entries {
            let entry = entry
                .map_err(|error| path_error("Read audio cache entry", &self.cache_dir, error))?;
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|extension| extension != "audio")
                || path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_none_or(|stem| {
                        stem.len() != 64 || !stem.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(path_error("Read audio cache metadata", &path, error)),
            };
            if metadata.is_file()
                && expired(&metadata)
                    .map_err(|error| path_error("Read audio cache timestamp", &path, error))?
            {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(path_error("Remove expired audio cache", &path, error));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn track(&self, key: &[&str], title: String) -> CachedTrack {
        let mut hash = Sha256::new();
        for part in key {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        let id = format!("{:x}", hash.finalize());
        CachedTrack {
            cache_path: self.cache_dir.join(format!("{id}.audio")),
            music_dir: self.music_dir.clone(),
            default_dir: self.music_dir.clone(),
            id,
            title,
        }
    }
}

#[derive(Clone)]
pub struct CachedTrack {
    cache_path: PathBuf,
    music_dir: Option<PathBuf>,
    default_dir: Option<PathBuf>,
    id: String,
    title: String,
}

impl CachedTrack {
    pub fn in_music_directory(mut self, directory: Option<&Path>) -> Self {
        if let Some(directory) = directory {
            self.music_dir = Some(directory.to_owned());
        }
        self
    }

    pub fn open(&self) -> io::Result<File> {
        match self.open_cache() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let legacy = self.default_dir.as_ref().map(|dir| dir.join("rs-music"));
                for directory in [
                    self.music_dir.as_deref(),
                    self.default_dir
                        .as_deref()
                        .filter(|dir| Some(*dir) != self.music_dir.as_deref()),
                    legacy.as_deref(),
                ]
                .into_iter()
                .flatten()
                {
                    if let Some(path) = self.archived_path(directory)? {
                        return File::open(&path)
                            .map_err(|error| path_error("Open saved audio", &path, error));
                    }
                }
                Err(error)
            }
            Err(error) => Err(path_error("Open cached audio", &self.cache_path, error)),
            Ok(file) => Ok(file),
        }
    }

    fn open_cache(&self) -> io::Result<File> {
        let file = File::open(&self.cache_path)?;
        if expired(&file.metadata()?)? {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        Ok(file)
    }

    pub fn cache(&self, audio: &[u8]) -> io::Result<()> {
        temporary_output(&self.cache_path, audio)?
            .persist(&self.cache_path)
            .map_err(|error| path_error("Save cached audio", &self.cache_path, error.into()))?;
        Ok(())
    }

    pub fn archive(&self, audio: &[u8]) -> io::Result<PathBuf> {
        let directory = self.music_directory()?;
        if let Some(path) = self.archived_path(directory)? {
            return Ok(path);
        }
        let extension = audio_extension(audio);
        save_new(
            self.output_path(directory, extension),
            audio,
            "Save completed audio",
        )
    }

    pub fn lyrics_path(&self) -> io::Result<PathBuf> {
        let directory = self.music_directory()?;
        Ok(match self.archived_path(directory)? {
            Some(path) => path.with_extension("lrc"),
            None => self.output_path(directory, "lrc"),
        })
    }

    pub fn archive_lyrics(&self, text: &str) -> io::Result<PathBuf> {
        save_new(self.lyrics_path()?, text.as_bytes(), "Save lyrics")
    }

    fn output_path(&self, directory: &Path, extension: &str) -> PathBuf {
        let name = filename(&self.title);
        directory.join(format!("{name} [{}].{extension}", &self.id[..16]))
    }

    fn music_directory(&self) -> io::Result<&Path> {
        self.music_dir.as_deref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "User music directory unavailable")
        })
    }

    fn archived_path(&self, directory: &Path) -> io::Result<Option<PathBuf>> {
        let files = match fs::read_dir(directory) {
            Ok(files) => files,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(path_error("Open music directory", directory, error)),
        };
        let suffix = format!(" [{}]", &self.id[..16]);
        for file in files {
            let file =
                file.map_err(|error| path_error("Read music directory entry", directory, error))?;
            let path = file.path();
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("lrc"))
            {
                continue;
            }
            if path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
                && file
                    .file_type()
                    .map_err(|error| path_error("Read saved audio file type", &path, error))?
                    .is_file()
            {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }
}

fn expired(metadata: &Metadata) -> io::Result<bool> {
    Ok(metadata
        .modified()?
        .elapsed()
        .is_ok_and(|age| age >= CACHE_MAX_AGE))
}

fn save_new(path: PathBuf, bytes: &[u8], operation: &'static str) -> io::Result<PathBuf> {
    if path.is_file() {
        return Ok(path);
    }
    match temporary_output(&path, bytes)?.persist_noclobber(&path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists && path.is_file() => {}
        Err(error) => return Err(path_error(operation, &path, error.into())),
    }
    Ok(path)
}

fn temporary_output(path: &Path, bytes: &[u8]) -> io::Result<tempfile::NamedTempFile> {
    let directory = path.parent().expect("download file has a parent");
    fs::create_dir_all(directory)
        .map_err(|error| path_error("Create output directory", directory, error))?;
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| path_error("Create temporary output file for", path, error))?;
    file.write_all(bytes)
        .map_err(|error| path_error("Write temporary output for", path, error))?;
    file.as_file()
        .sync_all()
        .map_err(|error| path_error("Sync temporary output for", path, error))?;
    Ok(file)
}

fn filename(title: &str) -> String {
    let mut name = String::new();
    for ch in title.chars() {
        if name.len() + ch.len_utf8() > 180 {
            break;
        }
        name.push(if ch.is_control() || r#"<>:"/\|?*"#.contains(ch) {
            '_'
        } else {
            ch
        });
    }
    let name = name.trim_matches([' ', '.']);
    if name.is_empty() { "Track" } else { name }.to_owned()
}

fn audio_extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"fLaC") {
        "flac"
    } else if bytes.starts_with(b"OggS") {
        "ogg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        "wav"
    } else if bytes.get(4..8) == Some(b"ftyp") {
        "m4a"
    } else if bytes.starts_with(b"ID3")
        || matches!(bytes, [0xff, second, ..] if second & 0xe0 == 0xe0)
    {
        "mp3"
    } else {
        "audio"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn expired_cache_is_bypassed_and_pruned_without_removing_saved_audio() {
        let temp = tempfile::tempdir().unwrap();
        let cache = AudioCache {
            cache_dir: temp.path().join("cache"),
            music_dir: Some(temp.path().join("Music")),
        };
        let track = cache.track(&["wy", "expired"], "Old track".into());
        let fresh = cache.track(&["wy", "fresh"], "Fresh track".into());
        track.cache(b"stale audio").unwrap();
        fresh.cache(b"fresh audio").unwrap();
        File::options()
            .write(true)
            .open(&track.cache_path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - CACHE_MAX_AGE - Duration::from_secs(60))
            .unwrap();

        assert_eq!(track.open().unwrap_err().kind(), io::ErrorKind::NotFound);
        let archived = track.archive(b"RIFF....WAVE").unwrap();
        let mut data = Vec::new();
        track.open().unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"RIFF....WAVE");

        cache.prune_expired().unwrap();
        assert!(!track.cache_path.exists());
        assert!(fresh.cache_path.is_file());
        assert!(archived.is_file());
    }

    #[test]
    fn lyrics_can_finish_first_and_share_the_downloaded_track_name() {
        let temp = tempfile::tempdir().unwrap();
        let cache = AudioCache {
            cache_dir: temp.path().join("cache"),
            music_dir: Some(temp.path().join("Music")),
        };
        let directory = temp.path().join("custom");
        let track = cache
            .track(&["wy", "123"], "Song".into())
            .in_music_directory(Some(&directory));
        let text = "[00:01.00]Example";
        let lyrics = track.archive_lyrics(text).unwrap();
        assert_eq!(lyrics.parent(), Some(directory.as_path()));
        assert_eq!(fs::read_to_string(&lyrics).unwrap(), text);
        assert_eq!(track.open().unwrap_err().kind(), io::ErrorKind::NotFound);
        let audio = track.archive(b"RIFF....WAVE").unwrap();
        assert_eq!(audio.with_extension("lrc"), lyrics);
        assert_eq!(track.archive_lyrics("Do not overwrite").unwrap(), lyrics);
        assert_eq!(fs::read_to_string(lyrics).unwrap(), text);
    }

    #[test]
    fn caches_by_identity_and_archives_without_overwriting_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let cache = AudioCache {
            cache_dir: temp.path().join("cache"),
            music_dir: Some(temp.path().join("Music/rs-music")),
        };
        let first = cache.track(&["lx", "kg", "128k", "id"], "../歌/曲".into());
        let bytes = b"RIFF....WAVE";
        first.cache(bytes).unwrap();
        let same = cache.track(&["lx", "kg", "128k", "id"], "Other title".into());
        assert_eq!(fs::read(&same.cache_path).unwrap(), bytes);
        let another = cache.track(&["lx", "kg", "320k", "id"], "../歌/曲".into());
        assert_eq!(another.open().unwrap_err().kind(), io::ErrorKind::NotFound);

        let path = first.archive(bytes).unwrap();
        assert_eq!(path.parent(), cache.music_dir.as_deref());
        assert_eq!(path.extension().unwrap(), "wav");
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(same.archive(bytes).unwrap(), path);
        fs::remove_file(&first.cache_path).unwrap();
        let mut archived = Vec::new();
        std::io::Read::read_to_end(&mut same.open().unwrap(), &mut archived).unwrap();
        assert_eq!(archived, bytes);
        fs::write(&path, b"existing file").unwrap();
        assert_eq!(first.archive(bytes).unwrap(), path);
        assert_eq!(fs::read(&path).unwrap(), b"existing file");
    }
}
