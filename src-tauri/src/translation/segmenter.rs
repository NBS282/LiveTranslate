/// Samples per 30ms frame at 16 kHz mono.
pub const FRAME_SAMPLES_16K: usize = 480;

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
        }
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
        let silence_triggered = !voiced && self.trailing_silence >= self.silence_close;
        let max_triggered = buf_frames >= self.max_frames;

        if silence_triggered || max_triggered {
            let segment = std::mem::take(&mut self.buf);
            let voiced_count = self.voiced_count;
            self.in_speech = false;
            self.voiced_count = 0;
            self.trailing_silence = 0;
            if voiced_count >= self.min_voiced {
                return Some(segment);
            }
        }

        None
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
}
