use crate::PlayStats;

#[derive(Debug, Default)]
pub struct PlayTracker {
    track_id: Option<String>,
    heard_seconds: f64,
    last_pos: Option<f64>,
    recorded: bool,
}

impl PlayTracker {
    pub fn reset(&mut self, track_id: Option<String>) {
        self.track_id = track_id;
        self.heard_seconds = 0.0;
        // A track starts at 0: the time before the first poll counts. A start further in (restored
        // session, resume) is a jump and is ignored like a seek.
        self.last_pos = Some(0.0);
        self.recorded = false;
    }

    /// Listening time needed: the C# threshold less one poll gap, since the polls never land exactly
    /// on the end (short tracks have to be heard in full, which the summed poll deltas never reach).
    fn needed(duration: f64) -> f64 {
        let full = PlayStats::threshold(duration);
        (full - Self::POLL_SLACK_S).max(full * 0.5)
    }

    /// The longest position poll interval (power-saver tier).
    const POLL_SLACK_S: f64 = 1.0;

    /// Accumulates active listening time without letting seeks trigger spurious play counts.
    pub fn on_time_pos(&mut self, pos: f64, duration: f64, is_playing: bool) -> Option<String> {
        let id = self.track_id.as_ref()?;
        if !is_playing || self.recorded {
            self.last_pos = Some(pos);
            return None;
        }
        if let Some(prev) = self.last_pos {
            let delta = pos - prev;
            if delta > 0.0 && delta <= 2.5 {
                self.heard_seconds += delta;
            }
        }
        self.last_pos = Some(pos);
        if self.heard_seconds >= Self::needed(duration) {
            self.recorded = true;
            Some(id.clone())
        } else {
            None
        }
    }
}
