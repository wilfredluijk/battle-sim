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
    pub command: Option<PendingCommand>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandSlot {
    pub window: Arc<Mutex<Window>>,
    pub notify: Arc<Notify>,
}

impl CommandSlot {
    pub fn submit(
        &self,
        match_id: &str,
        command: PendingCommand,
        received: Instant,
    ) -> Result<(), &'static str> {
        let mut window = self.window.lock().expect("command window poisoned");
        if window.cutoff.is_none() {
            return Err("not_active");
        }
        if match_id != window.match_id {
            return Err("wrong_match");
        }
        if command.tick != window.tick {
            return Err("wrong_tick");
        }
        if received > window.cutoff.expect("checked") {
            return Err("late_command");
        }
        if window.command.is_some() {
            return Err("duplicate_command");
        }
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
        };
    }

    pub fn close(&self) -> Option<PendingCommand> {
        let mut window = self.window.lock().expect("command window poisoned");
        window.cutoff = None;
        window.command.take()
    }
}
