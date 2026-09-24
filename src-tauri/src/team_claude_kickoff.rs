//! One native, constant post-D1 kickoff. No caller text, no boot/restart, no
//! readiness claim. Admission survives partial sends/crashes and forbids retry.
use crate::{
    journal::{
        open_private_file_nofollow, read_private_json, validate_component_path,
        write_private_json_atomic,
    },
    owner::{try_lock, OwnerRecord, OwnerStore},
    state::{ExecutionTuple, OwnerState},
    team_auth::AuthenticatedActor,
    team_claude_launch::{self as launch, ClaudeAttempt},
    team_process,
    team_replacement::{model_observation, repository, ReplacementError},
    teams::{classify_managed_seat, ManagedSeatState, TeamSnapshot},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, ReplacementError>;
fn hash(v: &[u8]) -> String {
    format!("{:x}", Sha256::digest(v))
}
fn json_hash(v: &impl Serialize) -> Result<String> {
    Ok(hash(
        &serde_json::to_vec(v).map_err(|_| ReplacementError::InvalidSnapshot)?,
    ))
}
fn authorized(actor: &AuthenticatedActor) -> Result<()> {
    if !actor.is_glados() {
        return Err(ReplacementError::AuthorizationRequired);
    }
    actor
        .revalidate_before_mutation()
        .map_err(|_| ReplacementError::AuthorizationRequired)
}
fn owner_valid(owner: &OwnerRecord, seat: &str, generation: u64) -> Result<()> {
    let inc = owner
        .incarnation
        .as_ref()
        .ok_or(ReplacementError::ModelUnverified)?;
    launch::exact_tuple(&owner.requested).map_err(|_| ReplacementError::ModelUnverified)?;
    if owner.schema_version != 1
        || owner.seat != seat
        || owner.generation != generation
        || generation == 0
        || owner.state != OwnerState::Active
        || owner.reservation_nonce_sha256.is_some()
        || owner.provisional_token_id.is_some()
        || !inc.observed
        || inc.harness != owner.requested.harness
        || inc.model != owner.requested.model
        || inc.reasoning != owner.requested.reasoning
        || !launch::canonical_uuid(&inc.thread_id)
        || inc.pid <= 1
        || inc.start_time == 0
        || !inc
            .processes
            .iter()
            .any(|p| p.pid == inc.pid && p.start_time == inc.start_time)
    {
        return Err(ReplacementError::ModelUnverified);
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pane {
    window: String,
    pane: String,
}
fn tmux_id(s: &str, prefix: char) -> bool {
    s.starts_with(prefix)
        && s.len() > 1
        && s.len() <= 16
        && s[1..].bytes().all(|v| v.is_ascii_digit())
}
/// Metadata only; never capture-pane/read screen/input/transcript. tmux cannot
/// prove Claude's internal input buffer empty. A visible or mode pane is denied;
/// earlier input/concurrent same-UID interference is an explicit residual.
fn pane_for(bytes: &[u8], pid: u32) -> Result<Pane> {
    if bytes.len() > 256 * 1024 {
        return Err(ReplacementError::LaunchUnavailable);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ReplacementError::LaunchUnavailable)?;
    let mut found = None;
    let mut rows = 0;
    for line in text.lines() {
        rows += 1;
        let p: Vec<_> = line.split('|').collect();
        if rows > 4096 || p.len() != 8 || !tmux_id(p[0], '@') || !tmux_id(p[1], '%') {
            return Err(ReplacementError::LaunchUnavailable);
        }
        let row_pid = p[2]
            .parse::<u32>()
            .map_err(|_| ReplacementError::LaunchUnavailable)?;
        if row_pid != pid {
            continue;
        }
        let attached = p[6]
            .parse::<u32>()
            .map_err(|_| ReplacementError::LaunchUnavailable)?;
        if found.is_some()
            || p[3] != "0"
            || p[4] != "0"
            || !["0", "1"].contains(&p[5])
            || attached > 1024
            || (p[5] == "1" && attached > 0)
            || p[7] != "aperture"
        {
            return Err(ReplacementError::LaunchUnavailable);
        }
        found = Some(Pane {
            window: p[0].into(),
            pane: p[1].into(),
        });
    }
    found.ok_or(ReplacementError::LaunchUnavailable)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    team: String,
    seat: String,
    generation: u64,
    team_generation: u64,
    owner_sha256: String,
    snapshot_sha256: String,
    attempt_sha256: String,
    session: String,
    pid: u32,
    start_time_us: u64,
    pane: Pane,
    tmux_path: PathBuf,
    tmux_sha256: String,
    constant_sha256: String,
}
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fact {
    schema_version: u32,
    phase: String,
    binding: Binding,
}
// Private injection boundary for hermetic tests only. Public entrypoint always
// uses Native and derives all observations; no DTO accepts any of these facts.
trait KickoffIo {
    fn current(&mut self) -> Result<Binding>;
    fn no_previous(&mut self) -> Result<()>;
    fn publish(&mut self, phase: &str, binding: &Binding) -> Result<()>;
    fn literal(&mut self, binding: &Binding) -> Result<()>;
    fn enter(&mut self, binding: &Binding) -> Result<()>;
}
fn execute(io: &mut impl KickoffIo) -> Result<()> {
    let binding = io.current()?;
    io.no_previous()?;
    if io.current()? != binding {
        return Err(ReplacementError::ModelUnverified);
    }
    // Even publication errors may follow durable admission: never retry.
    io.publish("admitted", &binding)
        .map_err(|_| ReplacementError::OutcomeUnknown)?;
    (|| {
        if io.current()? != binding {
            return Err(ReplacementError::OutcomeUnknown);
        }
        io.literal(&binding)?;
        if io.current()? != binding {
            return Err(ReplacementError::OutcomeUnknown);
        }
        io.enter(&binding)?;
        if io.current()? != binding {
            return Err(ReplacementError::OutcomeUnknown);
        }
        io.publish("sent", &binding)
    })()
    .map_err(|_| ReplacementError::OutcomeUnknown)
}
#[derive(Clone, Copy)]
enum KeyEffect {
    Literal,
    Enter,
}
fn key_args(pane: &str, effect: KeyEffect) -> Result<Vec<&str>> {
    if !tmux_id(pane, '%') {
        return Err(ReplacementError::InvalidSnapshot);
    }
    Ok(match effect {
        KeyEffect::Literal => vec![
            "send-keys",
            "-l",
            "-t",
            pane,
            "--",
            crate::launcher::KICKOFF_TEXT,
        ],
        KeyEffect::Enter => vec!["send-keys", "-t", pane, "Enter"],
    })
}
struct Native<'a> {
    home: &'a Path,
    actor: &'a AuthenticatedActor,
    team: &'a str,
    seat: &'a str,
    generation: u64,
    until: Instant,
    dir: PathBuf,
}
impl Native<'_> {
    fn remaining(&self) -> Result<Instant> {
        if Instant::now() >= self.until {
            return Err(ReplacementError::Deadline);
        }
        Ok(self.until.min(Instant::now() + Duration::from_secs(3)))
    }
    fn tmux(&self, path: &Path, args: &[&str]) -> Result<Vec<u8>> {
        let mut cmd = Command::new(path);
        cmd.args(args)
            .env_clear()
            .env("HOME", self.home)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(self.home);
        repository::bounded_command(cmd, self.remaining()?)
            .map_err(|_| ReplacementError::OutcomeUnknown)
    }
    fn fact_path(&self, phase: &str) -> PathBuf {
        self.dir.join(format!("claude-kickoff-{phase}.json"))
    }
}
impl KickoffIo for Native<'_> {
    fn current(&mut self) -> Result<Binding> {
        self.remaining()?;
        authorized(self.actor)?;
        let team_generation = match classify_managed_seat(self.home, self.seat)
            .map_err(|_| ReplacementError::InvalidSnapshot)?
        {
            Some(ManagedSeatState::Active { team, generation }) if team == self.team => generation,
            _ => return Err(ReplacementError::InvalidSnapshot),
        };
        let owner: OwnerRecord = OwnerStore::new(self.home.join(".aperture/run/owner"))
            .read_owner_locked(self.seat)
            .map_err(|_| ReplacementError::InvalidSnapshot)?;
        owner_valid(&owner, self.seat, self.generation)?;
        let inc = owner.incarnation.as_ref().unwrap();
        let snapshot_path = self
            .home
            .join(".aperture/teams")
            .join(self.team)
            .join("team.json");
        let mut bytes = Vec::new();
        open_private_file_nofollow(&snapshot_path)
            .map_err(|_| ReplacementError::InvalidSnapshot)?
            .take(1_048_577)
            .read_to_end(&mut bytes)
            .map_err(|_| ReplacementError::InvalidSnapshot)?;
        if bytes.len() > 1_048_576 {
            return Err(ReplacementError::InvalidSnapshot);
        }
        let snapshot: TeamSnapshot =
            serde_json::from_slice(&bytes).map_err(|_| ReplacementError::InvalidSnapshot)?;
        let seats: Vec<_> = snapshot
            .seats
            .iter()
            .filter(|s| s.name == self.seat)
            .collect();
        if snapshot.schema_version != 1 || snapshot.team != self.team || seats.len() != 1 {
            return Err(ReplacementError::InvalidSnapshot);
        }
        let configured = ExecutionTuple {
            harness: seats[0].harness.clone(),
            model: seats[0].model.clone(),
            reasoning: seats[0].reasoning.clone(),
        };
        if configured != owner.requested && !snapshot.fallbacks.contains(&owner.requested) {
            return Err(ReplacementError::ModelUnverified);
        }
        let attempt: ClaudeAttempt = read_private_json(&self.home.join(".aperture/run").join(
            format!("{}.g{}.claude-attempt.json", self.seat, self.generation),
        ))
        .map_err(|_| ReplacementError::InvalidSnapshot)?;
        let now = chrono::Utc::now().timestamp_millis();
        if attempt.schema_version != 1
            || attempt.team != self.team
            || attempt.seat != self.seat
            || attempt.generation != self.generation
            || attempt.team_generation != team_generation
            || attempt.snapshot_sha256 != hash(&bytes)
            || attempt.root_pid != inc.pid
            || attempt.root_start_time_us != inc.start_time
            || attempt.session_id != inc.thread_id
            || attempt.token_id != inc.token_id
            || attempt.requested_model != owner.requested.model
            || attempt.created_at_ms <= 0
            || now < attempt.created_at_ms
            || now - attempt.created_at_ms > launch::WINDOW_MS
        {
            return Err(ReplacementError::ModelUnverified);
        }
        model_observation::token_current(self.home, self.seat, self.generation, &inc.token_id)
            .map_err(|_| ReplacementError::RevocationUnverified)?;
        let expected = team_process::identity_from_owner(inc.pid, inc.start_time)?;
        let live = team_process::observe(inc.pid)?.ok_or(ReplacementError::StopUnverified)?;
        if live.identity != expected || live.uid != unsafe { libc::geteuid() } {
            return Err(ReplacementError::StopUnverified);
        }
        let (tmux_path, tmux_sha256) =
            launch::kickoff_tmux(self.home, self.team, &owner, &snapshot)
                .map_err(|_| ReplacementError::LaunchUnavailable)?;
        let out=self.tmux(&tmux_path,&["list-panes","-a","-F",
            "#{window_id}|#{pane_id}|#{pane_pid}|#{pane_dead}|#{pane_in_mode}|#{window_active}|#{session_attached}|#{session_name}"])?;
        let pane = pane_for(&out, inc.pid)?;
        // Metadata query is awaited; process/capability must still be exact.
        let after = team_process::observe(inc.pid)?.ok_or(ReplacementError::StopUnverified)?;
        if after.identity != expected || after.uid != live.uid {
            return Err(ReplacementError::StopUnverified);
        }
        let reread = OwnerStore::new(self.home.join(".aperture/run/owner"))
            .read_owner_locked(self.seat)
            .map_err(|_| ReplacementError::InvalidSnapshot)?;
        if reread != owner {
            return Err(ReplacementError::ModelUnverified);
        }
        model_observation::token_current(self.home, self.seat, self.generation, &inc.token_id)
            .map_err(|_| ReplacementError::RevocationUnverified)?;
        authorized(self.actor)?;
        self.remaining()?;
        Ok(Binding {
            team: self.team.into(),
            seat: self.seat.into(),
            generation: self.generation,
            team_generation,
            owner_sha256: json_hash(&owner)?,
            snapshot_sha256: hash(&bytes),
            attempt_sha256: json_hash(&attempt)?,
            session: inc.thread_id.clone(),
            pid: inc.pid,
            start_time_us: inc.start_time,
            pane,
            tmux_path,
            tmux_sha256,
            constant_sha256: hash(crate::launcher::KICKOFF_TEXT.as_bytes()),
        })
    }
    fn no_previous(&mut self) -> Result<()> {
        for phase in ["admitted", "sent"] {
            match std::fs::symlink_metadata(self.fact_path(phase)) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(ReplacementError::OutcomeUnknown),
            }
        }
        Ok(())
    }
    fn publish(&mut self, phase: &str, binding: &Binding) -> Result<()> {
        self.remaining()?;
        authorized(self.actor)?;
        let fact = Fact {
            schema_version: 1,
            phase: phase.into(),
            binding: binding.clone(),
        };
        let path = self.fact_path(phase);
        write_private_json_atomic(&path, &fact, false)
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        let read: Fact = read_private_json(&path).map_err(|_| ReplacementError::OutcomeUnknown)?;
        if read != fact {
            return Err(ReplacementError::OutcomeUnknown);
        }
        Ok(())
    }
    fn literal(&mut self, binding: &Binding) -> Result<()> {
        authorized(self.actor)?;
        let out = self.tmux(
            &binding.tmux_path,
            &key_args(&binding.pane.pane, KeyEffect::Literal)?,
        )?;
        if !out.is_empty() {
            return Err(ReplacementError::OutcomeUnknown);
        }
        Ok(())
    }
    fn enter(&mut self, binding: &Binding) -> Result<()> {
        authorized(self.actor)?;
        let out = self.tmux(
            &binding.tmux_path,
            &key_args(&binding.pane.pane, KeyEffect::Enter)?,
        )?;
        if !out.is_empty() {
            return Err(ReplacementError::OutcomeUnknown);
        }
        Ok(())
    }
}
/// Caller is native authenticated control for a NEW exclusively reserved probe,
/// immediately following D1/Active and before any Open. Never a general-purpose
/// prompt sender for existing interactive sessions or a worker-supplied proof/text.
/// Holds canonical team then seat locks through admission and both bounded keys.
pub(crate) fn kickoff_active(
    home: &Path,
    actor: &AuthenticatedActor,
    team: &str,
    seat: &str,
    generation: u64,
    until: Instant,
) -> Result<()> {
    authorized(actor)?;
    launch::valid_selector(team, seat, generation)
        .map_err(|_| ReplacementError::InvalidSnapshot)?;
    if Instant::now() >= until {
        return Err(ReplacementError::Deadline);
    }
    let _team = try_lock(&home.join(".aperture/run/team-locks"), team)
        .map_err(|_| ReplacementError::OutcomeUnknown)?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let _seat = store
        .lock(seat)
        .map_err(|_| ReplacementError::OutcomeUnknown)?;
    let dir = validate_component_path(
        &home.join(".aperture/run/managed"),
        &format!("{seat}/g{generation}"),
        false,
    )
    .map_err(|_| ReplacementError::InvalidSnapshot)?;
    execute(&mut Native {
        home,
        actor,
        team,
        seat,
        generation,
        until,
        dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        journal::{ensure_private_dir, write_private_bytes_atomic},
        state::{Harness, ReasoningEffort},
    };
    use std::collections::BTreeMap;
    fn binding() -> Binding {
        Binding {
            team: "t1".into(),
            seat: "t1-worker".into(),
            generation: 1,
            team_generation: 1,
            owner_sha256: "a".repeat(64),
            snapshot_sha256: "b".repeat(64),
            attempt_sha256: "c".repeat(64),
            session: "12345678-1234-4234-8234-123456789012".into(),
            pid: 77,
            start_time_us: 123,
            pane: Pane {
                window: "@1".into(),
                pane: "%2".into(),
            },
            tmux_path: "/fixture/tmux".into(),
            tmux_sha256: "d".repeat(64),
            constant_sha256: hash(crate::launcher::KICKOFF_TEXT.as_bytes()),
        }
    }
    struct Fake {
        current: usize,
        fail_current: Option<usize>,
        drift: Option<usize>,
        events: Vec<&'static str>,
        facts: BTreeMap<String, Binding>,
        fail_literal: bool,
        fail_enter: bool,
        fail_publish: Option<&'static str>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                current: 0,
                fail_current: None,
                drift: None,
                events: vec![],
                facts: BTreeMap::new(),
                fail_literal: false,
                fail_enter: false,
                fail_publish: None,
            }
        }
    }
    impl KickoffIo for Fake {
        fn current(&mut self) -> Result<Binding> {
            self.current += 1;
            if self.fail_current == Some(self.current) {
                return Err(ReplacementError::AuthorizationRequired);
            }
            let mut b = binding();
            if self.drift == Some(self.current) {
                b.start_time_us += 1;
            }
            Ok(b)
        }
        fn no_previous(&mut self) -> Result<()> {
            if self.facts.is_empty() {
                Ok(())
            } else {
                Err(ReplacementError::OutcomeUnknown)
            }
        }
        fn publish(&mut self, phase: &str, b: &Binding) -> Result<()> {
            if self.facts.contains_key(phase) {
                return Err(ReplacementError::OutcomeUnknown);
            }
            self.facts.insert(phase.into(), b.clone());
            self.events.push(if phase == "admitted" {
                "admitted"
            } else {
                "sent"
            });
            if self.fail_publish == Some(phase) {
                return Err(ReplacementError::OutcomeUnknown);
            }
            Ok(())
        }
        fn literal(&mut self, _: &Binding) -> Result<()> {
            self.events.push("literal");
            if self.fail_literal {
                Err(ReplacementError::Deadline)
            } else {
                Ok(())
            }
        }
        fn enter(&mut self, _: &Binding) -> Result<()> {
            self.events.push("enter");
            if self.fail_enter {
                Err(ReplacementError::NativeFailure)
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn exactly_once_after_admission_and_success_not_readiness() {
        let mut f = Fake::new();
        execute(&mut f).unwrap();
        assert_eq!(f.events, ["admitted", "literal", "enter", "sent"]);
        assert_eq!(f.facts["admitted"], f.facts["sent"]);
        assert!(matches!(
            execute(&mut f),
            Err(ReplacementError::OutcomeUnknown)
        ));
        assert_eq!(f.events.len(), 4);
    }
    #[test]
    fn authentication_or_drift_before_admission_has_no_effect() {
        for at in [1, 2] {
            let mut f = Fake::new();
            f.fail_current = Some(at);
            assert!(execute(&mut f).is_err());
            assert!(f.events.is_empty());
        }
        let mut f = Fake::new();
        f.drift = Some(2);
        assert!(execute(&mut f).is_err());
        assert!(f.events.is_empty());
    }
    #[test]
    fn replaced_auth_owner_pid_or_pane_between_effects_never_retries() {
        for at in [3, 4, 5] {
            for drift in [false, true] {
                let mut f = Fake::new();
                if drift {
                    f.drift = Some(at)
                } else {
                    f.fail_current = Some(at)
                };
                assert!(matches!(
                    execute(&mut f),
                    Err(ReplacementError::OutcomeUnknown)
                ));
                assert!(f.facts.contains_key("admitted"));
                assert!(!f.facts.contains_key("sent"));
                assert_eq!(f.events.len(), at - 2);
                let old = f.events.clone();
                assert!(execute(&mut f).is_err());
                assert_eq!(f.events, old);
            }
        }
    }
    #[test]
    fn timeout_partial_write_or_sent_publish_failure_is_unknown() {
        for stage in ["admitted", "literal", "enter", "sent"] {
            let mut f = Fake::new();
            match stage {
                "literal" => f.fail_literal = true,
                "enter" => f.fail_enter = true,
                _ => f.fail_publish = Some(stage),
            };
            assert!(matches!(
                execute(&mut f),
                Err(ReplacementError::OutcomeUnknown)
            ));
            assert!(f.facts.contains_key("admitted"));
            let old = f.events.clone();
            assert!(execute(&mut f).is_err());
            assert_eq!(old, f.events);
        }
    }
    #[test]
    fn native_key_arguments_are_only_literal_constant_then_single_enter() {
        assert_eq!(
            key_args("%2", KeyEffect::Literal).unwrap(),
            vec![
                "send-keys",
                "-l",
                "-t",
                "%2",
                "--",
                crate::launcher::KICKOFF_TEXT
            ]
        );
        assert_eq!(
            key_args("%2", KeyEffect::Enter).unwrap(),
            vec!["send-keys", "-t", "%2", "Enter"]
        );
        assert!(key_args("%2;new-window", KeyEffect::Literal).is_err());
        assert!(key_args("named-pane", KeyEffect::Enter).is_err());
    }
    #[test]
    fn pane_is_exact_alive_hidden_and_unambiguous_metadata_only() {
        assert_eq!(
            pane_for(b"@1|%2|77|0|0|0|1|aperture\n", 77).unwrap(),
            binding().pane
        );
        for bad in [
            "@1|%2|77|1|0|0|0|aperture",
            "@1|%2|77|0|1|0|0|aperture",
            "@1|%2|77|0|0|1|1|aperture",
            "@1|%2|77|0|0|0|0|other",
            "@1|%2|78|0|0|0|0|aperture",
            "name|%2|77|0|0|0|0|aperture",
            "@1|%2|77|0|0|?|0|aperture",
            "@1|%2|77|0|0|0|?|aperture",
            "@1|%2|77|0|0|0|0|aperture\n@1|%2|77|0|0|0|0|aperture",
        ] {
            assert!(pane_for(bad.as_bytes(), 77).is_err());
        }
    }
    fn owner() -> OwnerRecord {
        serde_json::from_value(serde_json::json!({"schema_version":1,"seat":"t1-worker","generation":1,"state":"active","since":"2026-09-20T00:00:00Z","writer":"launcher","reservation_nonce_sha256":null,"provisional_token_id":null,"requested":{"harness":"claude","model":launch::MODEL,"reasoning":null},"incarnation":{"pid":77,"start_time":123,"thread_id":binding().session,"token_id":"a".repeat(64),"harness":"claude","model":launch::MODEL,"reasoning":null,"observed":true,"processes":[{"pid":77,"start_time":123,"ppid":3,"pgid":77,"cmdline_sha256":"b".repeat(64),"cwd":"/fixture"}]}})).unwrap()
    }
    #[test]
    fn only_active_exact_d1_owner_admitted() {
        owner_valid(&owner(), "t1-worker", 1).unwrap();
        for change in 0..13 {
            let mut o = owner();
            match change {
                0 => o.state = OwnerState::Starting,
                1 => o.state = OwnerState::Quarantined,
                2 => o.generation = 0,
                3 => o.incarnation.as_mut().unwrap().observed = false,
                4 => o.requested.model = "sonnet".into(),
                5 => o.requested.harness = Harness::Codex,
                6 => o.incarnation.as_mut().unwrap().reasoning = Some(ReasoningEffort::High),
                7 => o.incarnation.as_mut().unwrap().thread_id = "newest".into(),
                8 => o.incarnation.as_mut().unwrap().processes.clear(),
                9 => o.reservation_nonce_sha256 = Some("c".repeat(64)),
                10 => o.provisional_token_id = Some("a".repeat(64)),
                11 => o.incarnation.as_mut().unwrap().pid = 1,
                _ => o.seat = "other".into(),
            }
            assert!(owner_valid(&o, "t1-worker", 1).is_err());
        }
    }
    #[test]
    fn canonical_auth_not_ui_launcher_or_replaced_capability() {
        assert!(authorized(&AuthenticatedActor::operator_ui()).is_err());
        assert!(authorized(&AuthenticatedActor::launcher()).is_err());
        let _lock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("kickoff-auth-{}", uuid::Uuid::new_v4()));
        ensure_private_dir(&home.join(".aperture/run/hub-tokens")).unwrap();
        ensure_private_dir(&home.join(".claude/aperture/glados")).unwrap();
        write_private_bytes_atomic(
            &home.join(".aperture/run/hub-tokens/glados.token"),
            &[b'a'; 64],
            false,
        )
        .unwrap();
        write_private_bytes_atomic(
            &home.join(".claude/aperture/glados/prompt.md"),
            b"fixture",
            false,
        )
        .unwrap();
        write_private_bytes_atomic(&home.join(".claude/aperture/glados/manifest.json"),br#"{"name":"GLaDOS","model":"sonnet","window":"glados","role":"orchestrator","enabled":true}"#,false).unwrap();
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
        let _restore = Restore(std::env::var_os("HOME"));
        std::env::set_var("HOME", &home);
        let actor = crate::team_auth::authenticate_glados_control().unwrap();
        authorized(&actor).unwrap();
        write_private_bytes_atomic(
            &home.join(".aperture/run/hub-tokens/glados.token"),
            &[b'b'; 64],
            true,
        )
        .unwrap();
        assert!(authorized(&actor).is_err());
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn private_admission_is_no_replace_and_survives_failure() {
        let root = std::env::temp_dir().join(format!("kickoff-facts-{}", uuid::Uuid::new_v4()));
        ensure_private_dir(&root).unwrap();
        let path = root.join("claude-kickoff-admitted.json");
        let fact = Fact {
            schema_version: 1,
            phase: "admitted".into(),
            binding: binding(),
        };
        write_private_json_atomic(&path, &fact, false).unwrap();
        assert!(write_private_json_atomic(&path, &fact, false).is_err());
        let read: Fact = read_private_json(&path).unwrap();
        assert!(read == fact);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(crate::launcher::KICKOFF_TEXT));
        std::fs::remove_dir_all(root).unwrap();
    }
}
