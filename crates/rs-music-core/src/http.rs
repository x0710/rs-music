use crate::{
    Result, Url,
    diagnostics::{ErrorContext, response_details},
};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use serde::de::DeserializeOwned;
use std::{
    io::{self, Read},
    time::Duration,
};

pub(crate) fn https_url(input: &str) -> Result<Url> {
    let url = Url::parse(input)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(io::Error::other("Expected an HTTPS URL without credentials").into());
    }
    Ok(url)
}

/// 处理 LX 脚本返回的地址。`//host/path` 补全为 HTTPS，显式 HTTP 保留。
/// 缺少主机名、包含凭据或协议不受支持时，错误携带调用方传入的阶段名称。
pub(crate) fn lx_url(input: &str, stage: &str) -> Result<Url> {
    let input = input.trim();
    let normalized;
    let input = if input.starts_with("//") {
        normalized = format!("https:{input}");
        &normalized
    } else {
        input
    };
    let url = Url::parse(input)
        .map_err(|error| io::Error::other(format!("{stage}: invalid URL ({error})")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(io::Error::other(format!(
            "{stage}: expected an HTTP(S) URL without embedded credentials"
        ))
        .into());
    }
    if url.scheme() == "http" {
        tracing::debug!(stage, "LX source uses an unencrypted HTTP connection");
    }
    Ok(url)
}

pub(crate) fn lx_client() -> Result<Client> {
    Ok(Client::builder().timeout(Duration::from_secs(20)).build()?)
}

pub(crate) fn client() -> Result<Client> {
    Ok(Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(20))
        .build()?)
}

pub(crate) fn read_json<T: DeserializeOwned>(response: Response) -> Result<T> {
    let details = response_details(&response);
    let bytes = read_limited(response, 2 * 1024 * 1024)?;
    Ok(serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Could not parse JSON response ({details}, {} bytes) → JSON {:?} error at line {}, column {}",
            bytes.len(), error.classify(), error.line(), error.column()
        )
    })?)
}

pub(crate) fn read_limited(response: Response, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    check_response(&response, "Unexpected response status")?;
    let mut body = response.take(limit + 1);
    body.read_to_end(&mut bytes).map_err(|error| {
        let details = response_details(body.get_ref());
        ErrorContext::new(format!("Could not read response body ({details})"), error)
    })?;
    if bytes.len() as u64 > limit {
        let details = response_details(body.get_ref());
        return Err(format!("Response exceeds the {limit}-byte limit ({details})").into());
    }
    Ok(bytes)
}

pub(crate) fn check_response(response: &Response, message: &str) -> Result<()> {
    if response.status() != StatusCode::OK {
        return Err(format!("{message} ({})", response_details(response)).into());
    }
    Ok(())
}
