/// Samples per 30ms frame at 16 kHz mono.
pub const FRAME_SAMPLES_16K: usize = 480;

/// Trailing-silence frame count that closes a segment when `fast_close` is armed
/// (~90ms at 30ms/frame) — short enough to catch a real inter-sentence micro-pause
/// without cutting mid-word.
pub const FAST_CLOSE_FRAMES: u32 = 3;

/// Closes a speech segment after enough trailing silence or after `max_frames` of
/// accumulated audio, whichever comes first. Drops segments shorter than `min_voiced`.
/// Fed one fixed-size frame at a time with its VAD voiced/unvoiced flag.
pub struct Segmenter {
    silence_close: u32,
    min_voiced: u32,
    max_frames: u32,
    in_speech: bool,
    voiced_count: u32,
    trailing_silence: u32,
    buf: Vec<i16>,
    /// Armed by the caller (e.g. the partial-decode thread) when the in-progress
    /// translation just ended a sentence. Shortens the trailing-silence threshold
    /// to `FAST_CLOSE_FRAMES` so the NEXT brief VAD dip closes the segment at a
    /// real micro-pause instead of waiting for the full `silence_close` window.
    /// Auto-resets to `false` on every close path.
    fast_close: bool,
}

impl Segmenter {
    pub fn new(silence_close: u32, min_voiced: u32, max_frames: u32) -> Self {
        Self {
            silence_close,
            min_voiced,
            max_frames,
            in_speech: false,
            voiced_count: 0,
            trailing_silence: 0,
            buf: Vec::new(),
            fast_close: false,
        }
    }

    /// Arms (or disarms) the fast-close window. When armed, the next
    /// `FAST_CLOSE_FRAMES` trailing-silence frames close the segment instead of
    /// waiting for the full `silence_close` count. Auto-resets on every close.
    pub fn set_fast_close(&mut self, on: bool) {
        self.fast_close = on;
    }

    /// Push one frame. Returns Some(samples) when a phrase closes (and passes the min-length gate).
    pub fn push(&mut self, frame: &[i16], voiced: bool) -> Option<Vec<i16>> {
        if voiced {
            self.in_speech = true;
            self.trailing_silence = 0;
            self.voiced_count += 1;
            self.buf.extend_from_slice(frame);
        } else {
            if !self.in_speech {
                return None;
            }
            self.buf.extend_from_slice(frame);
            self.trailing_silence += 1;
        }

        let buf_frames = (self.buf.len() / FRAME_SAMPLES_16K) as u32;
        let silence_close = if self.fast_close {
            FAST_CLOSE_FRAMES.min(self.silence_close)
        } else {
            self.silence_close
        };
        let silence_triggered = !voiced && self.trailing_silence >= silence_close;
        let max_triggered = buf_frames >= self.max_frames;

        if silence_triggered || max_triggered {
            let segment = std::mem::take(&mut self.buf);
            let voiced_count = self.voiced_count;
            self.in_speech = false;
            self.voiced_count = 0;
            self.trailing_silence = 0;
            self.fast_close = false;
            if voiced_count >= self.min_voiced {
                return Some(segment);
            }
        }

        None
    }

    /// Copy of the open segment while speech is in progress (None otherwise).
    pub fn snapshot(&self) -> Option<Vec<i16>> {
        if self.in_speech && !self.buf.is_empty() {
            Some(self.buf.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> Vec<i16> {
        vec![0i16; FRAME_SAMPLES_16K]
    }

    #[test]
    fn closes_phrase_after_silence() {
        let mut s = Segmenter::new(3, 2, 1000);
        assert!(s.push(&frame(), true).is_none());
        assert!(s.push(&frame(), true).is_none());
        assert!(s.push(&frame(), false).is_none());
        assert!(s.push(&frame(), false).is_none());
        let seg = s.push(&frame(), false);
        assert!(seg.is_some());
        assert_eq!(seg.unwrap().len(), 5 * FRAME_SAMPLES_16K);
    }

    #[test]
    fn drops_too_short_segment() {
        let mut s = Segmenter::new(2, 3, 1000);
        s.push(&frame(), true);
        s.push(&frame(), false);
        let seg = s.push(&frame(), false);
        assert!(seg.is_none());
    }

    #[test]
    fn ignores_silence_outside_speech() {
        let mut s = Segmenter::new(2, 1, 1000);
        assert!(s.push(&frame(), false).is_none());
        assert!(s.push(&frame(), false).is_none());
    }

    #[test]
    fn second_phrase_after_first_closes() {
        let mut s = Segmenter::new(2, 1, 1000);
        s.push(&frame(), true);
        s.push(&frame(), false);
        assert!(s.push(&frame(), false).is_some());
        s.push(&frame(), true);
        s.push(&frame(), false);
        assert!(s.push(&frame(), false).is_some());
    }

    #[test]
    fn force_cuts_at_max_frames_during_voiced() {
        // max_frames=4, silence_close=10 (won't trigger by silence)
        let mut s = Segmenter::new(10, 2, 4);
        assert!(s.push(&frame(), true).is_none());
        assert!(s.push(&frame(), true).is_none());
        assert!(s.push(&frame(), true).is_none());
        let seg = s.push(&frame(), true); // 4th frame → max_frames hit
        assert!(seg.is_some(), "expected force cut at max_frames");
        assert_eq!(seg.unwrap().len(), 4 * FRAME_SAMPLES_16K);
        // State reset — next frames start a new segment
        assert!(s.push(&frame(), true).is_none());
    }

    #[test]
    fn force_cuts_during_silence_before_silence_close() {
        // max_frames=3, silence_close=10. 2 voiced + 1 silence = 3 frames → max cut
        let mut s = Segmenter::new(10, 2, 3);
        assert!(s.push(&frame(), true).is_none());
        assert!(s.push(&frame(), true).is_none());
        let seg = s.push(&frame(), false); // 3rd frame → max_frames hit mid-silence
        assert!(seg.is_some(), "expected force cut during silence");
        assert_eq!(seg.unwrap().len(), 3 * FRAME_SAMPLES_16K);
    }

    #[test]
    fn max_cut_below_min_voiced_returns_none() {
        // max_frames=2, min_voiced=3. Cut happens but segment is too short.
        let mut s = Segmenter::new(10, 3, 2);
        assert!(s.push(&frame(), true).is_none());
        let seg = s.push(&frame(), true); // 2nd frame → max_frames hit, but voiced=2 < min=3
        assert!(seg.is_none(), "should drop segment below min_voiced");
    }

    #[test]
    fn snapshot_returns_open_buffer_only_during_speech() {
        let mut s = Segmenter::new(3, 1, 1000);
        assert!(s.snapshot().is_none());
        s.push(&frame(), true);
        s.push(&frame(), true);
        let snap = s.snapshot().expect("open segment should snapshot");
        assert_eq!(snap.len(), 2 * FRAME_SAMPLES_16K);
    }

    #[test]
    fn fast_close_shortens_silence_window() {
        let mut s = Segmenter::new(8, 1, 1000);
        s.push(&frame(), true);
        s.set_fast_close(true);
        s.push(&frame(), false);
        s.push(&frame(), false);
        assert!(
            s.push(&frame(), false).is_some(),
            "3 silence frames close when armed"
        );
    }

    #[test]
    fn fast_close_resets_after_segment_closes() {
        let mut s = Segmenter::new(8, 1, 1000);
        s.push(&frame(), true);
        s.set_fast_close(true);
        for _ in 0..2 {
            s.push(&frame(), false);
        }
        assert!(s.push(&frame(), false).is_some());
        // New segment: 3 silence frames must NOT close it anymore.
        s.push(&frame(), true);
        for _ in 0..3 {
            assert!(s.push(&frame(), false).is_none());
        }
    }
}
