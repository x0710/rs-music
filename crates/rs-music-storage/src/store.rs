use crate::{State, file_error::path_error};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Snapshot<T> {
    version: u32,
    state: T,
}

/// Holds the exclusive state-file lock for the lifetime of the open store.
pub struct Store {
    path: PathBuf,
    _lock: File,
}

impl Store {
    /// 获取独占锁并读取版本 1 的快照。文件缺失时使用默认状态；锁被占用、文件损坏
    /// 或快照版本不受支持时，返回错误，由调用方决定如何处理。
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

/// 在目标目录写入临时文件，序列化、刷新和同步成功后才替换目标文件。
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
    use crate::{CloseAction, OnlineTrack, QueueTrack};

    #[test]
    fn existing_local_queue_and_mixed_queue_keep_their_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            path: directory.path().join("state.json"),
            _lock: File::create(directory.path().join("state.lock")).unwrap(),
        };
        let mut old = serde_json::to_value(State::default()).unwrap();
        old.as_object_mut().unwrap().remove("zoom_factor");
        old.as_object_mut().unwrap().remove("cache_cleanup_minutes");
        old.as_object_mut().unwrap().remove("theme");
        old.as_object_mut().unwrap().remove("downloads");
        old.as_object_mut().unwrap().remove("close_action");
        old.as_object_mut().unwrap().remove("source_successes");
        old["queue"] = serde_json::json!([1, 0]);
        old["current"] = serde_json::json!(0);
        let mut state: State = serde_json::from_value(old).unwrap();
        assert_eq!(state.cache_cleanup_minutes, Some(5));
        assert_eq!(state.zoom_factor, 1.0);
        assert_eq!(state.theme, crate::ThemePreference::System);
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
        state.zoom_factor = 1.4;
        state.cache_cleanup_minutes = None;
        state.close_action = CloseAction::Exit;
        state.downloads.audio = false;
        state.downloads.directory = Some(directory.path().join("Music"));
        state.source_successes.insert("source-id".into(), 5);
        store.save(&state).unwrap();
        let saved: Snapshot<State> =
            serde_json::from_reader(File::open(store.path).unwrap()).unwrap();
        assert!(saved.state.queue == [QueueTrack::Local(1), entry.clone(), QueueTrack::Local(0)]);
        assert!(saved.state.current == Some(entry));
        assert_eq!(saved.state.zoom_factor, 1.4);
        assert_eq!(saved.state.cache_cleanup_minutes, None);
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
