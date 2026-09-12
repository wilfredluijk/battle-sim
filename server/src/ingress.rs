//! One bounded command slot per connection. No command traffic enters the room queue.
use crate::room::PendingCommand;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::Notify;

#[derive(Debug, Default)]
pub struct Window {
    pub match_id: String,
    pub tick: u64,
    pub cutoff: Option<Instant>,
    pub sent_at: Option<Instant>,
    pub sampled: bool,
    pub late_received: bool,
    pub command: Option<PendingCommand>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandSlot {
    pub window: Arc<Mutex<Window>>,
    pub notify: Arc<Notify>,
    pub measurements: Arc<Mutex<crate::diagnostics::Measurements>>,
}

impl CommandSlot {
    pub fn submit(
        &self,
        match_id: &str,
        command: PendingCommand,
        received: Instant,
    ) -> Result<(), &'static str> {
        let mut window = self.window.lock().expect("command window poisoned");
        let mut measurements = self.measurements.lock().expect("measurements poisoned");
        let rejection = if window.cutoff.is_none() {
            Some("not_active")
        } else if match_id != window.match_id {
            Some("wrong_match")
        } else if command.tick != window.tick {
            Some("wrong_tick")
        } else if window.cutoff.is_some_and(|cutoff| received > cutoff) {
            Some("late_command")
        } else if window.command.is_some() {
            Some("duplicate_command")
        } else {
            None
        };
        if window.cutoff.is_some()
            && match_id == window.match_id
            && command.tick == window.tick
            && !window.sampled
        {
            if let Some(sent) = window.sent_at {
                measurements
                    .response(received.saturating_duration_since(sent).as_secs_f64() * 1000.0);
                window.sampled = true;
            }
        }
        if let Some(reason) = rejection {
            match reason {
                "late_command" => {
                    measurements.counters.late += 1;
                    if window.command.is_none() {
                        window.late_received = true;
                    }
                }
                "wrong_tick" => measurements.counters.wrong_tick += 1,
                _ => measurements.counters.other_rejected += 1,
            }
            return Err(reason);
        }
        measurements.counters.accepted += 1;
        crate::metrics::ACCEPTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        window.command = Some(command);
        self.notify.notify_one();
        Ok(())
    }

    pub fn open(&self, match_id: &str, tick: u64, cutoff: Instant) {
        *self.window.lock().expect("command window poisoned") = Window {
            match_id: match_id.into(),
            tick,
            cutoff: Some(cutoff),
            command: None,
            sent_at: None,
            sampled: false,
            late_received: false,
        };
    }

    pub fn sent(&self, tick: u64, at: Instant) {
        let mut w = self.window.lock().expect("command window poisoned");
        if w.tick == tick && w.cutoff.is_some() {
            w.sent_at = Some(at);
        }
    }

    pub fn diagnostics(&self) -> crate::diagnostics::TeamDiagnostics {
        self.measurements
            .lock()
            .expect("measurements poisoned")
            .snapshot()
    }

    pub fn close(&self) -> Option<PendingCommand> {
        let mut window = self.window.lock().expect("command window poisoned");
        if window.cutoff.is_some() {
            let mut m = self.measurements.lock().expect("measurements poisoned");
            m.counters.completed_windows += 1;
            if window.late_received && window.command.is_none() {
                m.counters.late_windows += 1;
            }
            if window.command.is_none() {
                m.counters.missed_windows += 1;
            }
        }
        window.cutoff = None;
        window.command.take()
    }
}

#[cfg(test)]
mod diagnostics_tests {
    use super::*;
    use std::time::Duration;
    fn command(tick: u64) -> PendingCommand {
        PendingCommand {
            tick,
            throttle: 0.0,
            rudder: 0.0,
            sensor_mode: crate::protocol::SensorMode::Passive,
            fire: None,
            activate_powerup: None,
        }
    }
    #[test]
    fn diagnostic_rejections_do_not_change_admission_or_count_duplicates_as_missing() {
        let slot = CommandSlot::default();
        let sent = Instant::now();
        slot.open("match", 1, sent + Duration::from_millis(80));
        slot.sent(1, sent);
        assert!(slot
            .submit("match", command(1), sent + Duration::from_millis(25))
            .is_ok());
        assert_eq!(
            slot.submit("match", command(1), sent + Duration::from_millis(30)),
            Err("duplicate_command")
        );
        assert!(slot.close().is_some());
        slot.close();
        slot.open("match", 2, sent + Duration::from_millis(80));
        slot.sent(2, sent);
        assert_eq!(
            slot.submit("match", command(2), sent + Duration::from_millis(90)),
            Err("late_command")
        );
        assert_eq!(
            slot.submit("match", command(1), sent + Duration::from_millis(91)),
            Err("wrong_tick")
        );
        assert!(slot.close().is_none());
        let d = slot.diagnostics();
        assert_eq!(
            (d.accepted, d.late, d.wrong_tick, d.other_rejected),
            (1, 1, 1, 1)
        );
        assert_eq!((d.completed_windows, d.missed_windows), (2, 1));
        assert_eq!(d.response.samples, 2);
        assert_eq!(d.response.p95_ms, Some(90.0));
    }
}
