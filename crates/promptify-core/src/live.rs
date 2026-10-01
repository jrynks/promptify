//! Live transcription while the user is still speaking: audio is cut at natural pauses, finished
//! chunks are transcribed in the background, and only the uncommitted tail is left for the end.

use crate::audio::TARGET_SAMPLE_RATE;

const RATE: usize = TARGET_SAMPLE_RATE as usize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkPolicy {
    /// Analysis frame for loudness.
    pub frame: usize,
    /// Below this RMS a frame counts as silence.
    pub silence_rms: f32,
    /// A pause must last this long to end a chunk.
    pub pause: usize,
    /// Chunks shorter than this are not committed; whisper is less accurate on very short audio.
    pub min_chunk: usize,
    /// Without a pause, a chunk is cut here so whisper's 30 s window is never exceeded.
    pub max_chunk: usize,
}

impl Default for ChunkPolicy {
    fn default() -> Self {
        Self { frame: RATE / 50, silence_rms: 0.012, pause: RATE * 3 / 5, min_chunk: RATE * 3, max_chunk: RATE * 25 }
    }
}

/// Returns the length of the next chunk at the start of `audio`, cut in the middle of the first
/// long enough pause after `min_chunk`, or at `max_chunk` when there is none. `None` means wait.
pub fn next_cut(audio: &[f32], policy: &ChunkPolicy) -> Option<usize> {
    let frame = policy.frame.max(1);
    let mut silent_from: Option<usize> = None;
    for start in (0..audio.len().saturating_sub(frame - 1)).step_by(frame) {
        let chunk = &audio[start..start + frame];
        let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / frame as f32).sqrt();
        if rms < policy.silence_rms {
            let from = *silent_from.get_or_insert(start);
            let end = start + frame;
            let cut = from + (end - from) / 2;
            if end - from >= policy.pause && cut >= policy.min_chunk && from > 0 {
                return Some(cut);
            }
        } else {
            silent_from = None;
        }
        if start + frame >= policy.max_chunk {
            return Some(policy.max_chunk);
        }
    }
    None
}

/// True when any frame is louder than silence. Whisper tends to invent words for silent audio.
pub fn has_speech(audio: &[f32], policy: &ChunkPolicy) -> bool {
    let frame = policy.frame.max(1);
    audio.chunks(frame).any(|chunk| (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt() >= policy.silence_rms)
}

/// Text committed so far and how much audio it covers.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct LiveTranscript {
    committed_samples: usize,
    pieces: Vec<String>,
}

impl LiveTranscript {
    pub fn committed_samples(&self) -> usize {
        self.committed_samples
    }

    pub fn text(&self) -> String {
        join(&self.pieces)
    }

    /// Records that the next `samples` of audio were transcribed as `text`.
    pub fn commit(&mut self, samples: usize, text: &str) {
        self.committed_samples += samples;
        let text = text.trim();
        if !text.is_empty() {
            self.pieces.push(text.to_owned());
        }
    }

    /// The full transcript once the remaining audio is transcribed.
    pub fn finish(&self, tail: &str) -> String {
        let mut pieces = self.pieces.clone();
        if !tail.trim().is_empty() {
            pieces.push(tail.trim().to_owned());
        }
        join(&pieces)
    }
}

fn join(pieces: &[String]) -> String {
    pieces.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32) -> Vec<f32> {
        (0..(seconds * RATE as f32) as usize).map(|i| (i as f32 * 0.05).sin() * 0.3).collect()
    }

    fn quiet(seconds: f32) -> Vec<f32> {
        vec![0.001; (seconds * RATE as f32) as usize]
    }

    fn concat(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    #[test]
    fn cuts_in_the_middle_of_the_first_long_pause_after_min_chunk() {
        let p = ChunkPolicy::default();
        let audio = concat(&[tone(3.5), quiet(0.8), tone(1.0)]);
        let cut = next_cut(&audio, &p).expect("pause found");
        let pause_start = (3.5 * RATE as f32) as usize;
        assert!(cut > pause_start && cut < pause_start + RATE * 8 / 10, "{cut}");
    }

    #[test]
    fn waits_for_short_speech_short_pauses_and_leading_silence() {
        let p = ChunkPolicy::default();
        assert_eq!(next_cut(&concat(&[tone(2.0), quiet(0.8), tone(0.5)]), &p), None, "chunk shorter than min_chunk");
        assert_eq!(next_cut(&concat(&[tone(4.0), quiet(0.4), tone(1.0)]), &p), None, "comma-length pause");
        assert_eq!(next_cut(&concat(&[quiet(8.0)]), &p), None, "silence alone is not a chunk");
        assert_eq!(next_cut(&tone(4.0), &p), None, "still speaking");
    }

    #[test]
    fn forces_a_cut_at_max_chunk_without_pauses() {
        let p = ChunkPolicy::default();
        assert_eq!(next_cut(&tone(26.0), &p), Some(p.max_chunk));
        assert_eq!(next_cut(&tone(24.0), &p), None);
    }

    #[test]
    fn silent_tails_have_no_speech() {
        let p = ChunkPolicy::default();
        assert!(!has_speech(&quiet(1.5), &p));
        assert!(!has_speech(&[], &p));
        assert!(has_speech(&concat(&[quiet(1.0), tone(0.1)]), &p));
    }

    #[test]
    fn transcript_joins_committed_chunks_and_tail() {
        let mut live = LiveTranscript::default();
        live.commit(32_000, " First part. ");
        live.commit(8_000, "  ");
        live.commit(16_000, "second part");
        assert_eq!(live.committed_samples(), 56_000);
        assert_eq!(live.text(), "First part. second part");
        assert_eq!(live.finish(" and the end "), "First part. second part and the end");
        assert_eq!(live.finish(""), "First part. second part");
    }
}
