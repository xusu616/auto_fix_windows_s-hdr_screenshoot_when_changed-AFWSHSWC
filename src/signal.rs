use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    DisplaySdr,
    Highlights,
    HdrPlayback,
    WithinSdrRange,
    Unknown,
}

const HOLD_HDR: Duration = Duration::from_secs(5);

pub struct Signal {
    last_highlights: Option<Instant>,
}

impl Signal {
    pub fn new() -> Self {
        Self {
            last_highlights: None,
        }
    }

    pub fn correction(&mut self, now: Instant, evidence: Evidence) -> Option<bool> {
        match evidence {
            Evidence::DisplaySdr => {
                self.last_highlights = None;
                Some(false)
            }
            Evidence::Highlights | Evidence::HdrPlayback => {
                self.last_highlights = Some(now);
                Some(true)
            }
            Evidence::WithinSdrRange => Some(
                self.last_highlights
                    .is_some_and(|last| now.duration_since(last) < HOLD_HDR),
            ),
            Evidence::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_for_content_even_when_the_display_stays_in_hdr_mode() {
        let mut signal = Signal::new();
        let start = Instant::now();
        assert_eq!(
            signal.correction(start, Evidence::WithinSdrRange),
            Some(false)
        );
        assert_eq!(signal.correction(start, Evidence::Highlights), Some(true));
        assert_eq!(
            signal.correction(start + Duration::from_secs(1), Evidence::WithinSdrRange),
            Some(true)
        );
        assert_eq!(
            signal.correction(start + HOLD_HDR, Evidence::WithinSdrRange),
            Some(false)
        );
    }

    #[test]
    fn browser_hdr_playback_enables_correction_even_during_a_dark_scene() {
        let mut signal = Signal::new();
        let start = Instant::now();
        assert_eq!(signal.correction(start, Evidence::HdrPlayback), Some(true));
        assert_eq!(
            signal.correction(start + HOLD_HDR, Evidence::WithinSdrRange),
            Some(false)
        );
    }

    #[test]
    fn unknown_capture_keeps_the_setting_and_sdr_output_clears_the_hold() {
        let mut signal = Signal::new();
        let start = Instant::now();
        assert_eq!(signal.correction(start, Evidence::Highlights), Some(true));
        assert_eq!(signal.correction(start, Evidence::Unknown), None);
        assert_eq!(signal.correction(start, Evidence::DisplaySdr), Some(false));
        assert_eq!(
            signal.correction(start, Evidence::WithinSdrRange),
            Some(false)
        );
    }
}
