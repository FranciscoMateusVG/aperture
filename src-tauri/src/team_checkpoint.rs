//! Bounded checkpoint writer contract. The native adapter holds the existing
//! seat lock across `write`, uses shared secure IO/SHA256, and mirrors ONLY the
//! sanitized receipt to BEADS. Worker/hook DTOs cannot assign seq or validation.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_TEXT: usize = 1024;
const MAX_FILES: usize = 200;
const MAX_RECORD_BYTES: usize = 65536;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestRef {
    pub repository: String,
    pub number: u64,
    pub head_sha: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointProcess {
    pub pid: u32,
    pub start_time: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub code: String,
    pub text: String,
    pub evidence_ref: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteEffectRef {
    pub kind: String,
    pub reference: String,
    pub state: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointPayload {
    pub task_id: String,
    /// Relative path under the native, trusted project worktree root.
    pub worktree: String,
    pub branch: String,
    pub head_sha: String,
    pub dirty_files: Vec<String>,
    pub open_pr: Option<PullRequestRef>,
    pub running_procs: Vec<CheckpointProcess>,
    pub decisions: Vec<Decision>,
    pub next_step: String,
    pub remote_effects: Vec<RemoteEffectRef>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointWriter {
    Explicit,
    ClaudeStopHook,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CheckpointValidation {
    Pending,
    Ok,
    Divergent { fields: Vec<String> },
    Rejected { code: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointEntry {
    pub schema_version: u32,
    pub checkpoint_id: String,
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub seq: u64,
    pub written_by: CheckpointWriter,
    pub written_at: u64,
    pub content_hash: String,
    pub payload: CheckpointPayload,
    pub validation: CheckpointValidation,
}
#[derive(Debug, Clone)]
pub struct CheckpointContext {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    /// Transport/native ownership derived, not accepted as a caller assertion.
    pub authenticated_generation: u64,
    pub harness: String,
    pub writer: CheckpointWriter,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointError {
    Invalid,
    Unsafe,
    Generation,
    Io,
    Corrupt,
    HookHarness,
}

fn identifier(s: &str, max: usize) -> bool {
    let bytes = s.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= max
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
}
fn sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn relative(s: &str) -> bool {
    !s.is_empty()&&s.len()<=512&&!s.starts_with('/')&&!s.contains('\\')
        &&s.split('/').all(|p|!p.is_empty()&&p!="."&&p!="..")
        &&!s.chars().any(|c|c.is_control() || matches!(c,'\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}
fn safe_text(s: &str, max: usize, sentinels: &[String]) -> bool {
    if s.is_empty() || s.len()>max || s.chars().any(|c|c.is_control() || matches!(c,'\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) || s.trim()!=s {return false}
    let l = s.to_ascii_lowercase();
    // Deliberately bounded, not universal secret detection. Native callbacks
    // must never pass raw transcript/env/tool args here in the first place.
    ![
        "http://",
        "https://",
        "file://",
        "authorization:",
        "bearer ",
        "password=",
        "secret=",
        "token=",
        "api_key=",
        "sk_live_",
        "sk_test_",
        "rk_live_",
        "-----begin",
        "process.env",
        "export ",
        "--header",
        "curl ",
        "ssh ",
        "${",
    ]
    .iter()
    .any(|x| l.contains(x))
        && !sentinels
            .iter()
            .filter(|x| !x.is_empty())
            .any(|x| s.contains(x))
}
fn reference(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:./#".contains(&b))
        && !s.contains("..")
        && !s.contains("://")
        && !s.starts_with('/')
}
pub fn validate_payload(
    p: &CheckpointPayload,
    sentinels: &[String],
) -> Result<(), CheckpointError> {
    if !identifier(&p.task_id, 100)
        || !relative(&p.worktree)
        || !relative(&p.branch)
        || !sha(&p.head_sha)
        || p.dirty_files.len() > MAX_FILES
        || p.running_procs.len() > 256
        || p.decisions.len() > 32
        || p.remote_effects.len() > 64
        || !safe_text(&p.next_step, MAX_TEXT, sentinels)
    {
        return Err(CheckpointError::Invalid);
    }
    if p.dirty_files.iter().collect::<HashSet<_>>().len() != p.dirty_files.len()
        || p.dirty_files
            .iter()
            .any(|x| !relative(x) || !safe_text(x, 512, sentinels))
    {
        return Err(CheckpointError::Unsafe);
    }
    if !safe_text(&p.worktree, 512, sentinels) || !safe_text(&p.branch, 512, sentinels) {
        return Err(CheckpointError::Unsafe);
    }
    if let Some(pr) = &p.open_pr {
        if pr.number == 0
            || pr.repository.split('/').count() != 2
            || !relative(&pr.repository)
            || !safe_text(&pr.repository, 200, sentinels)
            || !sha(&pr.head_sha)
        {
            return Err(CheckpointError::Invalid);
        }
    }
    let mut pids = HashSet::new();
    for proc in &p.running_procs {
        if proc.pid <= 1
            || !pids.insert(proc.pid)
            || proc.start_time.is_empty()
            || !safe_text(&proc.start_time, 80, sentinels)
        {
            return Err(CheckpointError::Invalid);
        }
    }
    for d in &p.decisions {
        if !identifier(&d.code, 64)
            || !safe_text(&d.text, MAX_TEXT, sentinels)
            || d.evidence_ref
                .as_ref()
                .map(|x| !reference(x) || !safe_text(x, 200, sentinels))
                .unwrap_or(false)
        {
            return Err(CheckpointError::Unsafe);
        }
    }
    for e in &p.remote_effects {
        if !["ssh", "deploy", "ci", "provider", "shell"].contains(&e.kind.as_str())
            || !["finished", "cancelled", "unknown"].contains(&e.state.as_str())
            || !reference(&e.reference)
            || !safe_text(&e.reference, 200, sentinels)
        {
            return Err(CheckpointError::Unsafe);
        }
    }
    if serde_json::to_vec(p)
        .map_err(|_| CheckpointError::Invalid)?
        .len()
        > MAX_RECORD_BYTES
    {
        return Err(CheckpointError::Invalid);
    }
    Ok(())
}

/// All methods execute under the native seat lock (same lock order as owner).
/// append is no-replace, mode0600/nlink1, file+directory fsync. The filesystem
/// adapter must revalidate parents at mutation, not canonicalize then reuse.
pub trait CheckpointStore {
    fn entries(
        &mut self,
        team: &str,
        seat: &str,
        generation: u64,
    ) -> Result<Vec<CheckpointEntry>, CheckpointError>;
    fn digest(&self, canonical: &[u8]) -> Result<String, CheckpointError>;
    fn append(&mut self, entry: &CheckpointEntry) -> Result<(), CheckpointError>;
}
pub fn write<S: CheckpointStore>(
    store: &mut S,
    ctx: &CheckpointContext,
    schema_version: u32,
    payload: CheckpointPayload,
    now: u64,
    sentinels: &[String],
) -> Result<CheckpointEntry, CheckpointError> {
    if !identifier(&ctx.team, 16)
        || !identifier(&ctx.seat, 31)
        || ctx.generation == 0
        || ctx.authenticated_generation != ctx.generation
    {
        return Err(CheckpointError::Generation);
    }
    if ctx.writer == CheckpointWriter::ClaudeStopHook && ctx.harness != "claude" {
        return Err(CheckpointError::HookHarness);
    }
    validate_payload(&payload, sentinels)?;
    let canonical =
        serde_json::to_vec(&(schema_version, &payload)).map_err(|_| CheckpointError::Invalid)?;
    let hash = store.digest(&canonical)?;
    let entries = store.entries(&ctx.team, &ctx.seat, ctx.generation)?;
    let mut seqs = HashSet::new();
    for e in &entries {
        validate_payload(&e.payload, sentinels).map_err(|_| CheckpointError::Corrupt)?;
        let stored_canonical = serde_json::to_vec(&(e.schema_version, &e.payload))
            .map_err(|_| CheckpointError::Corrupt)?;
        if store.digest(&stored_canonical)? != e.content_hash {
            return Err(CheckpointError::Corrupt);
        }
        if e.team != ctx.team
            || e.seat != ctx.seat
            || e.generation != ctx.generation
            || e.seq == 0
            || !seqs.insert(e.seq)
            || e.checkpoint_id != format!("{}/{}/{}", ctx.seat, ctx.generation, e.seq)
        {
            return Err(CheckpointError::Corrupt);
        }
    }
    if let Some(e) = entries
        .iter()
        .filter(|e| {
            e.content_hash == hash
                && e.schema_version == schema_version
                && e.payload == payload
                && now >= e.written_at
                && now - e.written_at <= 5_000
        })
        .max_by_key(|e| e.seq)
    {
        return Ok(e.clone());
    }
    let seq = entries
        .iter()
        .map(|e| e.seq)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(CheckpointError::Corrupt)?;
    let e = CheckpointEntry {
        schema_version,
        checkpoint_id: format!("{}/{}/{}", ctx.seat, ctx.generation, seq),
        team: ctx.team.clone(),
        seat: ctx.seat.clone(),
        generation: ctx.generation,
        seq,
        written_by: ctx.writer,
        written_at: now,
        content_hash: hash,
        payload,
        validation: if schema_version == 1 {
            CheckpointValidation::Pending
        } else {
            CheckpointValidation::Rejected {
                code: "E_CHECKPOINT_SCHEMA".into(),
            }
        },
    };
    store.append(&e)?;
    Ok(e)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactObservation {
    pub head_sha: String,
    pub dirty_files: Vec<String>,
    pub open_pr: Option<PullRequestRef>,
}
pub fn validate_against_artifacts(
    e: &CheckpointEntry,
    actual: &ArtifactObservation,
) -> CheckpointValidation {
    if e.schema_version != 1 {
        return CheckpointValidation::Rejected {
            code: "E_CHECKPOINT_SCHEMA".into(),
        };
    }
    let mut fields = Vec::new();
    if e.payload.head_sha != actual.head_sha {
        fields.push("head_sha".into())
    }
    let mut a = e.payload.dirty_files.clone();
    let mut b = actual.dirty_files.clone();
    a.sort();
    b.sort();
    if a != b {
        fields.push("dirty_files".into())
    }
    if e.payload.open_pr != actual.open_pr {
        fields.push("open_pr".into())
    }
    if fields.is_empty() {
        CheckpointValidation::Ok
    } else {
        CheckpointValidation::Divergent { fields }
    }
}
/// The adapter applies append-only authenticated lead-validation facts to this
/// projection; worker `validation` input is never accepted.
pub fn latest_valid(entries: &[CheckpointEntry]) -> Option<&CheckpointEntry> {
    entries
        .iter()
        .filter(|e| e.schema_version == 1 && e.validation == CheckpointValidation::Ok)
        .max_by_key(|e| e.seq)
}
pub fn stale(e: &CheckpointEntry, now: u64, max_age_ms: u64) -> bool {
    e.validation != CheckpointValidation::Ok
        || now < e.written_at
        || now - e.written_at > max_age_ms
}

#[path = "team_checkpoint_native.rs"]
pub(crate) mod native;
