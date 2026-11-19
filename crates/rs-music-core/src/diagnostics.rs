use reqwest::{Url, blocking::Response, header::CONTENT_TYPE};
use std::{error::Error, fmt, io};

/// Formats the causes of an error for display and logging without request URLs.
pub fn error_details(error: &(dyn Error + 'static)) -> String {
    let mut details = Vec::new();
    let mut cause = Some(error);
    while let Some(error) = cause {
        if details.len() == 12 {
            details.push("additional error causes omitted".into());
            break;
        }
        let message = match error.downcast_ref::<reqwest::Error>() {
            Some(error) => request_error(error),
            None => sanitize(&error.to_string()),
        };
        details.push(message);
        cause = error
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| inner as &dyn Error)
            .or_else(|| error.source());
    }
    details.dedup();
    details.join(" → ")
}

fn request_error(error: &reqwest::Error) -> String {
    let message = if let Some(status) = error.status() {
        format!("HTTP {status}")
    } else if error.is_timeout() {
        "HTTP request timed out".into()
    } else if error.is_connect() {
        "HTTP connection failed".into()
    } else if error.is_redirect() {
        "HTTP redirect failed".into()
    } else if error.is_body() {
        "HTTP body transfer failed".into()
    } else if error.is_decode() {
        "HTTP response decoding failed".into()
    } else if error.is_builder() {
        "HTTP request construction failed".into()
    } else {
        "HTTP request failed".into()
    };
    match error.url() {
        Some(url) => format!("{message} ({})", endpoint(url)),
        None => message,
    }
}

pub(crate) fn endpoint(url: &Url) -> String {
    let host = url.host_str().unwrap_or("<unknown>");
    match url.port_or_known_default() {
        Some(port) => format!("host={host}, port={port}, scheme={}", url.scheme()),
        None => format!("host={host}, scheme={}", url.scheme()),
    }
}

pub(crate) fn response_details(response: &Response) -> String {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| {
            value.split_once('/').is_some()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/!#$&^_.+-".contains(&byte))
        })
        .unwrap_or("unknown");
    format!(
        "HTTP {}, {}, content-type={content_type}",
        response.status(),
        endpoint(response.url())
    )
}

pub(crate) fn sanitize(message: &str) -> String {
    message
        .lines()
        .take(20)
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if [
                "authorization:",
                "authorization=",
                "authorization\":",
                "cookie:",
                "cookie=",
                "cookie\":",
                "bearer ",
                "token=",
                "token\":",
                "password=",
                "password\":",
                "api_key=",
                "api-key:",
            ]
            .iter()
            .any(|secret| lower.contains(secret))
            {
                return "[redacted credential details]".into();
            }
            line.split_whitespace()
                .take(100)
                .map(|word| {
                    let address = word.trim_start_matches(['(', '[', '"', '\'', '<']);
                    if word.contains("://") || address.starts_with("//") || word.contains('?') {
                        "<redacted URL>".into()
                    } else {
                        word.chars()
                            .filter(|ch| !ch.is_control())
                            .take(250)
                            .collect::<String>()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug)]
pub(crate) struct ErrorContext {
    message: String,
    source: Box<dyn Error>,
}

impl ErrorContext {
    pub(crate) fn new(message: impl Into<String>, source: impl Into<Box<dyn Error>>) -> Self {
        Self {
            message: message.into(),
            source: source.into(),
        }
    }
}

impl fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ErrorContext {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_nested_io_causes() {
        let error = ErrorContext::new(
            "Read audio",
            io::Error::other(io::Error::new(io::ErrorKind::TimedOut, "socket timed out")),
        );
        assert_eq!(error_details(&error), "Read audio → socket timed out");
    }

    #[test]
    fn keeps_only_safe_endpoint_and_cause_information() {
        let url = Url::parse("https://user:secret@example.test:8443/audio?signature=123").unwrap();
        assert_eq!(endpoint(&url), "host=example.test, port=8443, scheme=https");
        let error = ErrorContext::new(
            format!("Read audio ({})", endpoint(&url)),
            io::Error::other(format!("while reading {url}\nAuthorization: Bearer secret")),
        );
        assert_eq!(
            error_details(&error),
            "Read audio (host=example.test, port=8443, scheme=https) → while reading <redacted URL>\n[redacted credential details]"
        );
    }
}

pub(crate) fn diagnostic_message(message: &str) -> String {
    sanitize(message.lines().next().unwrap_or_default())
        .chars()
        .filter(|ch| !ch.is_control())
        .take(300)
        .collect()
}
