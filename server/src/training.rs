//! Administrator-only session history and replay notes. Atomic persistence outside sim/.
use crate::room::MatchReport;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scoring {
    pub win: u32,
    pub draw: u32,
    pub loss: u32,
}
impl Default for Scoring {
    fn default() -> Self {
        Self {
            win: 3,
            draw: 1,
            loss: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundTag {
    pub session_id: String,
    pub session_name: String,
    pub number: usize,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    pub match_id: String,
    pub name: String,
    pub started_at: u64,
    pub config_hash: String,
    pub teams: Vec<String>,
    pub status: String,
    pub report: Option<MatchReport>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingSession {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub expected_teams: Vec<String>,
    pub scoring: Scoring,
    pub next_round: String,
    pub rounds: Vec<Round>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bookmark {
    pub tick: u64,
    pub label: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Debrief {
    pub notes: String,
    pub bookmarks: Vec<Bookmark>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingData {
    pub version: u32,
    pub revision: u64,
    pub active_session: Option<String>,
    pub sessions: Vec<TrainingSession>,
    pub debriefs: BTreeMap<String, Debrief>,
}
impl Default for TrainingData {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            active_session: None,
            sessions: vec![],
            debriefs: BTreeMap::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct TrainingSnapshot {
    #[serde(flatten)]
    pub data: TrainingData,
    pub storage_error: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct TrainingRequest {
    pub revision: u64,
    #[serde(flatten)]
    pub action: TrainingAction,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrainingAction {
    Create {
        name: String,
        expected_teams: Vec<String>,
        scoring: Scoring,
    },
    Activate {
        session_id: Option<String>,
    },
    Configure {
        name: String,
        expected_teams: Vec<String>,
        scoring: Scoring,
        next_round: String,
    },
    Debrief {
        replay_id: String,
        debrief: Debrief,
    },
}

#[derive(Debug, Default)]
pub struct TrainingStore {
    pub data: TrainingData,
    path: Option<PathBuf>,
    pub error: Option<String>,
    blocked: bool,
    dirty: bool,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn label(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn validate_settings(name: &str, teams: &[String], scoring: &Scoring) -> Result<(), String> {
    if !label(name, 120) || teams.len() > 32 || teams.iter().any(|t| !label(t, 64)) {
        return Err(
            "Use a session name up to 120 bytes and at most 32 team names of 64 bytes.".into(),
        );
    }
    let mut distinct = teams.to_vec();
    distinct.sort();
    distinct.dedup();
    if distinct.len() != teams.len() {
        return Err("Expected team names must be unique.".into());
    }
    if [scoring.win, scoring.draw, scoring.loss]
        .iter()
        .any(|v| *v > 1000)
    {
        return Err("Training points must be integers between 0 and 1000.".into());
    }
    Ok(())
}
impl TrainingStore {
    pub fn load(path: PathBuf) -> Self {
        let mut store = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        let loaded = (|| -> Result<TrainingData, String> {
            if !path.exists() {
                return Ok(TrainingData::default());
            }
            if fs::metadata(&path)
                .map_err(|_| "Cannot inspect training history")?
                .len()
                > 32 * 1024 * 1024
            {
                return Err("Training history exceeds the 32 MiB limit".into());
            }
            let bytes = fs::read(&path).map_err(|_| "Cannot read training history")?;
            let data: TrainingData = serde_json::from_slice(&bytes)
                .map_err(|_| "Training history is invalid; restore its backup before saving")?;
            if data.version != 1 {
                return Err("Unsupported training history version".into());
            }
            Ok(data)
        })();
        match loaded {
            Ok(mut data) => {
                for session in &mut data.sessions {
                    for round in &mut session.rounds {
                        if round.status == "running" {
                            round.status = "interrupted".into();
                            store.dirty = true;
                        }
                    }
                }
                if store.dirty {
                    data.revision += 1;
                }
                store.data = data;
                if let Err(e) = store.flush() {
                    store.error = Some(e);
                }
            }
            Err(e) => {
                store.error = Some(e);
                store.blocked = true;
            }
        }
        store
    }
    pub fn snapshot(&self) -> TrainingSnapshot {
        TrainingSnapshot {
            data: self.data.clone(),
            storage_error: self.error.clone(),
        }
    }
    pub fn active(&self) -> Option<&TrainingSession> {
        self.data
            .sessions
            .iter()
            .find(|s| Some(&s.id) == self.data.active_session.as_ref())
    }
    fn write(&self, data: &TrainingData) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(data).map_err(|_| "Cannot encode training history")?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(
                "Training history exceeds the 32 MiB limit; export and archive it on the server."
                    .into(),
            );
        }
        let parent = path.parent().ok_or("Invalid history directory")?;
        let result = (|| -> std::io::Result<()> {
            fs::create_dir_all(parent)?;
            let temp = path.with_extension("json.tmp");
            let mut file = fs::File::create(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(temp, path)?;
            fs::File::open(parent)?.sync_all()
        })();
        result
            .map_err(|_| "Could not save training history. Check server storage and retry.".into())
    }
    pub fn flush(&mut self) -> Result<(), String> {
        if self.blocked {
            return Err(self.error.clone().unwrap_or_default());
        }
        if self.dirty {
            self.write(&self.data)?;
            self.dirty = false;
        }
        self.error = None;
        Ok(())
    }
    fn commit(&mut self, mut data: TrainingData) -> Result<(), String> {
        data.revision += 1;
        self.write(&data)?;
        self.data = data;
        self.error = None;
        Ok(())
    }
    pub fn update(&mut self, request: TrainingRequest, lobby: bool) -> Result<(), String> {
        self.flush()?;
        if request.revision != self.data.revision {
            return Err("Session history changed. Reload before saving.".into());
        }
        if !lobby && !matches!(request.action, TrainingAction::Debrief { .. }) {
            return Err("Return to the lobby before changing session settings.".into());
        }
        let mut data = self.data.clone();
        match request.action {
            TrainingAction::Create {
                name,
                expected_teams,
                scoring,
            } => {
                validate_settings(&name, &expected_teams, &scoring)?;
                if data.sessions.len() >= 100 {
                    return Err(
                        "Session limit reached (100). Export and archive history on the server."
                            .into(),
                    );
                }
                let id = crate::replay::unique_suffix();
                data.active_session = Some(id.clone());
                data.sessions.push(TrainingSession {
                    id,
                    name,
                    expected_teams,
                    scoring,
                    created_at: now(),
                    next_round: "Round 1".into(),
                    rounds: vec![],
                });
            }
            TrainingAction::Activate { session_id } => {
                if session_id
                    .as_ref()
                    .is_some_and(|id| !data.sessions.iter().any(|s| &s.id == id))
                {
                    return Err("Session does not exist.".into());
                }
                data.active_session = session_id;
            }
            TrainingAction::Configure {
                name,
                expected_teams,
                scoring,
                next_round,
            } => {
                validate_settings(&name, &expected_teams, &scoring)?;
                if !label(&next_round, 120) {
                    return Err(
                        "Round name must contain 1–120 bytes without control characters.".into(),
                    );
                }
                let session = data
                    .sessions
                    .iter_mut()
                    .find(|s| Some(&s.id) == data.active_session.as_ref())
                    .ok_or("Select a session first.")?;
                if !session.rounds.is_empty()
                    && (session.scoring.win != scoring.win
                        || session.scoring.draw != scoring.draw
                        || session.scoring.loss != scoring.loss)
                {
                    return Err("Scoring is locked after the first round. Create a new session to use different points.".into());
                }
                session.name = name;
                session.expected_teams = expected_teams;
                session.scoring = scoring;
                session.next_round = next_round;
            }
            TrainingAction::Debrief { replay_id, debrief } => {
                if replay_id.is_empty()
                    || replay_id.len() > 160
                    || !replay_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    return Err("Invalid replay identifier.".into());
                }
                if debrief.notes.len() > 8000
                    || debrief.bookmarks.len() > 100
                    || debrief
                        .bookmarks
                        .iter()
                        .any(|b| !label(&b.label, 120) || b.tick > 1_000_000)
                {
                    return Err(
                        "Debriefs allow 8000 bytes of notes and 100 labelled bookmarks.".into(),
                    );
                }
                if !data.debriefs.contains_key(&replay_id) && data.debriefs.len() >= 1000 {
                    return Err("Debrief limit reached (1000).".into());
                }
                data.debriefs.insert(replay_id, debrief);
            }
        }
        self.commit(data)
    }
    pub fn begin(
        &mut self,
        match_id: &str,
        config_hash: String,
        teams: Vec<String>,
    ) -> Result<Option<RoundTag>, String> {
        self.flush()?;
        let Some(session) = self.active() else {
            return Ok(None);
        };
        if session.rounds.len() >= 500 {
            return Err("Round limit reached (500). Create a new session.".into());
        }
        let tag = RoundTag {
            session_id: session.id.clone(),
            session_name: session.name.clone(),
            number: session.rounds.len() + 1,
            name: session.next_round.clone(),
        };
        let mut data = self.data.clone();
        let s = data
            .sessions
            .iter_mut()
            .find(|s| s.id == tag.session_id)
            .ok_or("Session does not exist")?;
        s.rounds.push(Round {
            match_id: match_id.into(),
            name: tag.name.clone(),
            started_at: now(),
            config_hash,
            teams,
            status: "running".into(),
            report: None,
        });
        s.next_round = format!("Round {}", tag.number + 1);
        self.commit(data)?;
        Ok(Some(tag))
    }
    pub fn finish(&mut self, report: &MatchReport) {
        let Some(tag) = &report.round else {
            return;
        };
        let Some(session) = self
            .data
            .sessions
            .iter_mut()
            .find(|s| s.id == tag.session_id)
        else {
            return;
        };
        let Some(round) = session
            .rounds
            .iter_mut()
            .find(|r| r.match_id == report.match_id)
        else {
            return;
        };
        if round.report.is_some() {
            return;
        }
        round.status = "finished".into();
        round.report = Some(report.clone());
        self.data.revision += 1;
        self.dirty = true;
        if let Err(e) = self.flush() {
            self.error = Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("naval-training-{}", crate::replay::unique_suffix()))
            .join("history.json")
    }
    fn create(store: &mut TrainingStore) {
        let request: TrainingRequest = serde_json::from_value(serde_json::json!({ "revision": store.data.revision, "action": "create", "name": "Workshop", "expected_teams": ["Atlas", "Echo"], "scoring": {"win":3,"draw":1,"loss":0} })).unwrap();
        store.update(request, true).unwrap();
    }
    #[test]
    fn durable_rounds_interrupt_recovery_and_revision_conflicts() {
        let path = path();
        let mut store = TrainingStore::load(path.clone());
        create(&mut store);
        let tag = store
            .begin("match_1", "rules".into(), vec!["Atlas".into()])
            .unwrap()
            .unwrap();
        let mut restored = TrainingStore::load(path.clone());
        assert_eq!(restored.active().unwrap().rounds[0].status, "interrupted");
        assert!(restored
            .update(
                TrainingRequest {
                    revision: 0,
                    action: TrainingAction::Activate { session_id: None }
                },
                true
            )
            .is_err());
        let report: MatchReport = serde_json::from_value(serde_json::json!({"room":"test","match_id":"match_1","round":tag,"replay_id":"replay_1","outcome":"draw","end_reason":"timeout","winner":null,"winner_name":null,"duration_ticks":3000,"duration_seconds":300,"bots":[]})).unwrap();
        store.finish(&report);
        store.finish(&report);
        let restored = TrainingStore::load(path.clone());
        assert_eq!(restored.active().unwrap().rounds.len(), 1);
        assert_eq!(
            restored.active().unwrap().rounds[0]
                .report
                .as_ref()
                .unwrap()
                .outcome,
            "draw"
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn failed_write_does_not_acknowledge_or_replace_state_and_bad_history_is_preserved() {
        let path = path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"corrupt history").unwrap();
        let mut store = TrainingStore::load(path.clone());
        assert!(store.flush().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"corrupt history");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let mut store = TrainingStore {
            path: Some(path.clone()),
            ..Default::default()
        };
        let result = store.update(
            TrainingRequest {
                revision: 0,
                action: TrainingAction::Create {
                    name: "Test".into(),
                    expected_teams: vec![],
                    scoring: Scoring::default(),
                },
            },
            true,
        );
        assert!(result.is_err());
        assert!(store.data.sessions.is_empty());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn scoring_locks_after_first_round_and_debrief_survives_reload() {
        let path = path();
        let mut store = TrainingStore::load(path.clone());
        create(&mut store);
        store.begin("match_2", "rules".into(), vec![]).unwrap();
        assert!(store
            .update(
                TrainingRequest {
                    revision: store.data.revision,
                    action: TrainingAction::Configure {
                        name: "Workshop".into(),
                        expected_teams: vec![],
                        scoring: Scoring {
                            win: 9,
                            draw: 1,
                            loss: 0
                        },
                        next_round: "Final".into()
                    }
                },
                true
            )
            .is_err());
        store
            .update(
                TrainingRequest {
                    revision: store.data.revision,
                    action: TrainingAction::Debrief {
                        replay_id: "replay_1".into(),
                        debrief: Debrief {
                            notes: "Discuss blind spots".into(),
                            bookmarks: vec![Bookmark {
                                tick: 12,
                                label: "First contact".into(),
                            }],
                        },
                    },
                },
                false,
            )
            .unwrap();
        assert_eq!(
            TrainingStore::load(path.clone()).data.debriefs["replay_1"].bookmarks[0].tick,
            12
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
