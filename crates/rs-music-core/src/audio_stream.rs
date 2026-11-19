use crate::{
    Result, Url,
    diagnostics::response_details,
    error_details,
    http::{check_response, client},
};
use reqwest::blocking::Response;
use std::io::{self, Read};

pub fn fetch_audio(url: Url) -> Result<Vec<u8>> {
    open_audio(url)?.read_all()
}

pub(crate) fn open_audio(url: Url) -> Result<AudioStream> {
    AudioStream::new(client()?.get(url).send()?)
}

/// A blocking audio response capped at [`Self::MAX_BYTES`], including bodies of unknown length.
pub struct AudioStream {
    response: Response,
    remaining: u64,
}

impl AudioStream {
    pub const MAX_BYTES: u64 = 40 * 1024 * 1024;

    pub(crate) fn new(response: Response) -> Result<Self> {
        check_response(&response, "Unexpected audio response status")?;
        if let Some(len) = response.content_length()
            && len > Self::MAX_BYTES
        {
            let details = response_details(&response);
            return Err(
                format!("Audio response exceeds 40 MiB ({details}, declared {len} bytes)").into(),
            );
        }
        Ok(Self {
            response,
            remaining: Self::MAX_BYTES,
        })
    }

    pub fn content_length(&self) -> Option<u64> {
        self.response.content_length()
    }

    pub(crate) fn read_all(mut self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn read_body(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.response.read(buffer).map_err(|error| {
            let details = response_details(&self.response);
            let read = Self::MAX_BYTES - self.remaining;
            io::Error::new(
                error.kind(),
                format!(
                    "Could not read audio body ({details}, {read} bytes read) → {}",
                    error_details(&error)
                ),
            )
        })
    }
}

impl Read for AudioStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !buffer.is_empty() && self.remaining == 0 {
            // At the limit, probe for EOF so an exactly full stream still succeeds.
            return match self.read_body(&mut [0])? {
                0 => Ok(0),
                _ => Err(io::Error::other(format!(
                    "Audio response exceeds 40 MiB ({})",
                    response_details(&self.response)
                ))),
            };
        }
        let len = buffer.len().min(self.remaining as usize);
        let read = self.read_body(&mut buffer[..len])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}
