//! A low-latency, provisional view of the speakers in computer audio.
//!
//! NeMo Streaming Sortformer keeps a remote speaker cache per recording, so
//! it can retain identities instead of comparing unrelated rolling windows.

use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::audio::CHANNELS;
use crate::diarize::{Provider, Turn};
use crate::transcribe::{Abort, WHISPER_RATE};

/// Two seconds keeps requests cheap without making the timeline feel delayed.
const DIARIZATION_CHUNK: usize = WHISPER_RATE * 2;
/// Eight seconds keeps short phrases from being cut before Whisper has context.
const CAPTION_CHUNK: usize = WHISPER_RATE * 8;

#[derive(Clone, Debug)]
pub struct Update {
    pub turns: Vec<Turn>,
    pub captions: Vec<Caption>,
}

#[derive(Clone, Debug)]
pub struct Caption {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker: usize,
    pub text: String,
}

/// Runs in a worker thread. Chunks are already being captured for recording;
/// this only converts the computer track to 16 kHz mono for the preview.
pub fn run(
    chunks: Receiver<Vec<u8>>,
    provider: Provider,
    language: &'static str,
    updates: async_channel::Sender<Update>,
    abort: Abort,
) {
    let mut diarization = Vec::with_capacity(DIARIZATION_CHUNK);
    let mut captions = Vec::with_capacity(CAPTION_CHUNK);
    let mut diarization_offset = 0usize;
    let mut caption_offset = 0usize;
    let mut timeline = Vec::new();
    let caption_language = live_language(language);
    let session = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    while !abort.load(Ordering::Relaxed) {
        let Ok(chunk) = chunks.recv() else { break };
        let samples = mono_16k(&chunk);
        diarization.extend_from_slice(&samples);
        captions.extend_from_slice(&samples);
        if diarization.len() >= DIARIZATION_CHUNK {
            let current: Vec<f32> = diarization.drain(..DIARIZATION_CHUNK).collect();
            let offset_ms = (diarization_offset * 1000 / WHISPER_RATE) as i64;
            diarization_offset += current.len();
            let turns = match crate::diarize::live_turns(
                &current, &provider, &session, offset_ms, &abort,
            ) {
                Ok(turns) => turns,
                Err(_) if abort.load(Ordering::Relaxed) => break,
                Err(_) => Vec::new(),
            };
            timeline.extend(turns.iter().cloned());
            if !turns.is_empty()
                && updates
                    .send_blocking(Update {
                        turns,
                        captions: Vec::new(),
                    })
                    .is_err()
            {
                break;
            }
        }
        if captions.len() >= CAPTION_CHUNK {
            let current: Vec<f32> = captions.drain(..CAPTION_CHUNK).collect();
            let offset_ms = (caption_offset * 1000 / WHISPER_RATE) as i64;
            caption_offset += current.len();
            let captions =
                match crate::transcribe::live_remote(&current, caption_language, &provider, &abort)
                {
                    Ok(lines) => lines
                        .into_iter()
                        .map(|(start_ms, end_ms, text)| Caption {
                            start_ms: start_ms + offset_ms,
                            end_ms: end_ms + offset_ms,
                            speaker: crate::diarize::speaker_at(
                                &timeline,
                                start_ms + offset_ms,
                                end_ms + offset_ms,
                            ),
                            text,
                        })
                        .collect(),
                    Err(_) if abort.load(Ordering::Relaxed) => break,
                    Err(_) => Vec::new(),
                };
            timeline.retain(|turn| turn.end_ms >= offset_ms - 1000);
            if !captions.is_empty()
                && updates
                    .send_blocking(Update {
                        turns: Vec::new(),
                        captions,
                    })
                    .is_err()
            {
                break;
            }
        }
    }
}

fn mono_16k(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2 * CHANNELS as usize * 3)
        .map(|frame| {
            let mut total = 0i32;
            for offset in (0..2 * CHANNELS as usize * 3).step_by(2 * CHANNELS as usize) {
                for channel in 0..CHANNELS as usize {
                    let at = offset + channel * 2;
                    total += i16::from_le_bytes([frame[at], frame[at + 1]]) as i32;
                }
            }
            total as f32 / (CHANNELS * 3) as f32 / i16::MAX as f32
        })
        .collect()
}

// Short chunks mis-detect Portuguese surprisingly often. The final pass still
// honors Auto; the live preview defaults to the app's main language.
fn live_language(language: &str) -> &str {
    if language == "auto" { "pt" } else { language }
}

#[cfg(test)]
mod tests {
    use super::live_language;

    #[test]
    fn live_auto_uses_portuguese_for_short_chunks() {
        assert_eq!(live_language("auto"), "pt");
        assert_eq!(live_language("en"), "en");
    }
}
