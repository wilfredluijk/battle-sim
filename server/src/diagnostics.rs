//! Bounded, observational network measurements. Never inputs to the simulation.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::Instant;

const SAMPLES: usize = 512;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TimingSummary {
    pub samples: usize,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub max_ms: Option<f64>,
}

fn summary(values: &VecDeque<f64>) -> TimingSummary {
    let mut sorted: Vec<_> = values.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: usize| {
        sorted
            .get((sorted.len() * p).div_ceil(100).saturating_sub(1))
            .copied()
    };
    TimingSummary {
        samples: sorted.len(),
        p50_ms: percentile(50),
        p95_ms: percentile(95),
        max_ms: sorted.last().copied(),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TeamDiagnostics {
    pub accepted: u64,
    pub late: u64,
    pub wrong_tick: u64,
    pub other_rejected: u64,
    pub completed_windows: u64,
    pub missed_windows: u64,
    pub late_windows: u64,
    pub response: TimingSummary,
    pub rtt: TimingSummary,
    pub rtt_age_seconds: Option<f64>,
}

#[derive(Debug, Default)]
pub struct Measurements {
    pub counters: TeamDiagnostics,
    response: VecDeque<f64>,
    rtt: VecDeque<f64>,
    last_pong: Option<Instant>,
}

impl Measurements {
    pub fn response(&mut self, ms: f64) {
        push(&mut self.response, ms);
    }
    pub fn pong(&mut self, sent: Instant, received: Instant) {
        push(
            &mut self.rtt,
            received.saturating_duration_since(sent).as_secs_f64() * 1000.0,
        );
        self.last_pong = Some(received);
    }
    pub fn reset_match(&mut self) {
        self.counters = TeamDiagnostics::default();
        self.response.clear();
    }
    pub fn snapshot(&self) -> TeamDiagnostics {
        TeamDiagnostics {
            response: summary(&self.response),
            rtt: summary(&self.rtt),
            rtt_age_seconds: self.last_pong.map(|t| t.elapsed().as_secs_f64()),
            ..self.counters.clone()
        }
    }
}

fn push(samples: &mut VecDeque<f64>, value: f64) {
    if samples.len() == SAMPLES {
        samples.pop_front();
    }
    samples.push_back(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_nearest_rank_percentiles_and_match_reset() {
        let mut m = Measurements::default();
        for i in 1..=600 {
            m.response(f64::from(i));
        }
        let s = m.snapshot();
        assert_eq!(s.response.samples, 512);
        assert_eq!(s.response.p50_ms, Some(344.0));
        assert_eq!(s.response.p95_ms, Some(575.0));
        let now = Instant::now();
        m.pong(now, now + std::time::Duration::from_millis(12));
        m.reset_match();
        assert_eq!(m.snapshot().response.samples, 0);
        assert_eq!(m.snapshot().rtt.p50_ms, Some(12.0));
    }
}
