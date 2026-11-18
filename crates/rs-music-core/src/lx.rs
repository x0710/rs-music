use crate::lx_cache::InitCache;
use crate::{
    AudioStream, Result, Track,
    diagnostics::{ErrorContext, endpoint, response_details, sanitize},
    lx_client, lx_url, report_failure,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::read::ZlibDecoder;
use reqwest::{Method, Url};
use rquickjs::{CatchResultExt, Context, Ctx, Function, Promise, Runtime};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    hash::{BuildHasher, BuildHasherDefault, DefaultHasher},
    io::{self, Read},
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Capability {
    #[serde(rename = "type")]
    kind: String,
    actions: Vec<String>,
    #[serde(default)]
    qualitys: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LxExport {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    homepage: String,
    script: String,
}

type Reply<T> = mpsc::SyncSender<std::result::Result<T, String>>;

struct Request {
    event: String,
    reply: Reply<String>,
}

#[derive(Clone)]
pub struct LxSource {
    name: String,
    platforms: Arc<BTreeMap<String, Vec<String>>>,
    requests: mpsc::Sender<Request>,
}

impl LxSource {
    pub(crate) fn load(script: &str) -> Result<Self> {
        let script = script.to_owned();
        let mut metadata = json!({"rawScript": script});
        if let Some((header, _)) = script
            .trim_start()
            .strip_prefix("/*")
            .and_then(|s| s.split_once("*/"))
        {
            for line in header.trim_start_matches('!').lines() {
                if let Some((key, value)) = line
                    .trim()
                    .trim_start_matches('*')
                    .trim()
                    .strip_prefix('@')
                    .and_then(|s| s.split_once(char::is_whitespace))
                    && matches!(
                        key,
                        "name" | "description" | "author" | "version" | "homepage"
                    )
                {
                    metadata[key] = json!(value.trim());
                }
            }
        }
        Self::start(script, metadata)
    }

    pub(crate) fn load_export(document: &str) -> Result<Self> {
        let source: LxExport = serde_json::from_str(document)?;
        let script = match source.script.strip_prefix("gz_") {
            Some(encoded) => {
                let compressed = STANDARD.decode(encoded).map_err(|error| {
                    io::Error::other(format!("Invalid LX script encoding: {error}"))
                })?;
                const MAX_SCRIPT_BYTES: u64 = 2 * 1024 * 1024;
                let mut script = String::new();
                ZlibDecoder::new(compressed.as_slice())
                    .take(MAX_SCRIPT_BYTES + 1)
                    .read_to_string(&mut script)
                    .map_err(|error| {
                        io::Error::other(format!("Could not decompress LX script: {error}"))
                    })?;
                if script.len() as u64 > MAX_SCRIPT_BYTES {
                    return Err(io::Error::other("Decompressed LX script exceeds 2 MiB").into());
                }
                script
            }
            None => source.script,
        };
        let metadata = json!({
            "name": source.name,
            "description": source.description,
            "version": source.version,
            "author": source.author,
            "homepage": source.homepage,
            "rawScript": script,
        });
        Self::start(script, metadata)
    }

    fn start(script: String, metadata: Value) -> Result<Self> {
        let name = metadata["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Missing LX @name")?
            .to_owned();
        let (init, ready) = mpsc::sync_channel(1);
        let (requests, inbox) = mpsc::channel();
        std::thread::Builder::new()
            .name("lx-source".into())
            .spawn(move || {
                let script_info = metadata.to_string();
                let cache = InitCache::new(&script_info).map(Rc::new);
                let mut result = run(&script, &script_info, &init, &inbox, cache.clone());
                if result.is_err()
                    && let Some(cache) = &cache
                    && cache.used.get()
                {
                    cache.refresh.set(true);
                    result = run(&script, &script_info, &init, &inbox, Some(cache.clone()));
                }
                if let Err(err) = result {
                    let _ = init.send(Err(crate::public_error(err)));
                }
            })?;
        let platforms = ready.recv()??;
        Ok(Self {
            name,
            platforms: Arc::new(platforms),
            requests,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn platforms(&self) -> &BTreeMap<String, Vec<String>> {
        &self.platforms
    }

    pub fn music_url(&self, platform: &str, quality: &str, music_info: Value) -> Result<Url> {
        if !self
            .platforms
            .get(platform)
            .is_some_and(|qs| qs.iter().any(|q| q == quality))
        {
            return Err("Unsupported LX platform or quality".into());
        }
        let fields: Vec<_> = music_info
            .as_object()
            .into_iter()
            .flat_map(|fields| fields.keys())
            .collect();
        tracing::debug!(source = %self.name(), platform, quality, ?fields, "Resolving LX musicUrl");
        let event = json!({"source":platform,"action":"musicUrl","info":{"type":quality,"musicInfo":music_info}});
        let (reply, response) = mpsc::sync_channel(1);
        self.requests.send(Request {
            event: event.to_string(),
            reply,
        })?;
        let resolved = response.recv()?.map_err(|error| {
            io::Error::other(format!(
                "LX musicUrl rejected ({platform}, {quality}): {error}"
            ))
        })?;
        lx_url(&resolved, "LX playback URL")
    }

    pub fn fetch_by_id(&self, platform: &str, quality: &str, id: &str) -> Result<Vec<u8>> {
        self.stream_by_id(platform, quality, id)?.read_all()
    }

    pub fn stream_by_id(&self, platform: &str, quality: &str, id: &str) -> Result<AudioStream> {
        let mut info = json!({"source":platform, "songmid":id});
        match platform {
            "kg" => info["hash"] = json!(id),
            "mg" => info["copyrightId"] = json!(id),
            _ => {}
        }
        report_failure(
            self.name(),
            "LX playback",
            self.fetch_with_info(platform, quality, info),
        )
    }

    pub fn fetch_track(&self, platform: &str, quality: &str, track: &Track) -> Result<Vec<u8>> {
        self.stream_track(platform, quality, track)?.read_all()
    }

    pub fn stream_track(
        &self,
        platform: &str,
        quality: &str,
        track: &Track,
    ) -> Result<AudioStream> {
        let id = track
            .id
            .as_deref()
            .ok_or("Search result has no platform track ID")?;
        let mut info = json!({
            "source": platform,
            "songmid": id,
            "name": track.title,
            "singer": track.artist,
        });
        match platform {
            "kg" => info["hash"] = json!(id),
            "mg" => info["copyrightId"] = json!(id),
            _ => {}
        }
        if let (Some(fields), Some(metadata)) = (
            info.as_object_mut(),
            track.lx_metadata.as_ref().and_then(Value::as_object),
        ) {
            fields.extend(
                metadata
                    .iter()
                    .filter(|(_, value)| !value.is_null())
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        report_failure(
            self.name(),
            "LX playback",
            self.fetch_with_info(platform, quality, info),
        )
    }

    fn fetch_with_info(&self, platform: &str, quality: &str, info: Value) -> Result<AudioStream> {
        let url = self.music_url(platform, quality, info)?;
        let host = url.host_str().unwrap_or_default().to_owned();
        tracing::debug!(%host, platform, quality, "LX musicUrl resolved; fetching audio");
        let result = lx_client()?.get(url).send().map_err(|error| {
            ErrorContext::new(format!("LX audio download ({platform}, {quality})"), error)
        })?;
        tracing::debug!(%host, status = result.status().as_u16(),
            content_length = result.content_length(), "LX audio response");
        Ok(AudioStream::new(result).map_err(|error| {
            ErrorContext::new(format!("LX audio download ({platform}, {quality})"), error)
        })?)
    }
}

pub fn open_lx_with_fallback<'a, T>(
    track: &Track,
    primary: Option<&LxSource>,
    alternatives: impl IntoIterator<Item = (&'a str, &'a str)>,
    platform: &str,
    quality: &str,
    cancelled: impl Fn() -> bool,
    mut open: impl FnMut(AudioStream) -> std::result::Result<T, String>,
) -> std::result::Result<(T, Option<usize>), String> {
    let mut fetch = |source: &LxSource, quality: &str| {
        let stream = source
            .stream_track(platform, quality, track)
            .map_err(crate::public_error)?;
        open(stream)
    };
    let mut errors = Vec::new();
    if let Some(source) = primary {
        match fetch(source, quality) {
            Ok(audio) => return Ok((audio, None)),
            Err(error) => errors.push(format!("{}: {error}", source.name())),
        }
    }
    for (index, (name, contents)) in alternatives.into_iter().enumerate() {
        if cancelled() {
            break;
        }
        let source = match crate::Source::from_document(true, contents) {
            Ok(crate::Source::Lx(source)) => source,
            Ok(_) => continue,
            Err(error) => {
                errors.push(format!("{}: {}", name, crate::public_error(error)));
                continue;
            }
        };
        let Some(qualities) = source.platforms().get(platform) else {
            continue;
        };
        if cancelled() {
            break;
        }
        let fallback_quality = qualities
            .iter()
            .find(|candidate| candidate.as_str() == quality)
            .or_else(|| {
                qualities
                    .iter()
                    .find(|candidate| candidate.as_str() == "128k")
            })
            .unwrap_or(&qualities[0]);
        tracing::info!(source = source.name(), platform, quality = %fallback_quality, "Trying next LX playback source");
        match fetch(&source, fallback_quality) {
            Ok(audio) => return Ok((audio, Some(index))),
            Err(error) => errors.push(format!("{}: {error}", name)),
        }
    }
    if errors.is_empty() {
        return Err(format!(
            "No LX playback source supports platform {platform}"
        ));
    }
    Err(errors.join("\n\n"))
}

const BUDGET: Duration = Duration::from_secs(25);

fn run(
    script: &str,
    script_info: &str,
    init: &Reply<BTreeMap<String, Vec<String>>>,
    inbox: &mpsc::Receiver<Request>,
    cache: Option<Rc<InitCache>>,
) -> Result<()> {
    let rt = Runtime::new()?;
    rt.set_memory_limit(64 * 1024 * 1024);
    rt.set_max_stack_size(1024 * 1024);
    let deadline = Arc::new(Mutex::new(Instant::now() + BUDGET));
    let clock = deadline.clone();
    rt.set_interrupt_handler(Some(Box::new(move || {
        Instant::now() >= *clock.lock().unwrap()
    })));
    let rejections = Rc::new(RefCell::new(BTreeMap::new()));
    let tracked = rejections.clone();
    rt.set_host_promise_rejection_tracker(Some(Box::new(move |_, promise, reason, handled| {
        let key = BuildHasherDefault::<DefaultHasher>::default().hash_one(&promise);
        if handled {
            tracked.borrow_mut().remove(&key);
        } else {
            let message = reason
                .as_exception()
                .and_then(|e| e.message())
                .or_else(|| reason.as_string().and_then(|s| s.to_string().ok()))
                .unwrap_or_else(|| "Unhandled JavaScript rejection".into());
            tracing::debug!(error = %diagnostic_message(&message), "LX asynchronous rejection");
            let mut tracked = tracked.borrow_mut();
            if tracked.len() >= 8 {
                tracked.pop_first();
            }
            tracked.insert(key, message.chars().take(1024).collect::<String>());
        }
    })));
    let context = Context::full(&rt)?;
    context.with(|ctx| -> Result<()> {
        let http = lx_client()?;
        let clock = deadline.clone();
        let last_request = Rc::new(RefCell::new(None::<String>));
        let request_diagnostic = last_request.clone();
        let initializing = Rc::new(Cell::new(true));
        let request_phase = initializing.clone();
        ctx.globals().set(
            "__http",
            Function::new(ctx.clone(), move |url: String, options: String| {
                let remaining = clock
                    .lock()
                    .unwrap()
                    .saturating_duration_since(Instant::now());
                let initialization_cache = cache.as_deref().filter(|_| request_phase.get());
                let (response, diagnostic) =
                    match http_request(&http, &url, &options, remaining, initialization_cache) {
                        Ok(result) => result,
                        Err(error) => {
                            let error = crate::public_error(error);
                            let diagnostic = format!("HTTP request failed: {error}");
                            tracing::warn!(%error, "LX script request failed");
                            (json!({"error": error}), diagnostic)
                        }
                    };
                *request_diagnostic.borrow_mut() = Some(diagnostic);
                response.to_string()
            })?,
        )?;
        ctx.globals().set("__scriptInfo", script_info)?;
        let start = Instant::now();
        ctx.globals().set(
            "__now",
            Function::new(ctx.clone(), move || start.elapsed().as_millis() as f64)?,
        )?;
        crate::lx_utils::install(&ctx)?;
        eval(&ctx, include_str!("lx_host.js"))?;
        eval(&ctx, script)?;
        let init_deadline = *deadline.lock().unwrap();
        let sources =
            promise(&ctx, "lxHost.initialized", init_deadline, &rejections).map_err(|error| {
                format!(
                    "LX initialization (inited): {}",
                    script_failure(&error, last_request.borrow().as_deref())
                )
            })?;
        let capabilities: BTreeMap<String, Capability> = serde_json::from_str(&sources)?;
        let platforms: BTreeMap<_, _> = capabilities
            .into_iter()
            .filter(|(id, cap)| {
                matches!(id.as_str(), "kw" | "kg" | "wy" | "tx" | "mg")
                    && cap.kind == "music"
                    && cap.actions.iter().any(|a| a == "musicUrl")
                    && !cap.qualitys.is_empty()
            })
            .map(|(id, cap)| (id, cap.qualitys))
            .collect();
        if platforms.is_empty() {
            return Err("Script declares no online musicUrl platform".into());
        }
        init.send(Ok(platforms))?;
        while let Some(req) = next_request(&ctx, inbox, &deadline) {
            initializing.set(false);
            rejections.borrow_mut().clear();
            last_request.borrow_mut().take();
            let request_deadline = Instant::now() + BUDGET;
            *deadline.lock().unwrap() = request_deadline;
            let result = ctx
                .globals()
                .set("__event", req.event)
                .map_err(|e| e.to_string())
                .and_then(|_| {
                    promise(
                        &ctx,
                        "lxHost.resolve(__event)",
                        request_deadline,
                        &rejections,
                    )
                });
            let result =
                result.map_err(|error| script_failure(&error, last_request.borrow().as_deref()));
            let _ = req.reply.send(result);
        }
        Ok(())
    })
}

fn script_failure(error: &str, last_request: Option<&str>) -> String {
    let error = sanitize(error);
    match last_request {
        Some(diagnostic) => format!("{error}\nLast LX request: {diagnostic}"),
        None => format!("{error}\nNo LX HTTP request was made"),
    }
}

// LX scripts can continue fetching configuration after emitting inited.
fn next_request(
    ctx: &Ctx<'_>,
    inbox: &mpsc::Receiver<Request>,
    deadline: &Mutex<Instant>,
) -> Option<Request> {
    loop {
        *deadline.lock().unwrap() = Instant::now() + BUDGET;
        if ctx.execute_pending_job() {
            match inbox.try_recv() {
                Ok(request) => return Some(request),
                Err(mpsc::TryRecvError::Disconnected) => return None,
                Err(mpsc::TryRecvError::Empty) => continue,
            }
        }
        let timer = ctx.eval::<i32, _>("lxHost.runNextTimer()").catch(ctx);
        match timer {
            Ok(wait) if wait >= 0 => match inbox.recv_timeout(Duration::from_millis(wait as u64)) {
                Ok(request) => return Some(request),
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            },
            Ok(_) => return inbox.recv().ok(),
            Err(error) => {
                tracing::warn!(error = %diagnostic_message(&error.to_string()), "LX background timer failed");
                return inbox.recv().ok();
            }
        }
    }
}

fn eval(ctx: &Ctx<'_>, code: &str) -> Result<()> {
    ctx.eval::<(), _>(code)
        .catch(ctx)
        .map_err(|e| e.to_string().into())
}

fn promise(
    ctx: &Ctx<'_>,
    code: &str,
    deadline: Instant,
    rejections: &RefCell<BTreeMap<u64, String>>,
) -> std::result::Result<String, String> {
    let promise = ctx
        .eval::<Promise, _>(code)
        .catch(ctx)
        .map_err(|err| err.to_string())?;
    loop {
        if let Some(result) = promise.result() {
            return result.catch(ctx).map_err(|err| err.to_string());
        }
        if Instant::now() >= deadline {
            return Err("LX script timed out".into());
        }
        if !ctx.execute_pending_job() {
            let wait = ctx
                .eval::<i32, _>("lxHost.runNextTimer()")
                .catch(ctx)
                .map_err(|e| e.to_string())?;
            if wait < 0 {
                return Err(rejections
                    .borrow()
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "No response from script and no pending callbacks".into()));
            }
            std::thread::sleep(
                Duration::from_millis(wait as u64)
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

fn http_request(
    client: &reqwest::blocking::Client,
    url: &str,
    options: &str,
    remaining: Duration,
    cache: Option<&InitCache>,
) -> Result<(Value, String)> {
    if remaining.is_zero() {
        return Err("LX request timed out".into());
    }
    let opts: Value = serde_json::from_str(options)?;
    if !opts["formData"].is_null() {
        return Err("LX multipart requests are unsupported".into());
    }
    let method = Method::from_bytes(opts["method"].as_str().unwrap_or("GET").as_bytes())?;
    let cache_path = cache.and_then(|cache| cache.key(&method, url, &opts));
    if let Some(response) = cache
        .zip(cache_path.as_deref())
        .and_then(|(cache, path)| cache.read(path))
    {
        tracing::debug!("Using cached LX initialization response");
        return Ok((response, "Cached LX initialization response".into()));
    }
    let timeout = Duration::from_millis(
        opts["timeout"]
            .as_u64()
            .filter(|ms| *ms > 0)
            .unwrap_or(20000),
    )
    .min(remaining);
    let target = lx_url(url, "LX script request")?;
    let host = target.host_str().unwrap_or_default().to_owned();
    let destination = endpoint(&target);
    tracing::debug!(%host, %method, timeout_ms = timeout.as_millis(), "Sending LX script request");
    let mut req = client.request(method.clone(), target).timeout(timeout);
    if let Some(headers) = opts["headers"].as_object() {
        for (key, val) in headers {
            req = req.header(key, val.as_str().ok_or("Invalid LX header")?);
        }
    }
    if !opts["form"].is_null() {
        req = req.form(&opts["form"]);
    } else if let Some(body) = opts["body"].as_str() {
        req = req.body(body.to_owned());
    } else if !opts["body"].is_null() {
        req = req.json(&opts["body"]);
    }
    let started = Instant::now();
    let response = req.send().map_err(|error| {
        ErrorContext::new(
            format!(
                "LX {method} request ({destination}, timeout {} ms, elapsed {} ms)",
                timeout.as_millis(),
                started.elapsed().as_millis()
            ),
            error,
        )
    })?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        });
    let headers: BTreeMap<_, _> = response
        .headers()
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
        .collect();
    let mut bytes = Vec::new();
    let mut body = response.take(2_097_153);
    body.read_to_end(&mut bytes).map_err(|error| {
        let details = response_details(body.get_ref());
        ErrorContext::new(
            format!(
                "Could not read LX {method} response ({details}, {} bytes read)",
                bytes.len()
            ),
            error,
        )
    })?;
    if bytes.len() > 2_097_152 {
        let details = response_details(body.get_ref());
        return Err(format!("LX {method} response exceeds 2 MiB ({details})").into());
    }
    let body =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    let application_code = ["code", "status", "error_code", "errcode"]
        .into_iter()
        .find_map(|key| {
            let value = body.get(key)?;
            value
                .as_i64()
                .or_else(|| value.as_str()?.parse::<i64>().ok())
        });
    let body_kind = match &body {
        Value::Object(_) => "JSON object",
        Value::Array(_) => "JSON array",
        Value::String(_) => "text",
        _ => "JSON scalar",
    };
    let mut diagnostic = format!(
        "{method} {host} → HTTP {}, {body_kind}, {} bytes",
        status.as_u16(),
        bytes.len()
    );
    if let Some(content_type) = &content_type {
        diagnostic.push_str(&format!(" ({content_type})"));
    }
    if content_type
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("text/html"))
        && body
            .as_str()
            .is_some_and(|html| html.contains("抱歉，站点已暂停"))
    {
        diagnostic.push_str(": upstream site is suspended");
    }
    tracing::debug!(
        %host,
        has_url = body.get("url").is_some(),
        has_data = body.get("data").is_some(),
        has_nested_url = body.pointer("/data/url").is_some(),
        "LX response playback fields"
    );
    if let Some(code) = application_code {
        diagnostic.push_str(&format!(", API code {code}"));
    }
    let message = ["message", "msg", "errmsg", "error_message", "error"]
        .into_iter()
        .find_map(|key| body.get(key).and_then(Value::as_str))
        .map(diagnostic_message)
        .filter(|message| !message.is_empty());
    if let Some(message) = message.as_deref() {
        tracing::debug!(%host, %message, "LX service message");
        diagnostic.push_str(": ");
        diagnostic.push_str(message);
    }
    tracing::info!(%host, %method, status = status.as_u16(), ?application_code,
        bytes = bytes.len(), elapsed_ms = started.elapsed().as_millis(), "LX script response");
    if status.is_client_error() || status.is_server_error() {
        tracing::warn!(%diagnostic, elapsed_ms = started.elapsed().as_millis(), "LX server returned an HTTP error");
    }
    let response = json!({"statusCode":status.as_u16(),"statusMessage":status.canonical_reason(),"headers":headers,"body":body});
    if let Some(path) = cache_path
        && let Err(error) = InitCache::write(&path, &response)
    {
        tracing::debug!(error = %crate::error_details(&error), "Could not save LX initialization response");
    }
    Ok((response, diagnostic))
}

fn diagnostic_message(message: &str) -> String {
    sanitize(message.lines().next().unwrap_or_default())
        .chars()
        .filter(|ch| !ch.is_control())
        .take(300)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"/** @name Example */
        const { on, send, EVENT_NAMES } = window.lx;
        let calls = 0;
        on(EVENT_NAMES.request, async ({source, action, info}) => {
            if (action !== 'musicUrl') throw Error('Unexpected action');
            return 'https://example.org/' + [source, info.musicInfo.songmid, info.type, ++calls].join('/');
        });
        Promise.resolve().then(() => send(EVENT_NAMES.inited, { sources: {
            wy: {type: 'music', actions: ['musicUrl'], qualitys: ['128k']}
        }}));
    "#;

    #[test]
    fn provides_console_groups_in_playback_handlers() {
        let script = SCRIPT.replace(
            "return 'https://example.org/'",
            "console.group('Handle Action(musicUrl)'); console.groupCollapsed('Details'); console.groupEnd(); console.groupEnd(); return 'https://example.org/'",
        );
        let source = LxSource::load(&script).unwrap();
        let url = source
            .music_url("wy", "128k", json!({"songmid":"42"}))
            .unwrap();
        assert_eq!(url.path(), "/wy/42/128k/1");
    }

    #[test]
    fn imports_file_and_resolves_in_same_runtime() {
        let path = std::env::temp_dir().join(format!("rs-music-lx-{}.js", std::process::id()));
        std::fs::write(&path, SCRIPT).unwrap();
        let source = crate::Source::import(&path);
        std::fs::remove_file(path).unwrap();
        let crate::Source::Lx(source) = source.unwrap() else {
            panic!("expected LX source")
        };
        assert_eq!(source.name(), "Example");
        assert!(source.music_url("kw", "128k", json!({})).is_err());
        for call in 1..=2 {
            let url = source
                .music_url("wy", "128k", json!({"songmid":"42"}))
                .unwrap();
            assert_eq!(url.path(), format!("/wy/42/128k/{call}"));
        }
    }

    #[test]
    fn lx_export_preserves_raw_script_and_metadata() {
        let script = SCRIPT.split_once("*/").unwrap().1;
        let script = format!(
            "{script}\nif (lx.currentScriptInfo.version !== 'v1' || lx.currentScriptInfo.rawScript.startsWith('/*')) throw Error('metadata mismatch');"
        );
        let document = json!({"name":"Exported source", "version":"v1", "script":script});
        let crate::Source::Lx(source) =
            crate::Source::from_document(true, &document.to_string()).unwrap()
        else {
            panic!("expected LX source");
        };
        assert_eq!(source.name(), "Exported source");
        assert!(
            source
                .music_url("wy", "128k", json!({"songmid":"42"}))
                .is_ok()
        );
    }

    #[test]
    fn imports_compressed_lx_music_export() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let script = SCRIPT.split_once("*/").unwrap().1;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(script.as_bytes()).unwrap();
        let encoded = format!("gz_{}", STANDARD.encode(encoder.finish().unwrap()));
        let document = json!({"name":"Compressed source", "version":"v1", "script":encoded});
        let crate::Source::Lx(source) =
            crate::Source::from_document(true, &document.to_string()).unwrap()
        else {
            panic!("expected LX source");
        };
        assert_eq!(source.name(), "Compressed source");
        assert_eq!(
            source
                .music_url("wy", "128k", json!({"songmid":"42"}))
                .unwrap()
                .path(),
            "/wy/42/128k/1"
        );
    }

    #[test]
    fn reports_undefined_music_url_as_missing_playback_address() {
        let script = SCRIPT.replace(
            "return 'https://example.org/' + [source, info.musicInfo.songmid, info.type, ++calls].join('/');",
            "return undefined;",
        );
        let source = LxSource::load(&script).unwrap();
        let error = source
            .music_url("wy", "128k", json!({"songmid": "42"}))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("LX script returned no playback URL (result: undefined)"),
            "{error}"
        );
    }

    #[test]
    fn accepts_common_headers_and_delayed_initialization() {
        for opener in ["/*!", "/*"] {
            let script = SCRIPT
                .replacen("/**", opener, 1)
                .replace("Promise.resolve().then(", "setTimeout(")
                .replace(
                    "wy: {type:",
                    "local: {type: 'music', actions: ['lyric']}, wy: {type:",
                );
            assert_eq!(LxSource::load(&script).unwrap().name(), "Example");
        }
    }

    #[test]
    fn reports_async_initialization_error() {
        let script = "/** @name Async */ Promise.resolve().then(() => { throw Error('configuration unavailable') })";
        let err = LxSource::load(script).err().unwrap().to_string();
        assert!(err.contains("configuration unavailable"), "{err}");
    }

    #[test]
    fn provides_encoding_and_crypto_helpers_to_scripts() {
        let checks = r#"
            const {buffer, crypto} = lx.utils
            const encode = (s, fmt) => buffer.bufToString(buffer.from(s), fmt)
            if (encode('洛雪', 'base64') !== '5rSb6Zuq') throw Error('bad encoding')
            if (crypto.md5('abc') !== '900150983cd24fb0d6963f7d28e17f72') throw Error('bad digest')
            const result = crypto.aesEncrypt('', 'aes-128-ecb', new Uint8Array(16), '')
            if (buffer.bufToString(result, 'hex') !== '0143db63ee66b0cdff9f69917680151e') throw Error('bad encryption')
            const timer = setTimeout(() => {throw Error('timer not cancelled')})
            clearTimeout(timer)
        "#;
        let script = SCRIPT
            .replace("let calls = 0;", checks)
            .replace("Promise.resolve().then(", "setTimeout(");
        assert!(LxSource::load(&script).is_ok());
    }

    #[test]
    fn rejects_missing_handshake_and_script_errors() {
        assert!(LxSource::load("/** @name Empty */").is_err());
        assert!(LxSource::load("/** @name Broken */ throw Error('broken')").is_err());
    }
}
