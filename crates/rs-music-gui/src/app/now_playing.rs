use super::{
    cover,
    tasks::{Task, finished, spawn},
    title,
};
use eframe::egui;
use rs_music_core::{
    Track,
    lyrics::{self, Line, Lyrics},
    public_error,
};
use std::{borrow::Cow, path::PathBuf, time::Duration};

#[derive(Clone)]
enum Target {
    Local(PathBuf),
    Online { platform: String, track: Track },
}

enum Load {
    Pending(Task<Option<Lyrics>>),
    Complete(Result<Option<Lyrics>, String>),
}

enum Cover {
    Pending(Task<Option<egui::ColorImage>>),
    Ready(Option<egui::TextureHandle>),
}

#[derive(Default)]
pub(super) struct NowPlaying {
    target: Option<Target>,
    load: Option<Load>,
    cover: Option<Cover>,
    pub generation: u64,
    pub followed_line: Option<usize>,
}

impl NowPlaying {
    #[cfg(target_os = "linux")]
    pub fn clear_texture(&mut self) {
        if matches!(self.cover, Some(Cover::Ready(Some(_)))) {
            self.cover = None;
        }
    }

    fn select(&mut self, target: Option<Target>) {
        self.target = target;
        self.load = None;
        self.cover = None;
        self.followed_line = None;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn local(&mut self, path: PathBuf) {
        self.select(Some(Target::Local(path)));
    }

    pub fn online(
        &mut self,
        platform: String,
        track: Track,
        pending: Option<Task<Option<Lyrics>>>,
    ) {
        self.select(Some(Target::Online { platform, track }));
        self.load = pending.map(Load::Pending);
    }

    pub fn clear(&mut self) {
        self.select(None);
    }

    pub fn has_track(&self) -> bool {
        self.target.is_some()
    }

    pub fn title_and_artist(&self) -> Option<(Cow<'_, str>, &str)> {
        match self.target.as_ref()? {
            Target::Local(path) => Some((title(path).into(), "")),
            Target::Online { track, .. } => Some((track.title.as_str().into(), &track.artist)),
        }
    }

    pub fn artwork(&self) -> Option<&egui::TextureHandle> {
        match &self.cover {
            Some(Cover::Ready(texture)) => texture.as_ref(),
            _ => None,
        }
    }

    pub fn lyrics(&self) -> Option<&Result<Option<Lyrics>, String>> {
        match &self.load {
            Some(Load::Complete(result)) => Some(result),
            _ => None,
        }
    }

    pub fn current_line(&self, position: Duration) -> Option<&str> {
        let Ok(Some(Lyrics::Timed(lines))) = self.lyrics()? else {
            return None;
        };
        active_line(lines, position).map(|index| lines[index].text.as_ref())
    }

    pub fn retry_lyrics(&mut self) {
        self.load = None;
    }

    pub fn update(&mut self, ctx: &egui::Context) {
        let Some(target) = &self.target else {
            return;
        };
        if self.cover.is_none() {
            let target = target.clone();
            self.cover = Some(Cover::Pending(spawn(ctx, move || {
                let result = match &target {
                    Target::Local(path) => lyrics::local_cover(path),
                    Target::Online { platform, track } => lyrics::online_cover(platform, track),
                };
                result
                    .and_then(|cover| cover.map(|bytes| cover::decode(&bytes, 512)).transpose())
                    .map_err(|error| {
                        let error = public_error(error);
                        match &target {
                            Target::Local(path) => {
                                tracing::warn!(path = %path.display(), %error, "Could not load song cover");
                            }
                            Target::Online { platform, track } => {
                                tracing::warn!(%platform, title = %track.title, %error, "Could not load song cover");
                            }
                        }
                        error
                    })
            })));
        }
        if self.load.is_none() {
            let target = target.clone();
            self.load = Some(Load::Pending(spawn(ctx, move || match target {
                Target::Local(path) => lyrics::local(&path).map_err(|error| {
                    let error = public_error(error);
                    tracing::warn!(path = %path.display(), %error, "Could not load local lyrics");
                    error
                }),
                Target::Online { platform, track } => {
                    lyrics::online(&platform, &track).map_err(public_error)
                }
            })));
        }

        if let Some(Cover::Pending(task)) = &self.cover
            && let Some(result) = finished(task)
        {
            self.cover = Some(Cover::Ready(result.ok().flatten().map(|image| {
                ctx.load_texture("track-cover", image, egui::TextureOptions::LINEAR)
            })));
        }
        if let Some(Load::Pending(task)) = &self.load
            && let Some(result) = finished(task)
        {
            self.load = Some(Load::Complete(result));
        }
        if matches!(self.load, Some(Load::Pending(_)))
            || matches!(self.cover, Some(Cover::Pending(_)))
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

pub(super) fn active_line(lines: &[Line], position: Duration) -> Option<usize> {
    lines
        .partition_point(|line| line.at <= position)
        .checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_line_moves_forward_and_backward_with_playback_position() {
        let playing = NowPlaying {
            load: Some(Load::Complete(Ok(Lyrics::parse(
                "[00:02]First\n[00:04]Second",
            )))),
            ..NowPlaying::default()
        };
        assert_eq!(playing.current_line(Duration::from_secs(1)), None);
        assert_eq!(playing.current_line(Duration::from_secs(4)), Some("Second"));
        assert_eq!(playing.current_line(Duration::from_secs(2)), Some("First"));
    }
}
