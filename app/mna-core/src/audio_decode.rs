//! Decode audio files (e.g. voice memos) to mono `f32` samples.
//!
//! Supports `.m4a`/`.mp4` (AAC, as recorded by the Voice Memos app), `.mp3`,
//! `.wav`, `.flac` and `.ogg` (Vorbis) through the pure Rust `symphonia`
//! decoders. Multi-channel audio is mixed down to mono. Resampling to the
//! transcription engine's rate is left to the caller.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// File extensions offered in the import dialog.
pub const SUPPORTED_EXTENSIONS: &[&str] =
    &["m4a", "mp4", "aac", "mp3", "wav", "flac", "ogg", "oga"];

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudio {
    /// Mono samples in -1.0..=1.0
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl DecodedAudio {
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        self.samples.len() as u64 * 1000 / self.sample_rate as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Open(String),
    UnsupportedFormat,
    NoAudioTrack,
    Decode(String),
    Empty,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Open(e) => write!(f, "Could not open the audio file: {e}"),
            DecodeError::UnsupportedFormat => write!(
                f,
                "This audio format is not supported. Use m4a, mp3, wav, flac or ogg."
            ),
            DecodeError::NoAudioTrack => write!(f, "The file contains no audio track."),
            DecodeError::Decode(e) => write!(f, "The audio file could not be decoded: {e}"),
            DecodeError::Empty => write!(f, "The audio file contains no audio."),
        }
    }
}

impl std::error::Error for DecodeError {}

pub fn is_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Decode a whole file to mono samples at its native sample rate.
///
/// `should_cancel` is polled between packets so long files can be aborted.
pub fn decode_file_to_mono(
    path: &Path,
    should_cancel: &dyn Fn() -> bool,
) -> Result<DecodedAudio, DecodeError> {
    let file = File::open(path).map_err(|e| DecodeError::Open(e.to_string()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| DecodeError::UnsupportedFormat)?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(DecodeError::NoAudioTrack)?;
    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| DecodeError::UnsupportedFormat)?;

    let mut samples: Vec<f32> = Vec::new();
    loop {
        if should_cancel() {
            return Err(DecodeError::Decode("canceled".into()));
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A damaged packet is skipped, not fatal
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };
        let spec = *decoded.spec();
        if sample_rate == 0 {
            sample_rate = spec.rate;
        }
        let channels = spec.channels.count().max(1);
        let mut sample_buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sample_buffer.copy_interleaved_ref(decoded);
        let interleaved = sample_buffer.samples();
        samples.reserve(interleaved.len() / channels);
        for frame in interleaved.chunks(channels) {
            let sum: f32 = frame.iter().sum();
            samples.push((sum / channels as f32).clamp(-1.0, 1.0));
        }
    }

    if samples.is_empty() || sample_rate == 0 {
        return Err(DecodeError::Empty);
    }
    Ok(DecodedAudio {
        samples,
        sample_rate,
    })
}
