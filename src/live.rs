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
const CHUNK: usize = WHISPER_RATE * 2;

#[derive(Clone, Debug)]
pub struct Update {
    pub turns: Vec<Turn>,
}

/// Runs in a worker thread. Chunks are already being captured for recording;
/// this only converts the computer track to 16 kHz mono for the preview.
pub fn run(
    chunks: Receiver<Vec<u8>>,
    provider: Provider,
    updates: async_channel::Sender<Update>,
    abort: Abort,
) {
    let mut samples = Vec::with_capacity(CHUNK);
    let mut offset_samples = 0usize;
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
        samples.extend(mono_16k(&chunk));
        if samples.len() < CHUNK {
            continue;
        }
        let current: Vec<f32> = samples.drain(..CHUNK).collect();
        let offset_ms = (offset_samples * 1000 / WHISPER_RATE) as i64;
        offset_samples += current.len();
        let turns =
            match crate::diarize::live_turns(&current, &provider, &session, offset_ms, &abort) {
                Ok(turns) => turns,
                Err(_) if abort.load(Ordering::Relaxed) => break,
                Err(_) => continue,
            };
        if updates.send_blocking(Update { turns }).is_err() {
            break;
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
