mod audio_cache;
mod cache_cleanup;

pub use audio_cache::{AudioCache, CachedTrack, default_music_directory};
pub use cache_cleanup::clear_recent_cache;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, BufReader, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum LanguagePreference {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    #[default]
    Sequential,
    RepeatAll,
    RepeatOne,
    Shuffle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    #[default]
    HideToTray,
    Exit,
}

fn default_volume() -> u8 {
    100
}

fn default_auto_switch_source() -> bool {
    true
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSource {
    pub name: String,
    pub script: bool,
    pub contents: String,
    pub platform: Option<String>,
}

impl SavedSource {
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(self.contents.as_bytes()))
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OnlineTrack {
    pub platform: String,
    pub track: rs_music_core::Track,
    pub source: Option<String>,
}

impl PartialEq for OnlineTrack {
    fn eq(&self, other: &Self) -> bool {
        self.platform == other.platform
            && self.source == other.source
            && match (&self.track.id, &other.track.id) {
                (Some(left), Some(right)) => left == right,
                (None, None) => {
                    self.track.audio_url == other.track.audio_url
                        && self.track.title == other.track.title
                        && self.track.artist == other.track.artist
                }
                _ => false,
            }
    }
}

impl Eq for OnlineTrack {}

impl Hash for OnlineTrack {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.platform.hash(state);
        self.source.hash(state);
        self.track.id.hash(state);
        if self.track.id.is_none() {
            self.track.audio_url.hash(state);
            self.track.title.hash(state);
            self.track.artist.hash(state);
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QueueTrack {
    Local(usize),
    Online(Box<OnlineTrack>),
}

impl From<OnlineTrack> for QueueTrack {
    fn from(track: OnlineTrack) -> Self {
        Self::Online(Box::new(track))
    }
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceSelection {
    pub source: Option<usize>,
    pub catalog: Option<usize>,
    pub platform: String,
    pub quality: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    pub audio: bool,
    pub lyrics: bool,
    pub directory: Option<PathBuf>,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            audio: true,
            lyrics: true,
            directory: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct State {
    pub language: LanguagePreference,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub close_action: CloseAction,
    #[serde(default)]
    pub downloads: DownloadSettings,
    #[serde(with = "paths")]
    pub tracks: Vec<PathBuf>,
    pub queue: Vec<QueueTrack>,
    pub current: Option<QueueTrack>,
    #[serde(default)]
    pub playback_mode: PlaybackMode,
    #[serde(default = "default_volume")]
    pub volume: u8,
    #[serde(default)]
    pub sources: Vec<SavedSource>,
    #[serde(default)]
    pub source_selection: Option<SourceSelection>,
    #[serde(default = "default_auto_switch_source")]
    pub auto_switch_source: bool,
    #[serde(default)]
    pub source_successes: BTreeMap<String, u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            language: LanguagePreference::default(),
            font_family: None,
            close_action: CloseAction::default(),
            downloads: DownloadSettings::default(),
            tracks: Vec::new(),
            queue: Vec::new(),
            current: None,
            playback_mode: PlaybackMode::default(),
            volume: default_volume(),
            sources: Vec::new(),
            source_selection: None,
            auto_switch_source: default_auto_switch_source(),
            source_successes: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Snapshot<T> {
    version: u32,
    state: T,
}

pub struct Store {
    path: PathBuf,
    _lock: File,
}

impl Store {
    pub fn open() -> io::Result<(Self, State)> {
        let dirs = ProjectDirs::from("", "", "rs-music")
            .ok_or_else(|| io::Error::other("User data directory unavailable"))?;
        let directory = dirs.data_local_dir();
        fs::create_dir_all(directory)
            .map_err(|error| path_error("Create data directory", directory, error))?;
        let lock_path = directory.join("state.lock");
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| path_error("Open state lock file", &lock_path, error))?;
        lock.try_lock()
            .map_err(|error| path_error("Acquire state lock", &lock_path, error.into()))?;

        let path = directory.join("state.json");
        let state = match File::open(&path) {
            Ok(file) => {
                let snapshot: Snapshot<State> = serde_json::from_reader(BufReader::new(file))
                    .map_err(|error| path_error("Read state JSON", &path, error.into()))?;
                if snapshot.version != 1 {
                    return Err(io::Error::other(format!(
                        "Unsupported state format version {} in {} (expected 1)",
                        snapshot.version,
                        path.display()
                    )));
                }
                snapshot.state
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => State::default(),
            Err(error) => return Err(path_error("Open state file", &path, error)),
        };
        Ok((Self { path, _lock: lock }, state))
    }

    pub fn save(&self, state: &State) -> io::Result<()> {
        save_json(&self.path, &Snapshot { version: 1, state })
    }

    pub fn load_search_history(&self) -> io::Result<Vec<String>> {
        let path = self.path.with_file_name("search-history.json");
        match File::open(&path) {
            Ok(file) => serde_json::from_reader(BufReader::new(file))
                .map_err(|error| path_error("Read search history JSON", &path, error.into())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(path_error("Open search history file", &path, error)),
        }
    }

    pub fn save_search_history(&self, history: &[String]) -> io::Result<()> {
        save_json(&self.path.with_file_name("search-history.json"), history)
    }
}

#[derive(Debug)]
struct FileError {
    operation: &'static str,
    path: PathBuf,
    source: io::Error,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.operation, self.path.display())
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn path_error(operation: &'static str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        FileError {
            operation,
            path: path.to_owned(),
            source: error,
        },
    )
}

fn save_json(path: &Path, value: &(impl Serialize + ?Sized)) -> io::Result<()> {
    let directory = path.parent().expect("data file has a parent");
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| path_error("Create temporary file for", path, error))?;
    serde_json::to_writer(&mut file, value)
        .map_err(|error| path_error("Write JSON for", path, error.into()))?;
    file.flush()
        .map_err(|error| path_error("Flush temporary file for", path, error))?;
    file.as_file()
        .sync_all()
        .map_err(|error| path_error("Sync temporary file for", path, error))?;
    file.persist(path)
        .map_err(|error| path_error("Replace file", path, error.into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_local_queue_and_mixed_queue_keep_their_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            path: directory.path().join("state.json"),
            _lock: File::create(directory.path().join("state.lock")).unwrap(),
        };
        let mut old = serde_json::to_value(State::default()).unwrap();
        old.as_object_mut().unwrap().remove("downloads");
        old.as_object_mut().unwrap().remove("close_action");
        old.as_object_mut().unwrap().remove("source_successes");
        old["queue"] = serde_json::json!([1, 0]);
        old["current"] = serde_json::json!(0);
        let mut state: State = serde_json::from_value(old).unwrap();
        assert_eq!(state.close_action, CloseAction::HideToTray);
        assert!(state.source_successes.is_empty());
        assert!(
            state.downloads.audio && state.downloads.lyrics && state.downloads.directory.is_none()
        );
        assert!(matches!(
            state.queue.as_slice(),
            [QueueTrack::Local(1), QueueTrack::Local(0)]
        ));
        assert!(matches!(state.current, Some(QueueTrack::Local(0))));

        let mut track = rs_music_core::Track::default();
        track.id = Some("123".into());
        track.title = "Online song".into();
        let entry = QueueTrack::from(OnlineTrack {
            platform: "wy".into(),
            track,
            source: None,
        });
        state.queue.insert(1, entry.clone());
        state.current = Some(entry.clone());
        state.close_action = CloseAction::Exit;
        state.downloads.audio = false;
        state.downloads.directory = Some(directory.path().join("Music"));
        state.source_successes.insert("source-id".into(), 5);
        store.save(&state).unwrap();
        let saved: Snapshot<State> =
            serde_json::from_reader(File::open(store.path).unwrap()).unwrap();
        assert!(saved.state.queue == [QueueTrack::Local(1), entry.clone(), QueueTrack::Local(0)]);
        assert!(saved.state.current == Some(entry));
        assert_eq!(saved.state.close_action, CloseAction::Exit);
        assert!(saved.state.downloads == state.downloads);
        assert_eq!(saved.state.source_successes, state.source_successes);
    }

    #[test]
    fn search_history_persists_separately_from_state() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            path: directory.path().join("state.json"),
            _lock: File::create(directory.path().join("state.lock")).unwrap(),
        };
        store.save(&State::default()).unwrap();
        let config = fs::read(&store.path).unwrap();
        assert!(store.load_search_history().unwrap().is_empty());

        let queries = vec!["周杰伦".to_owned(), "Daft Punk".to_owned()];
        store.save_search_history(&queries).unwrap();
        assert_eq!(store.load_search_history().unwrap(), queries);
        store.save_search_history(&[]).unwrap();
        assert!(store.load_search_history().unwrap().is_empty());
        assert_eq!(fs::read(&store.path).unwrap(), config);
    }
}

mod paths {
    use super::*;

    #[cfg(windows)]
    type PathUnit = u16;
    #[cfg(not(windows))]
    type PathUnit = u8;

    pub(super) fn serialize<S: serde::Serializer>(
        paths: &[PathBuf],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        #[cfg(windows)]
        use std::os::windows::ffi::OsStrExt;

        let paths: Vec<Vec<PathUnit>> = paths
            .iter()
            .map(|path| {
                #[cfg(windows)]
                {
                    path.as_os_str().encode_wide().collect()
                }
                #[cfg(not(windows))]
                {
                    path.as_os_str().as_encoded_bytes().to_vec()
                }
            })
            .collect();
        paths.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PathBuf>, D::Error> {
        let paths = Vec::<Vec<PathUnit>>::deserialize(deserializer)?;
        paths
            .into_iter()
            .map(|units| {
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStringExt;
                    Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)))
                }
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    Ok(PathBuf::from(std::ffi::OsString::from_vec(units)))
                }
                #[cfg(not(any(unix, windows)))]
                {
                    String::from_utf8(units)
                        .map(PathBuf::from)
                        .map_err(serde::de::Error::custom)
                }
            })
            .collect()
    }
}
