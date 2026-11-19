use crate::{
    Result,
    diagnostics::{ErrorContext, diagnostic_message, endpoint, response_details},
    http::lx_url,
    lx_cache::InitCache,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    time::{Duration, Instant},
};
/// 返回交给脚本的响应对象和供宿主记录的诊断文本。HTTP 错误状态仍交给脚本处理。
/// 脚本指定的超时受 `remaining` 限制；初始化阶段可以传入缓存以复用响应。
pub(super) fn http_request(
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
