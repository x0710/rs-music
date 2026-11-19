use crate::{AudioStream, LxSource, Track};
/// Tries the primary source, then compatible alternatives in caller-supplied order.
/// A stream succeeds when `open` accepts it. The returned index is `None` for the
/// primary source or the zero-based position in the original alternatives iterator.
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
