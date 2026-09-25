//! A low-latency, provisional view of the speakers in computer audio.
//!
//! The remote pyannote 3.1 pipeline is offline, so this sends overlapping
//! windows to the existing endpoint. The final post-call diarization remains
//! authoritative.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;

use crate::audio::CHANNELS;
use crate::diarize::{Provider, Turn};
use crate::transcribe::{Abort, WHISPER_RATE};

/// Eight seconds of context, refreshed every four seconds.
const WINDOW: usize = WHISPER_RATE * 8;
const STEP: usize = WHISPER_RATE * 4;

#[derive(Clone, Debug)]
pub struct Update {
    pub turns: Vec<Turn>,
}

/// Runs in a worker thread. Chunks are already being captured for recording;
/// this only converts the computer track to 16 kHz mono for the preview.
pub fn run(
    chunks: Receiver<Vec<u8>>,
    provider: Provider,
    speaker_count: Option<usize>,
    updates: async_channel::Sender<Update>,
    abort: Abort,
) {
    let mut samples = Vec::with_capacity(WINDOW);
    let mut labels = StableLabels::default();
    while !abort.load(Ordering::Relaxed) {
        let Ok(chunk) = chunks.recv() else { break };
        samples.extend(mono_16k(&chunk));
        if samples.len() < WINDOW {
            continue;
        }
        let offset_ms = ((samples.len() - WINDOW) * 1000 / WHISPER_RATE) as i64;
        let (events, _discard) = async_channel::unbounded();
        let turns = match crate::diarize::turns(&samples, speaker_count, &provider, &events, &abort)
        {
            Ok(turns) => turns,
            Err(_) if abort.load(Ordering::Relaxed) => break,
            Err(_) => {
                samples.drain(..STEP);
                continue;
            }
        };
        let turns = turns
            .into_iter()
            .map(|mut turn| {
                turn.start_ms += offset_ms;
                turn.end_ms += offset_ms;
                turn
            })
            .collect();
        if updates
            .send_blocking(Update {
                turns: labels.assign(turns),
            })
            .is_err()
        {
            break;
        }
        samples.drain(..STEP);
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

#[derive(Default)]
struct StableLabels {
    previous: Vec<Turn>,
    next: usize,
}

impl StableLabels {
    fn assign(&mut self, mut turns: Vec<Turn>) -> Vec<Turn> {
        let mut speakers: Vec<usize> = turns
            .iter()
            .map(|turn| turn.speaker)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        speakers.sort_unstable();
        let mut scores: HashMap<(usize, usize), i64> = HashMap::new();
        for turn in &turns {
            for previous in &self.previous {
                let overlap = (turn.end_ms.min(previous.end_ms)
                    - turn.start_ms.max(previous.start_ms))
                .max(0);
                *scores.entry((turn.speaker, previous.speaker)).or_default() += overlap;
            }
        }
        let mut mapped = HashMap::new();
        let mut used = HashSet::new();
        for speaker in speakers {
            let known = self
                .previous
                .iter()
                .map(|turn| turn.speaker)
                .collect::<HashSet<_>>();
            let best = known
                .into_iter()
                .filter(|known| !used.contains(known))
                .max_by_key(|known| scores.get(&(speaker, *known)).copied().unwrap_or(0));
            let label = match best
                .filter(|known| scores.get(&(speaker, *known)).copied().unwrap_or(0) > 0)
            {
                Some(label) => label,
                None => {
                    let label = self.next;
                    self.next += 1;
                    label
                }
            };
            used.insert(label);
            mapped.insert(speaker, label);
        }
        for turn in &mut turns {
            turn.speaker = mapped[&turn.speaker];
        }
        // ponytail: identities survive only the four-second overlap; use a
        // stateful streaming model if long silent gaps need stable identities.
        self.previous = turns.clone();
        turns
    }
}

#[cfg(test)]
mod tests {
    use super::{StableLabels, Turn};

    #[test]
    fn labels_stay_stable_across_an_overlapping_window() {
        let mut labels = StableLabels::default();
        let first = labels.assign(vec![Turn {
            start_ms: 0,
            end_ms: 4000,
            speaker: 0,
        }]);
        let second = labels.assign(vec![Turn {
            start_ms: 3000,
            end_ms: 7000,
            speaker: 1,
        }]);
        assert_eq!(first[0].speaker, second[0].speaker);
    }
}
