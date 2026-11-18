use crate::{Result, error_details};
use directories::ProjectDirs;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const FEED_TTL: Duration = Duration::from_secs(30 * 60);
pub const IMAGE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_ENTRY: u64 = 4 * 1024 * 1024;

pub struct DiscoveryData<T> {
    pub value: T,
    pub valid_until: Instant,
}

pub(crate) fn json<T: Serialize + DeserializeOwned>(
    key: &str,
    refresh: bool,
    fetch: impl FnOnce() -> Result<T>,
) -> Result<DiscoveryData<T>> {
    cached(
        key,
        FEED_TTL,
        refresh,
        |body| serde_json::from_slice(&body).ok(),
        serde_json::to_vec,
        fetch,
    )
}

pub(crate) fn image(
    key: &str,
    refresh: bool,
    fetch: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<DiscoveryData<Vec<u8>>> {
    cached(
        key,
        IMAGE_TTL,
        refresh,
        Some,
        |bytes| Ok::<_, io::Error>(bytes.clone()),
        || {
            let bytes = fetch()?;
            if !bytes.starts_with(&[0xff, 0xd8, 0xff]) && !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err("Unexpected playlist cover format (expected JPEG or PNG)".into());
            }
            Ok(bytes)
        },
    )
}

fn cached<T, E: std::error::Error + 'static>(
    key: &str,
    ttl: Duration,
    refresh: bool,
    decode: impl FnOnce(Vec<u8>) -> Option<T>,
    encode: impl FnOnce(&T) -> std::result::Result<Vec<u8>, E>,
    fetch: impl FnOnce() -> Result<T>,
) -> Result<DiscoveryData<T>> {
    let path = path(key);
    if !refresh
        && let Some(path) = &path
        && let Some((bytes, valid_for)) = read(path, ttl)
        && let Some(value) = decode(bytes)
    {
        return Ok(DiscoveryData {
            value,
            valid_until: Instant::now() + valid_for,
        });
    }
    let value = fetch()?;
    if let Some(path) = &path {
        match encode(&value) {
            Ok(bytes) => {
                if let Err(error) = write(path, &bytes) {
                    tracing::warn!(path = %path.display(), error = %error_details(&error), "Could not save music hall cache");
                }
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), error = %error_details(&error), "Could not encode music hall cache")
            }
        }
    }
    Ok(DiscoveryData {
        value,
        valid_until: Instant::now() + ttl,
    })
}

fn path(key: &str) -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "rs-music")?;
    Some(
        dirs.cache_dir()
            .join("discovery-v1")
            .join(hex::encode(Sha256::digest(key.as_bytes()))),
    )
}

fn read(path: &Path, ttl: Duration) -> Option<(Vec<u8>, Duration)> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if metadata.len() > MAX_ENTRY {
        return None;
    }
    let age = metadata.modified().ok()?.elapsed().ok()?;
    let remaining = ttl
        .checked_sub(age)
        .filter(|duration| !duration.is_zero())?;
    let mut bytes = Vec::new();
    file.take(MAX_ENTRY + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_ENTRY).then_some((bytes, remaining))
}

fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() as u64 > MAX_ENTRY {
        return Ok(());
    }
    let directory = path.parent().expect("cache entry has a parent");
    fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_reuses_fresh_files_and_expires_old_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cover");
        write(&path, b"image").unwrap();
        let (bytes, remaining) = read(&path, IMAGE_TTL).unwrap();
        assert_eq!(bytes, b"image");
        assert!(remaining <= IMAGE_TTL);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - IMAGE_TTL - Duration::from_secs(1))
            .unwrap();
        assert!(read(&path, IMAGE_TTL).is_none());
    }
}
