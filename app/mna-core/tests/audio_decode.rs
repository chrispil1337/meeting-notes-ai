use std::path::PathBuf;

use mna_core::audio_decode::{DecodeError, decode_file_to_mono, is_supported_extension};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn never() -> bool {
    false
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[test]
fn decodes_voice_memo_style_aac_m4a_and_mixes_to_mono() {
    let audio = decode_file_to_mono(&fixture("tone_stereo.m4a"), &never).expect("decode m4a");
    assert_eq!(audio.sample_rate, 44_100);
    // about one second (AAC adds priming/padding frames)
    assert!(
        (900..=1200).contains(&audio.duration_ms()),
        "{} ms",
        audio.duration_ms()
    );
    assert!(rms(&audio.samples) > 0.05);
    assert!(audio.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
}

#[test]
fn decodes_mp3() {
    let audio = decode_file_to_mono(&fixture("tone.mp3"), &never).expect("decode mp3");
    assert_eq!(audio.sample_rate, 44_100);
    assert!((900..=1200).contains(&audio.duration_ms()));
    assert!(rms(&audio.samples) > 0.05);
}

#[test]
fn decodes_wav_and_flac() {
    let wav = decode_file_to_mono(&fixture("tone_16k.wav"), &never).expect("decode wav");
    assert_eq!(wav.sample_rate, 16_000);
    assert_eq!(wav.samples.len(), 16_000);

    let flac = decode_file_to_mono(&fixture("tone_stereo.flac"), &never).expect("decode flac");
    assert_eq!(flac.sample_rate, 48_000);
    assert_eq!(flac.samples.len(), 48_000);
}

#[test]
fn rejects_non_audio_and_missing_files() {
    assert_eq!(
        decode_file_to_mono(&fixture("not_audio.m4a"), &never),
        Err(DecodeError::UnsupportedFormat)
    );
    assert!(matches!(
        decode_file_to_mono(&fixture("does_not_exist.m4a"), &never),
        Err(DecodeError::Open(_))
    ));
}

#[test]
fn can_be_canceled() {
    let result = decode_file_to_mono(&fixture("tone.mp3"), &|| true);
    assert!(matches!(result, Err(DecodeError::Decode(_))));
}

#[test]
fn supported_extensions() {
    assert!(is_supported_extension(&PathBuf::from(
        "Neue Aufnahme 3.M4A"
    )));
    assert!(is_supported_extension(&PathBuf::from("memo.mp3")));
    assert!(!is_supported_extension(&PathBuf::from("notes.txt")));
    assert!(!is_supported_extension(&PathBuf::from("noext")));
}
