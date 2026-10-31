use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) struct Change {
    pub revision: u64,
    pub value: f64,
    changed: Instant,
    queued: Instant,
    flush: bool,
}

#[derive(Default)]
pub(super) struct Control {
    revision: u64,
    sent: u64,
    pending: Option<Change>,
}
impl Control {
    pub fn target(&self) -> Option<f64> {
        self.pending.map(|p| p.value)
    }
    pub fn request(&mut self, value: f64, now: Instant) {
        if !value.is_finite() || self.target() == Some(value) {
            return;
        }
        let queued = self
            .pending
            .filter(|p| p.revision != self.sent)
            .map(|p| p.queued)
            .unwrap_or(now);
        self.revision += 1;
        self.pending = Some(Change {
            revision: self.revision,
            value,
            changed: now,
            queued,
            flush: false,
        });
    }
    pub fn flush(&mut self) {
        if let Some(p) = &mut self.pending {
            p.flush = true;
        }
    }
    pub fn ready(
        &mut self,
        now: Instant,
        debounce: Duration,
        max_wait: Duration,
    ) -> Option<Change> {
        let p = self.pending?;
        if p.revision == self.sent
            || (!p.flush
                && now.duration_since(p.changed) < debounce
                && now.duration_since(p.queued) < max_wait)
        {
            return None;
        }
        self.sent = p.revision;
        Some(p)
    }
    pub fn finish(&mut self, revision: u64) {
        if self.pending.is_some_and(|p| p.revision == revision) {
            self.pending = None;
        }
    }
}

pub(super) struct Flight {
    pub change: Change,
    pub command: u64,
    pub replied: bool,
    pub failed: bool,
    seek_seen: bool,
    restarted: bool,
}
impl Flight {
    pub fn new(change: Change, command: u64) -> Self {
        Self {
            change,
            command,
            replied: false,
            failed: false,
            seek_seen: false,
            restarted: false,
        }
    }
    pub fn event(&mut self, kind: i32, command: u64, error: i32) {
        match kind {
            5 if command == self.command => {
                self.replied = true;
                self.failed = error < 0;
            }
            20 => self.seek_seen = true,
            21 if self.seek_seen => self.restarted = true,
            _ => {}
        }
    }
    pub fn seek_ready(&self) -> bool {
        self.replied && self.restarted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const DEBOUNCE: Duration = Duration::from_millis(70);
    const MAX_WAIT: Duration = Duration::from_millis(140);
    #[test]
    fn drag_debounces_but_keeps_loading_during_continuous_movement() {
        let now = Instant::now();
        let mut c = Control::default();
        for ms in [0, 20, 40, 60, 80, 100, 120, 140] {
            c.request(ms as f64, now + Duration::from_millis(ms));
            let ready = c.ready(now + Duration::from_millis(ms), DEBOUNCE, MAX_WAIT);
            if ms < 140 {
                assert!(ready.is_none());
            } else {
                assert_eq!(ready.unwrap().value, 140.);
            }
        }
        c.request(200., now + Duration::from_millis(150));
        assert!(
            c.ready(now + Duration::from_millis(180), DEBOUNCE, MAX_WAIT)
                .is_none()
        );
        assert_eq!(
            c.ready(now + Duration::from_millis(220), DEBOUNCE, MAX_WAIT)
                .unwrap()
                .value,
            200.
        );
    }
    #[test]
    fn late_completion_cannot_replace_newer_seek_or_volume_and_release_is_not_duplicated() {
        let now = Instant::now();
        let mut c = Control::default();
        c.request(20., now);
        c.flush();
        let a = c.ready(now, DEBOUNCE, MAX_WAIT).unwrap();
        c.request(80., now);
        c.flush();
        c.finish(a.revision);
        assert_eq!(c.target(), Some(80.));
        let b = c.ready(now, DEBOUNCE, MAX_WAIT).unwrap();
        c.flush();
        assert!(c.ready(now, DEBOUNCE, MAX_WAIT).is_none());
        c.finish(a.revision);
        assert_eq!(c.target(), Some(80.));
        c.finish(b.revision);
        assert_eq!(c.target(), None);
    }
    #[test]
    fn startup_restart_and_unrelated_replies_do_not_complete_a_seek() {
        let mut c = Control::default();
        let now = Instant::now();
        c.request(50., now);
        c.flush();
        let mut f = Flight::new(c.ready(now, DEBOUNCE, MAX_WAIT).unwrap(), 42);
        f.event(21, 0, 0);
        f.event(5, 41, 0);
        assert!(!f.seek_ready());
        f.event(20, 0, 0);
        f.event(21, 0, 0);
        assert!(!f.seek_ready());
        f.event(5, 42, 0);
        assert!(f.seek_ready());
    }
}
