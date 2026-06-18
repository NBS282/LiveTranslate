/// Samples per 30ms frame at 16 kHz mono.
pub const FRAME_SAMPLES_16K: usize = 480;

/// Closes a speech segment after enough trailing silence; drops too-short segments.
/// Fed one fixed-size frame at a time with its VAD voiced/unvoiced flag.
pub struct Segmenter {
    silence_close: u32,
    min_voiced: u32,
    in_speech: bool,
    voiced_count: u32,
    trailing_silence: u32,
    buf: Vec<i16>,
}

impl Segmenter {
    pub fn new(silence_close: u32, min_voiced: u32) -> Self {
        Self {
            silence_close,
            min_voiced,
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
            return None;
        }
        if !self.in_speech {
            return None;
        }
        self.buf.extend_from_slice(frame);
        self.trailing_silence += 1;
        if self.trailing_silence < self.silence_close {
            return None;
        }
        let segment = std::mem::take(&mut self.buf);
        let voiced_count = self.voiced_count;
        self.in_speech = false;
        self.voiced_count = 0;
        self.trailing_silence = 0;
        if voiced_count >= self.min_voiced {
            Some(segment)
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
        let mut s = Segmenter::new(3, 2);
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
        let mut s = Segmenter::new(2, 3);
        s.push(&frame(), true);
        s.push(&frame(), false);
        let seg = s.push(&frame(), false);
        assert!(seg.is_none());
    }

    #[test]
    fn ignores_silence_outside_speech() {
        let mut s = Segmenter::new(2, 1);
        assert!(s.push(&frame(), false).is_none());
        assert!(s.push(&frame(), false).is_none());
    }

    #[test]
    fn second_phrase_after_first_closes() {
        let mut s = Segmenter::new(2, 1);
        s.push(&frame(), true);
        s.push(&frame(), false);
        assert!(s.push(&frame(), false).is_some());
        s.push(&frame(), true);
        s.push(&frame(), false);
        assert!(s.push(&frame(), false).is_some());
    }
}
