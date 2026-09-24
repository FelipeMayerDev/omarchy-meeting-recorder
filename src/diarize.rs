//! Who speaks when, in a single audio file.
//!
//! A recording made by the app has two tracks, so the speaker of every line
//! follows from which track is louder. An imported file has one, so the voices
//! themselves have to be told apart. That is sherpa-onnx's offline speaker
//! diarization, run locally: pyannote's segmentation model finds stretches with
//! one voice in them, a speaker embedding model (WeSpeaker ResNet34, trained on
//! VoxCeleb) turns each stretch into a voice print, and the prints are clustered
//! into speakers. Both models are downloaded on first use, about 32 MB together.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use std::ffi::{CString, c_void};

use sherpa_rs_sys as sys;

use crate::transcribe::{Abort, CANCELLED, Event, Events, WHISPER_RATE, download, models_dir};

const SEGMENTATION_FILE: &str = "pyannote-segmentation-3.0.onnx";
const SEGMENTATION_URL: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx";
const SEGMENTATION_MIN_BYTES: u64 = 5_000_000;
const EMBEDDING_FILE: &str = "wespeaker_en_voxceleb_resnet34_LM.onnx";
const EMBEDDING_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34_LM.onnx";
const EMBEDDING_MIN_BYTES: u64 = 20_000_000;

/// Cosine distance under which two stretches are the same voice, when the
/// number of speakers is left to the clustering.
const THRESHOLD: f32 = 0.6;

/// A stretch of one speaker. `speaker` counts from 0 in the order the voices
/// are first heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker: usize,
}

fn model(
    file: &str,
    url: &str,
    min_bytes: u64,
    events: &Events,
    abort: &Abort,
) -> Result<PathBuf, String> {
    let path = models_dir().join(file);
    if std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() >= min_bytes) {
        return Ok(path);
    }
    download(
        url,
        &path,
        "Downloading speaker models",
        min_bytes,
        events,
        abort,
    )?;
    Ok(path)
}

/// Finds the speakers in `samples` (16 kHz mono). `speakers` fixes how many
/// there are; `None` lets the clustering decide.
pub fn turns(
    samples: &[f32],
    speakers: Option<usize>,
    events: &Events,
    abort: &Abort,
) -> Result<Vec<Turn>, String> {
    let segmentation = model(
        SEGMENTATION_FILE,
        SEGMENTATION_URL,
        SEGMENTATION_MIN_BYTES,
        events,
        abort,
    )?;
    let embedding = model(
        EMBEDDING_FILE,
        EMBEDDING_URL,
        EMBEDDING_MIN_BYTES,
        events,
        abort,
    )?;
    if abort.load(Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    let _ = events.send_blocking(Event::Stage("Finding speakers".into()));
    let _ = events.send_blocking(Event::Progress(0.0));
    run(&segmentation, &embedding, samples, speakers, events, abort)
}

fn run(
    segmentation: &Path,
    embedding: &Path,
    samples: &[f32],
    speakers: Option<usize>,
    events: &Events,
    abort: &Abort,
) -> Result<Vec<Turn>, String> {
    let raw = diarize(
        segmentation,
        embedding,
        samples,
        speakers.map_or(-1, |n| n as i32),
        THRESHOLD,
        events,
        abort,
    )?;
    // With the count left open, voices heard for only a few seconds are almost
    // always one of the others on a bad moment (a cough, a laugh, crosstalk).
    let raw = if speakers.is_none() {
        absorb_small_clusters(raw)
    } else {
        raw
    };
    Ok(renumber(raw))
}

/// What the progress callback gets to see.
struct Progress<'a> {
    events: &'a Events,
    abort: &'a Abort,
}

unsafe extern "C" fn on_progress(done: i32, total: i32, arg: *mut c_void) -> i32 {
    // SAFETY: `arg` is the `Progress` that `diarize` keeps alive for the call.
    let progress = unsafe { &*(arg as *const Progress) };
    if total > 0 {
        let _ = progress
            .events
            .send_blocking(Event::Progress(f64::from(done) / f64::from(total)));
    }
    // A non-zero return asks sherpa-onnx to stop.
    i32::from(progress.abort.load(Ordering::Relaxed))
}

/// sherpa-onnx's offline diarization, through its C API directly: the Rust
/// wrapper fixes both models at one thread, several times slower on long files.
fn diarize(
    segmentation: &Path,
    embedding: &Path,
    samples: &[f32],
    clusters: i32,
    threshold: f32,
    events: &Events,
    abort: &Abort,
) -> Result<Vec<(i64, i64, i32)>, String> {
    let path = |p: &Path| CString::new(p.to_string_lossy().as_bytes()).map_err(|e| e.to_string());
    let (segmentation, embedding) = (path(segmentation)?, path(embedding)?);
    let provider = CString::new("cpu").expect("no nul byte");
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8) as i32;
    let config = sys::SherpaOnnxOfflineSpeakerDiarizationConfig {
        segmentation: sys::SherpaOnnxOfflineSpeakerSegmentationModelConfig {
            pyannote: sys::SherpaOnnxOfflineSpeakerSegmentationPyannoteModelConfig {
                model: segmentation.as_ptr(),
            },
            num_threads: threads,
            debug: 0,
            provider: provider.as_ptr(),
        },
        embedding: sys::SherpaOnnxSpeakerEmbeddingExtractorConfig {
            model: embedding.as_ptr(),
            num_threads: threads,
            debug: 0,
            provider: provider.as_ptr(),
        },
        // A positive count fixes the number of speakers; otherwise the
        // threshold decides.
        clustering: sys::SherpaOnnxFastClusteringConfig {
            num_clusters: clusters,
            threshold,
        },
        // Ignore blips shorter than this, and bridge pauses shorter than this.
        min_duration_on: 0.3,
        min_duration_off: 0.5,
    };
    // SAFETY: the config and the strings it points to outlive the call.
    let sd = unsafe { sys::SherpaOnnxCreateOfflineSpeakerDiarization(&config) };
    if sd.is_null() {
        return Err("could not load the speaker models".into());
    }
    let progress = Progress { events, abort };
    // SAFETY: `sd` is valid, `samples` lives through the call, and `progress`
    // is what `on_progress` casts its argument back to.
    let result = unsafe {
        sys::SherpaOnnxOfflineSpeakerDiarizationProcessWithCallback(
            sd,
            samples.as_ptr(),
            samples.len() as i32,
            Some(on_progress),
            &progress as *const Progress as *mut c_void,
        )
    };
    let mut raw = Vec::new();
    if !result.is_null() {
        // SAFETY: `result` is a valid result; the segments array has `count`
        // entries and is freed below, after it has been copied.
        unsafe {
            let count = sys::SherpaOnnxOfflineSpeakerDiarizationResultGetNumSegments(result);
            let segments = sys::SherpaOnnxOfflineSpeakerDiarizationResultSortByStartTime(result);
            if !segments.is_null() && count > 0 {
                for s in std::slice::from_raw_parts(segments, count as usize) {
                    raw.push((
                        (f64::from(s.start) * 1000.0) as i64,
                        (f64::from(s.end) * 1000.0) as i64,
                        s.speaker,
                    ));
                }
                sys::SherpaOnnxOfflineSpeakerDiarizationDestroySegment(segments);
            }
            sys::SherpaOnnxOfflineSpeakerDiarizationDestroyResult(result);
        }
    }
    // SAFETY: created above and not used after this.
    unsafe { sys::SherpaOnnxDestroyOfflineSpeakerDiarization(sd) };
    if abort.load(Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    Ok(raw)
}

/// Gives every cluster with little speech (under 4 seconds, or under 4% of
/// all speech) to the speaker of the nearest turn from a cluster that stays.
fn absorb_small_clusters(raw: Vec<(i64, i64, i32)>) -> Vec<(i64, i64, i32)> {
    let mut spoken = std::collections::HashMap::<i32, i64>::new();
    for (start, end, id) in &raw {
        *spoken.entry(*id).or_default() += end - start;
    }
    let total: i64 = spoken.values().sum();
    let floor = (total * 4 / 100).max(4000);
    let keeps = |id: &i32| spoken.get(id).is_some_and(|ms| *ms >= floor);
    if spoken.keys().filter(|id| keeps(id)).count() == 0 {
        return raw;
    }
    let anchors: Vec<(i64, i64, i32)> =
        raw.iter().copied().filter(|(_, _, id)| keeps(id)).collect();
    raw.iter()
        .map(|&(start, end, id)| {
            if keeps(&id) {
                return (start, end, id);
            }
            let middle = (start + end) / 2;
            let nearest = anchors
                .iter()
                .min_by_key(|(s, e, _)| {
                    if middle < *s {
                        s - middle
                    } else {
                        (middle - e).max(0)
                    }
                })
                .map_or(id, |(_, _, other)| *other);
            (start, end, nearest)
        })
        .collect()
}

/// Sorts the turns and numbers the speakers in the order they are first heard.
fn renumber(mut raw: Vec<(i64, i64, i32)>) -> Vec<Turn> {
    raw.sort_by_key(|(start, _, _)| *start);
    let mut order: Vec<i32> = Vec::new();
    raw.into_iter()
        .map(|(start_ms, end_ms, id)| {
            let speaker = order.iter().position(|o| *o == id).unwrap_or_else(|| {
                order.push(id);
                order.len() - 1
            });
            Turn {
                start_ms,
                end_ms,
                speaker,
            }
        })
        .collect()
}

/// The whole file as one speaker, for when there is only one.
pub fn single(samples: &[f32]) -> Vec<Turn> {
    vec![Turn {
        start_ms: 0,
        end_ms: (samples.len() * 1000 / WHISPER_RATE) as i64,
        speaker: 0,
    }]
}

/// The speaker of `start_ms..end_ms`: the one whose turns overlap it most, or
/// the nearest turn when none do (whisper's words can fall in a gap).
pub fn speaker_at(turns: &[Turn], start_ms: i64, end_ms: i64) -> usize {
    let end_ms = end_ms.max(start_ms + 1);
    let mut overlap = std::collections::HashMap::<usize, i64>::new();
    for turn in turns {
        let shared = turn.end_ms.min(end_ms) - turn.start_ms.max(start_ms);
        if shared > 0 {
            *overlap.entry(turn.speaker).or_default() += shared;
        }
    }
    if let Some((speaker, _)) = overlap
        .into_iter()
        .max_by_key(|(s, o)| (*o, usize::MAX - s))
    {
        return speaker;
    }
    let middle = (start_ms + end_ms) / 2;
    turns
        .iter()
        .min_by_key(|t| {
            if middle < t.start_ms {
                t.start_ms - middle
            } else {
                (middle - t.end_ms).max(0)
            }
        })
        .map_or(0, |t| t.speaker)
}

/// Where `speaker`'s turn starts near `around_ms`, within a second and a half.
pub fn turn_start_near(turns: &[Turn], speaker: usize, around_ms: i64) -> Option<i64> {
    turns
        .iter()
        .filter(|t| t.speaker == speaker && (t.start_ms - around_ms).abs() <= 1500)
        .min_by_key(|t| (t.start_ms - around_ms).abs())
        .map(|t| t.start_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speakers_are_numbered_by_first_appearance() {
        let turns = renumber(vec![
            (5000, 6000, 3),
            (0, 1000, 7),
            (2000, 3000, 3),
            (7000, 8000, 7),
        ]);
        let order: Vec<usize> = turns.iter().map(|t| t.speaker).collect();
        assert_eq!(order, vec![0, 1, 1, 0]);
    }

    #[test]
    fn words_go_to_the_turn_they_overlap_most() {
        let turns = vec![
            Turn {
                start_ms: 0,
                end_ms: 2000,
                speaker: 0,
            },
            Turn {
                start_ms: 1800,
                end_ms: 5000,
                speaker: 1,
            },
        ];
        assert_eq!(speaker_at(&turns, 1500, 1900), 0);
        assert_eq!(speaker_at(&turns, 1900, 3000), 1);
        // In a gap after the last turn: the nearest one.
        assert_eq!(speaker_at(&turns, 6000, 6500), 1);
        assert_eq!(turn_start_near(&turns, 1, 2500), Some(1800));
        assert_eq!(turn_start_near(&turns, 1, 9000), None);
    }
}
