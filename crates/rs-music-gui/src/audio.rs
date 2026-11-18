mod online;

use online::OnlineStatus;
pub(crate) use online::{DownloadMonitor, DownloadProgress, OnlineArchive, OnlineAudio};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
use std::{error::Error, fs::File, io, path::Path, time::Duration};

#[derive(Default)]
pub(crate) struct Audio {
    sink: Option<Sink>,
    stream: Option<OutputStream>,
    duration: Option<Duration>,
    online: Option<OnlineStatus>,
    after_seek_progress: Option<DownloadProgress>,
    archive: Option<ArchiveState>,
    position_offset: Duration,
}

pub(crate) enum ArchiveState {
    Pending(OnlineArchive),
    Requested,
    Skipped,
}

impl Audio {
    #[cfg(test)]
    pub(crate) fn with_drained_sink() -> Self {
        let (sink, _output) = Sink::new();
        Self {
            sink: Some(sink),
            ..Self::default()
        }
    }

    pub(crate) fn play(
        &mut self,
        path: &Path,
        volume: u8,
        paused: bool,
    ) -> Result<(), Box<dyn Error>> {
        let source = Decoder::try_from(File::open(path)?)?;
        self.play_source(source, volume, paused)?;
        self.archive = None;
        Ok(())
    }

    pub(crate) fn play_online(
        &mut self,
        mut source: OnlineAudio,
        duration_hint: Option<Duration>,
        volume: u8,
        paused: bool,
    ) -> Result<(), Box<dyn Error>> {
        let status = source.status();
        let archive = source.archive();
        self.play_source(source, volume, paused)?;
        self.duration = self.duration.or(duration_hint);
        self.online = Some(status);
        self.archive = archive.map(ArchiveState::Pending);
        Ok(())
    }

    fn play_source(
        &mut self,
        source: impl Source + Send + 'static,
        volume: u8,
        paused: bool,
    ) -> Result<(), Box<dyn Error>> {
        if self.stream.is_none() {
            self.stream = Some(OutputStreamBuilder::open_default_stream()?);
        }
        let stream = self.stream.as_ref().expect("output stream initialized");
        let sink = Sink::connect_new(stream.mixer());
        self.duration = source.total_duration();
        sink.set_volume(f32::from(volume) / 100.0);
        if paused {
            sink.pause();
        }
        sink.append(source);
        self.sink = Some(sink);
        self.online = None;
        self.after_seek_progress = None;
        self.position_offset = Duration::ZERO;
        Ok(())
    }

    pub(crate) fn position(&self) -> Duration {
        if let Some(online) = &self.online {
            return online.position();
        }
        self.sink().map_or(Duration::ZERO, |sink| {
            self.position_offset.saturating_add(sink.get_pos())
        })
    }

    pub(crate) fn duration(&self) -> Option<Duration> {
        self.online
            .as_ref()
            .and_then(OnlineStatus::duration)
            .or(self.duration)
    }

    pub(crate) fn can_seek(&self) -> bool {
        self.online.as_ref().is_none_or(OnlineStatus::complete)
    }

    pub(crate) fn seek(&mut self, position: Duration, volume: u8) -> Result<(), Box<dyn Error>> {
        let Some(sink) = self.sink() else {
            return Ok(());
        };
        if let Some(online) = &self.online {
            if !online.complete() {
                return Err(io::Error::other("Audio is still downloading").into());
            }
            let paused = sink.is_paused();
            let mut decoder = online.decoder().map_err(io::Error::other)?;
            decoder.try_seek(position)?;
            let duration = self.duration();
            let progress = online.progress();
            self.play_source(decoder, volume, paused)?;
            self.duration = self.duration.or(duration);
            self.position_offset = position;
            self.after_seek_progress = progress;
        } else {
            sink.try_seek(position)?;
            self.position_offset = Duration::ZERO;
        }
        Ok(())
    }

    pub(crate) fn buffering(&self) -> bool {
        self.playing() && self.online.as_ref().is_some_and(OnlineStatus::buffering)
    }

    pub(crate) fn stream_error(&self) -> Option<String> {
        self.online.as_ref().and_then(OnlineStatus::error)
    }

    pub(crate) fn download_progress(&self) -> Option<DownloadProgress> {
        self.online
            .as_ref()
            .and_then(OnlineStatus::progress)
            .or(self.after_seek_progress)
    }

    pub(crate) fn archive_state(&self) -> Option<&ArchiveState> {
        self.archive.as_ref()
    }

    pub(crate) fn choose_archive(&mut self, save: bool) -> Option<OnlineArchive> {
        if !matches!(self.archive, Some(ArchiveState::Pending(_))) {
            return None;
        }
        let next = if save {
            ArchiveState::Requested
        } else {
            ArchiveState::Skipped
        };
        match self.archive.replace(next) {
            Some(ArchiveState::Pending(archive)) if save => Some(archive),
            _ => None,
        }
    }

    pub(crate) fn sink(&self) -> Option<&Sink> {
        self.sink.as_ref()
    }

    pub(crate) fn playing(&self) -> bool {
        self.sink()
            .is_some_and(|sink| !sink.empty() && !sink.is_paused())
    }

    pub(crate) fn finished(&self) -> bool {
        self.sink().is_some_and(Sink::empty)
    }

    pub(crate) fn stop(&mut self) {
        self.sink = None;
        self.duration = None;
        self.online = None;
        self.after_seek_progress = None;
        self.archive = None;
        self.position_offset = Duration::ZERO;
    }
}
