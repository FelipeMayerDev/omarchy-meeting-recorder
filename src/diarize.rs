//! Who speaks when, in a single audio file.
//!
//! A recording made by the app has two tracks, so the speaker of every line
//! follows from which track is louder. An imported file has one, so the voices
//! themselves have to be told apart. That is NVIDIA's Nemotron 3 Diarization
//! (see `nemotron.rs`) locally, or a compatible remote diarization server.

use std::io::Read;
use std::net::IpAddr;
use std::path::PathBuf;

use crate::transcribe::{Abort, Event, Events, WHISPER_RATE};

/// A stretch of one speaker. `speaker` counts from 0 in the order the voices
/// are first heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker: usize,
}

/// Where voices are separated. The API key is only held while the app runs.
#[derive(Clone)]
pub enum Provider {
    Off,
    Local,
    Remote { ip: String, api_key: String },
}

/// Finds the speakers in `samples` (16 kHz mono). `speakers` fixes how many
/// there are; `None` lets the model decide.
pub fn turns(
    samples: &[f32],
    speakers: Option<usize>,
    provider: &Provider,
    events: &Events,
    abort: &Abort,
) -> Result<Vec<Turn>, String> {
    match provider {
        Provider::Off => return Ok(single(samples)),
        Provider::Remote { ip, api_key } => {
            return remote_turns(samples, speakers, ip, api_key, events, abort);
        }
        Provider::Local => {}
    }
    let path = crate::nemotron::ensure(events, abort)?;
    let _ = events.send_blocking(Event::Stage("Finding speakers".into()));
    let _ = events.send_blocking(Event::Progress(0.0));
    let mut model = crate::nemotron::Model::load(&path)?;
    let probs = model.probabilities(samples, events, abort)?;
    let raw = segments(&probs, 8);
    let raw = match speakers {
        Some(n) => keep_largest(raw, n),
        // Voices heard for only a few seconds are almost always one of the
        // others on a bad moment (a cough, a laugh, crosstalk).
        None => absorb_small_clusters(raw),
    };
    Ok(renumber(raw))
}

/// Sends a short chunk to NeMo's stateful streaming endpoint.  The endpoint
/// retains the speaker cache for `session`, so its numeric labels stay stable.
pub fn live_turns(
    samples: &[f32],
    provider: &Provider,
    session: &str,
    offset_ms: i64,
    abort: &Abort,
) -> Result<Vec<Turn>, String> {
    if abort.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(crate::transcribe::CANCELLED.into());
    }
    let Provider::Remote { ip, api_key } = provider else {
        return Err("live diarization needs a remote server".into());
    };
    if api_key.trim().is_empty() {
        return Err("remote diarization needs an API key".into());
    }
    let url = format!(
        "{}?session={session}&offset_ms={offset_ms}",
        remote_url(ip, "live")?
    );
    let response = ureq::post(&url)
        .header("X-API-Key", api_key)
        .header(
            "Content-Type",
            "multipart/form-data; boundary=omarchy-meeting-recorder",
        )
        .send(multipart(samples, None, None).as_slice())
        .map_err(|e| format!("live diarization failed: {e}"))?;
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| format!("could not read live diarization result: {e}"))?;
    parse_live_turns(&text)
}

/// Sends the 16 kHz mono track to the compatible `/diarize` endpoint.
fn remote_turns(
    samples: &[f32],
    speakers: Option<usize>,
    ip: &str,
    api_key: &str,
    events: &Events,
    abort: &Abort,
) -> Result<Vec<Turn>, String> {
    if abort.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(crate::transcribe::CANCELLED.into());
    }
    if api_key.trim().is_empty() {
        return Err("remote diarization needs an API key".into());
    }
    let _ = events.send_blocking(Event::Stage("Finding speakers remotely".into()));
    let _ = events.send_blocking(Event::Progress(0.0));
    let body = multipart(samples, speakers, None);
    let response = ureq::post(&remote_url(ip, "diarize")?)
        .header("X-API-Key", api_key)
        .header(
            "Content-Type",
            "multipart/form-data; boundary=omarchy-meeting-recorder",
        )
        .send(body.as_slice())
        .map_err(|e| format!("remote diarization failed: {e}"))?;
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| format!("could not read remote diarization result: {e}"))?;
    let turns = parse_remote_turns(&text)?;
    let _ = events.send_blocking(Event::Progress(1.0));
    Ok(turns)
}

pub(crate) fn remote_url(ip: &str, route: &str) -> Result<String, String> {
    let ip: IpAddr = ip
        .parse()
        .map_err(|_| "remote server needs a valid IP address")?;
    let host = match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let port = if route == "live" { 8001 } else { 8000 };
    Ok(format!("http://{host}:{port}/{route}"))
}

pub(crate) fn multipart(
    samples: &[f32],
    speakers: Option<usize>,
    language: Option<&str>,
) -> Vec<u8> {
    const BOUNDARY: &str = "omarchy-meeting-recorder";
    let wav = wav(samples);
    let mut body = Vec::with_capacity(wav.len() + 512);
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(&wav);
    body.extend_from_slice(b"\r\n");
    if let Some(speakers) = speakers {
        body.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"num_speakers\"\r\n\r\n{speakers}\r\n").as_bytes(),
        );
    }
    if let Some(language) = language {
        body.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\n{language}\r\n").as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(WHISPER_RATE as u32).to_le_bytes());
    out.extend_from_slice(&(WHISPER_RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

fn parse_remote_turns(text: &str) -> Result<Vec<Turn>, String> {
    let result: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("invalid remote diarization result: {e}"))?;
    let segments = result["segments"]
        .as_array()
        .ok_or("remote diarization result has no segments")?;
    let mut labels = Vec::<String>::new();
    let mut raw = Vec::with_capacity(segments.len());
    for segment in segments {
        let label = segment["speaker"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| segment["speaker"].as_i64().map(|id| id.to_string()))
            .ok_or("remote diarization segment has no speaker")?;
        let start = segment["start"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or("remote diarization segment has an invalid start")?;
        let end = segment["end"]
            .as_f64()
            .filter(|n| n.is_finite() && *n > start)
            .ok_or("remote diarization segment has an invalid end")?;
        let speaker = labels
            .iter()
            .position(|known| known == &label)
            .unwrap_or_else(|| {
                labels.push(label);
                labels.len() - 1
            });
        raw.push((
            (start * 1000.0).round() as i64,
            (end * 1000.0).round() as i64,
            speaker as i32,
        ));
    }
    Ok(renumber(raw))
}

fn parse_live_turns(text: &str) -> Result<Vec<Turn>, String> {
    let result: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid live diarization result: {e}"))?;
    let segments = result["segments"]
        .as_array()
        .ok_or("live diarization result has no segments")?;
    let mut turns = Vec::with_capacity(segments.len());
    for segment in segments {
        let speaker = segment["speaker"]
            .as_u64()
            .or_else(|| {
                segment["speaker"]
                    .as_str()
                    .and_then(|value| value.parse().ok())
            })
            .ok_or("live diarization segment has an invalid speaker")?
            as usize;
        let start = segment["start"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or("live diarization segment has an invalid start")?;
        let end = segment["end"]
            .as_f64()
            .filter(|n| n.is_finite() && *n > start)
            .ok_or("live diarization segment has an invalid end")?;
        turns.push(Turn {
            start_ms: (start * 1000.0).round() as i64,
            end_ms: (end * 1000.0).round() as i64,
            speaker,
        });
    }
    turns.sort_by_key(|turn| (turn.start_ms, turn.speaker));
    Ok(turns)
}

/// Stretches where a speaker's probability is over one half, in ms. A frame
/// is 10 ms; pauses under half a second within one speaker are bridged and
/// blips under 0.3 seconds dropped, as the old diarization did.
fn segments(probs: &[f32], speakers: usize) -> Vec<(i64, i64, i32)> {
    let frames = probs.len() / speakers;
    let mut raw = Vec::new();
    for s in 0..speakers {
        let mut runs: Vec<(i64, i64)> = Vec::new();
        let mut start = None;
        for f in 0..=frames {
            let on = f < frames && probs[f * speakers + s] > 0.5;
            match (on, start) {
                (true, None) => start = Some(f),
                (false, Some(from)) => {
                    let (from, to) = (from as i64 * 10, f as i64 * 10);
                    match runs.last_mut() {
                        Some(last) if from - last.1 < 500 => last.1 = to,
                        _ => runs.push((from, to)),
                    }
                    start = None;
                }
                _ => {}
            }
        }
        raw.extend(
            runs.into_iter()
                .filter(|(from, to)| to - from >= 300)
                .map(|(from, to)| (from, to, s as i32)),
        );
    }
    raw
}

/// Keeps the `n` speakers with the most speech; the turns of the others go to
/// the nearest kept speaker.
fn keep_largest(raw: Vec<(i64, i64, i32)>, n: usize) -> Vec<(i64, i64, i32)> {
    let mut spoken = std::collections::HashMap::<i32, i64>::new();
    for (start, end, id) in &raw {
        *spoken.entry(*id).or_default() += end - start;
    }
    let mut ranked: Vec<(i32, i64)> = spoken.into_iter().collect();
    ranked.sort_by_key(|(id, ms)| (-ms, *id));
    let kept: Vec<i32> = ranked.iter().take(n.max(1)).map(|(id, _)| *id).collect();
    reassign(raw, |id| kept.contains(id))
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
    reassign(raw, |id| spoken.get(id).is_some_and(|ms| *ms >= floor))
}

/// Moves the turns of every speaker that `keeps` rejects to the speaker of
/// the nearest kept turn.
fn reassign(raw: Vec<(i64, i64, i32)>, keeps: impl Fn(&i32) -> bool) -> Vec<(i64, i64, i32)> {
    let anchors: Vec<(i64, i64, i32)> =
        raw.iter().copied().filter(|(_, _, id)| keeps(id)).collect();
    if anchors.is_empty() {
        return raw;
    }
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

/// `diarize <audio> [--speakers N]`: prints the speaker turns as JSON, for
/// comparing diarization engines on the same file.
pub fn cli(args: &[String]) -> gtk::glib::ExitCode {
    let mut path = None;
    let mut speakers = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--speakers" | "-s" => speakers = iter.next().and_then(|n| n.parse::<usize>().ok()),
            other => path = Some(PathBuf::from(other)),
        }
    }
    let Some(path) = path else {
        eprintln!("Usage: {} diarize <audio> [--speakers N]", crate::APP_NAME);
        return gtk::glib::ExitCode::from(2);
    };
    let result = crate::transcribe::load_track(&path).and_then(|samples| {
        let (events, _rx) = async_channel::unbounded();
        let started = std::time::Instant::now();
        let turns = turns(
            &samples,
            speakers,
            &Provider::Local,
            &events,
            &Abort::default(),
        )?;
        eprintln!(
            "{} turns in {:.1}s for {}s of audio",
            turns.len(),
            started.elapsed().as_secs_f64(),
            samples.len() / WHISPER_RATE
        );
        Ok(turns)
    });
    match result {
        Ok(turns) => {
            let json: Vec<serde_json::Value> = turns
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "speaker": t.speaker,
                        "start": t.start_ms as f64 / 1000.0,
                        "end": t.end_ms as f64 / 1000.0,
                    })
                })
                .collect();
            println!("{}", serde_json::Value::Array(json));
            gtk::glib::ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{}: {message}", crate::APP_NAME);
            gtk::glib::ExitCode::FAILURE
        }
    }
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

    #[test]
    fn remote_speakers_are_numbered_by_first_appearance() {
        let turns = parse_remote_turns(
            r#"{"segments":[{"speaker":"SPEAKER_01","start":2.0,"end":3.0},{"speaker":"SPEAKER_00","start":0.0,"end":1.0},{"speaker":"SPEAKER_01","start":4.0,"end":5.0}]}"#,
        )
        .unwrap();
        assert_eq!(
            turns.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 1, 1]
        );
    }

    #[test]
    fn automatic_remote_diarization_omits_the_speaker_count() {
        let automatic = multipart(&[], None, None);
        let fixed = multipart(&[], Some(3), None);
        let has = |body: &[u8], field: &[u8]| body.windows(field.len()).any(|part| part == field);
        assert!(!has(&automatic, b"num_speakers"));
        assert!(has(&fixed, b"name=\"num_speakers\"\r\n\r\n3"));
    }

    #[test]
    fn live_labels_are_not_renumbered_per_chunk() {
        let turns =
            parse_live_turns(r#"{"segments":[{"speaker":3,"start":2.0,"end":3.0}]}"#).unwrap();
        assert_eq!(turns[0].speaker, 3);
    }
}
