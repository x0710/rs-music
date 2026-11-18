use directories::ProjectDirs;
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};

const ENTRY_LIMIT: u64 = 256 * 1024;

pub(super) struct InitCache {
    directory: PathBuf,
    pub used: Cell<bool>,
    pub refresh: Cell<bool>,
}

#[derive(Deserialize, Serialize)]
struct Entry<T> {
    response: T,
}

impl InitCache {
    pub fn new(script_info: &str) -> Option<Self> {
        let dirs = ProjectDirs::from("", "", "rs-music")?;
        let script = hex::encode(Sha256::digest(script_info));
        Some(Self {
            directory: dirs.cache_dir().join("lx-init-v1").join(script),
            used: Cell::new(false),
            refresh: Cell::new(false),
        })
    }

    pub fn key(&self, method: &Method, url: &str, options: &Value) -> Option<PathBuf> {
        if !method.as_str().eq_ignore_ascii_case("GET")
            || ["body", "form", "formData"]
                .iter()
                .any(|key| !options[*key].is_null())
        {
            return None;
        }
        let mut headers = BTreeMap::new();
        if let Some(fields) = options["headers"].as_object() {
            for (key, value) in fields {
                let name = key.to_ascii_lowercase();
                if ["authorization", "cookie", "token", "key"]
                    .iter()
                    .any(|part| name.contains(part))
                    || matches!(name.as_str(), "cache-control" | "pragma" | "range")
                    || name.starts_with("if-")
                {
                    return None;
                }
                if headers.insert(name, value.as_str()?).is_some() {
                    return None;
                }
            }
        }
        let url = url.trim();
        let normalized;
        let target = if url.starts_with("//") {
            normalized = format!("https:{url}");
            normalized.as_str()
        } else {
            url
        };
        let parsed = Url::parse(target).ok()?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query_pairs().any(|(key, _)| {
                let key = key.to_ascii_lowercase();
                ["token", "password", "api_key", "apikey", "session", "auth"]
                    .iter()
                    .any(|part| key.contains(part))
            })
        {
            return None;
        }
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(&(url, headers)).ok()?));
        Some(self.directory.join(format!("{digest}.json")))
    }

    pub fn read(&self, path: &Path) -> Option<Value> {
        if self.refresh.get() {
            return None;
        }
        let file = File::open(path).ok()?;
        if file.metadata().ok()?.len() > ENTRY_LIMIT {
            return None;
        }
        let entry: Entry<Value> =
            serde_json::from_reader(BufReader::new(file).take(ENTRY_LIMIT)).ok()?;
        if !cacheable(&entry.response) {
            return None;
        }
        self.used.set(true);
        Some(entry.response)
    }

    pub fn write(path: &Path, response: &Value) -> io::Result<()> {
        if !cacheable(response) {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&Entry { response })?;
        if bytes.len() as u64 > ENTRY_LIMIT {
            return Ok(());
        }
        let directory = path.parent().expect("cache entry has a parent");
        fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.persist(path)?;
        Ok(())
    }
}

fn cacheable(response: &Value) -> bool {
    let body = &response["body"];
    let headers = &response["headers"];
    if response["statusCode"].as_u64() != Some(200)
        || !(body.is_object() || body.is_array())
        || !headers["set-cookie"].is_null()
        || headers["vary"]
            .as_str()
            .is_some_and(|vary| vary.trim() == "*")
        || body["success"].as_bool() == Some(false)
        || body["status"].as_str().is_some_and(|status| {
            matches!(
                status.to_ascii_lowercase().as_str(),
                "error" | "fail" | "failed"
            )
        })
        || (!body["error"].is_null()
            && body["error"] != false
            && body["error"] != ""
            && body["error"] != 0)
    {
        return false;
    }
    for field in ["code", "status", "error_code", "errcode"] {
        let value = &body[field];
        let code = value.as_i64().or_else(|| value.as_str()?.parse().ok());
        if code.is_some_and(|code| !matches!(code, 0 | 1 | 200)) {
            return false;
        }
    }
    !headers["cache-control"].as_str().is_some_and(|control| {
        control.split(',').any(|directive| {
            let (name, value) = directive
                .trim()
                .split_once('=')
                .unwrap_or((directive.trim(), ""));
            match name.trim().to_ascii_lowercase().as_str() {
                "no-store" | "no-cache" | "private" => true,
                "max-age" => value.trim().trim_matches('"').parse::<u64>() == Ok(0),
                _ => false,
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reuses_initialization_responses_without_expiration() {
        let directory = tempfile::tempdir().unwrap();
        let cache = InitCache {
            directory: directory.path().to_owned(),
            used: Cell::new(false),
            refresh: Cell::new(false),
        };
        let url = "https://example.org/config";
        let headers = json!({"headers": {"User-Agent": "test"}});
        let path = cache.key(&Method::GET, url, &headers).unwrap();
        assert_eq!(
            cache.key(
                &Method::GET,
                url,
                &json!({"headers": {"user-agent": "test"}, "timeout": 100})
            ),
            Some(path.clone())
        );
        assert!(cache.key(&Method::POST, url, &headers).is_none());
        assert!(
            cache
                .key(
                    &Method::GET,
                    url,
                    &json!({"headers":{"Authorization": "test"}})
                )
                .is_none()
        );
        assert!(
            cache
                .key(
                    &Method::GET,
                    "https://example.org/config?token=test",
                    &headers
                )
                .is_none()
        );

        let response = json!({"statusCode": 200, "headers": {}, "body":{"code":0,"config":true}});
        InitCache::write(&path, &response).unwrap();
        assert_eq!(cache.read(&path), Some(response.clone()));
        assert!(cache.used.get());
        cache.refresh.set(true);
        assert!(cache.read(&path).is_none());
        cache.refresh.set(false);
        fs::write(
            &path,
            serde_json::to_vec(&json!({"expires": 0, "response": response})).unwrap(),
        )
        .unwrap();
        assert_eq!(cache.read(&path), Some(response));
        let aged = json!({
            "statusCode": 200,
            "headers": {"cache-control": "max-age=1", "age": "3600"},
            "body": {"code": 0}
        });
        InitCache::write(&path, &aged).unwrap();
        assert_eq!(cache.read(&path), Some(aged));
        let denied =
            json!({"statusCode": 200, "headers": {"cache-control": "no-store"}, "body":{"code":0}});
        let denied_path = directory.path().join("denied");
        InitCache::write(&denied_path, &denied).unwrap();
        assert!(!denied_path.exists());
    }
}
