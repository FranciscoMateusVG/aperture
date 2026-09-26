//! Crash-consistent Aperture V4 team creation and activation.
//!
//! The public Tauri surface can create/list/cancel pending teams and manage
//! presets. Activation is deliberately absent from the UI surface: the
//! headless control path constructs a private `AuthenticatedActor` from the
//! canonical GLaDOS bearer capability and calls the same engine below.

use crate::agent_loader::is_valid_seat_name;
use crate::journal::{
    apply_or_recover_journal, ensure_private_dir, read_private_json, remove_journal,
    rename_no_replace, sync_dir, sync_parent,
    write_journal, write_private_bytes_atomic, write_private_json_atomic, Journal,
    JournalMove, JournalObjectKind, JournalOperation, JournalRoot, JournalRoots,
};
use crate::owner::{try_lock, AdvisoryLock, OwnerStore};
use crate::state::{AppState, ExecutionTuple, Harness, OwnerState, OwnerSummary, ReasoningEffort};
use crate::team_auth::{authenticate_glados_control, authenticate_seat_control, AuthenticatedActor};
use crate::team_checkpoint::{CheckpointContext, CheckpointError, CheckpointPayload, CheckpointWriter};
use crate::team_replacement::{
    CheckpointRecovery, ReplacementError, ReplacementPhase, StartSelection,
};
use crate::team_replacement::native::{
    bootstrap_authorized, bootstrap_claude_smoke_authorized, prepare_operator, replace_authorized, start_operator, stop_for_archive,
    ReplacementAuthority,
};
use crate::team_replacement::remote::{
    inspect_authorized, resolve_native, RemoteError, RemoteInventoryView, RemoteTarget,
    ResolutionAuthority, ResolutionReceipt, ResolutionRequest,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const MAX_SEATS: usize = 99;
const MAX_FALLBACKS: usize = 16;
const MAX_ROLE_SKILLS: usize = 64;
const MAX_PRESET_BYTES: usize = 262_144;
const MAX_TEMPLATE_BYTES: usize = 131_072;
const MAX_RENDERED_SEAT_BYTES: usize = 262_144;
const MAX_RENDERED_TEAM_BYTES: usize = 8_388_608;
const MAX_DISPLAY_SCALARS: usize = 80;
const MAX_DISPLAY_BYTES: usize = 320;
const MAX_MISSION_SCALARS: usize = 2_000;
const MAX_MISSION_BYTES: usize = 8_000;

const REPOSITORY_SEEDS: &[(&str, &str, &str)] = &[
    ("project:aperture", "aperture", "Aperture"),
    ("project:beads-galaxy", "beads-galaxy", "Beads Galaxy"),
    ("project:frame", "frame", "Frame"),
    ("project:incluir", "monorepo-incluir", "Programa Incluir"),
    ("project:incluir", "eunenem", "EuNeném"),
];

#[path = "team_repository_catalog.rs"]
mod repository_registry;
pub use repository_registry::{RepositoryRegistryView, SaveRepositoryInput};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamError {
    pub code: String,
    pub message: String,
}

impl TeamError {
    fn new(code: &str, message: &str) -> Self {
        Self { code: code.into(), message: message.into() }
    }
    fn name(message: &str) -> Self { Self::new("E_NAME_INVALID", message) }
    fn preset(message: &str) -> Self { Self::new("E_PRESET_INVALID", message) }
    fn budget(message: &str) -> Self { Self::new("E_BUDGET_EXCEEDED", message) }
    fn io(message: &str) -> Self { Self::new("E_STAGING_IO", message) }
    fn state(message: &str) -> Self { Self::new("E_STATE_CONFLICT", message) }
    fn from_message(message: String) -> Self {
        let code = message.split(':').next().filter(|s| s.starts_with("E_")).unwrap_or("E_STAGING_IO");
        Self::new(code, message.split_once(':').map_or(message.as_str(), |(_, rest)| rest.trim()))
    }
}

impl std::fmt::Display for TeamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TeamError {}

type TeamResult<T> = Result<T, TeamError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresetSource { Shipped, Local }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PresetSeat {
    pub role: String,
    pub harness: Harness,
    pub model: String,
    pub reasoning: Option<ReasoningEffort>,
}

impl PresetSeat {
    fn tuple(&self) -> ExecutionTuple {
        ExecutionTuple { harness: self.harness.clone(), model: self.model.clone(), reasoning: self.reasoning.clone() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TeamPresetWrite {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub mission_placeholder: String,
    pub acceptance_placeholder: String,
    pub seats: Vec<PresetSeat>,
    pub lead_index: usize,
    pub fallbacks: Vec<ExecutionTuple>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamPreset {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    pub mission_placeholder: String,
    pub acceptance_placeholder: String,
    pub seats: Vec<PresetSeat>,
    pub lead_index: usize,
    pub fallbacks: Vec<ExecutionTuple>,
    pub source: PresetSource,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
struct StoredPreset {
    schema_version: u32,
    id: String,
    display_name: String,
    mission_placeholder: String,
    acceptance_placeholder: String,
    seats: Vec<PresetSeat>,
    lead_index: usize,
    fallbacks: Vec<ExecutionTuple>,
    #[serde(default)]
    source: Option<PresetSource>,
}

impl StoredPreset {
    fn into_public(self, source: PresetSource, sha256: String) -> TeamPreset {
        TeamPreset {
            schema_version: self.schema_version,
            id: self.id,
            display_name: self.display_name,
            mission_placeholder: self.mission_placeholder,
            acceptance_placeholder: self.acceptance_placeholder,
            seats: self.seats,
            lead_index: self.lead_index,
            fallbacks: self.fallbacks,
            source,
            sha256,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavePresetInput {
    pub preset: TeamPresetWrite,
    pub expected_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleCatalogEntry {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryCatalogEntry {
    pub project: String,
    pub repo: String,
    pub display_name: String,
    /// Native availability is informational. The create and activation paths
    /// re-resolve the fixed catalog entry before mutating anything.
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamLimits {
    pub max_seats: usize,
    pub max_fallbacks: usize,
    pub max_role_skills: usize,
    pub max_preset_bytes: usize,
    pub max_template_bytes: usize,
    pub max_rendered_seat_bytes: usize,
    pub max_rendered_team_bytes: usize,
    pub max_display_scalars: usize,
    pub max_display_bytes: usize,
    pub max_mission_scalars: usize,
    pub max_mission_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamCatalog {
    pub roles: Vec<RoleCatalogEntry>,
    pub repositories: Vec<RepositoryCatalogEntry>,
    pub execution_tuples: Vec<ExecutionTuple>,
    pub limits: TeamLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateTeamInput {
    pub team: String,
    pub project: String,
    pub repo: String,
    pub mission: String,
    pub acceptance: String,
    pub preset_id: Option<String>,
    pub seats: Vec<PresetSeat>,
    pub lead_index: usize,
    pub fallbacks: Vec<ExecutionTuple>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetSnapshotRef {
    pub id: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamSeat {
    pub name: String,
    pub role: String,
    pub harness: Harness,
    pub model: String,
    pub reasoning: Option<ReasoningEffort>,
}

impl TeamSeat {
    fn tuple(&self) -> ExecutionTuple {
        ExecutionTuple { harness: self.harness.clone(), model: self.model.clone(), reasoning: self.reasoning.clone() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamGrant {
    pub from: String,
    pub to: String,
    pub scope: String,
    pub by: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamSnapshot {
    pub schema_version: u32,
    pub team: String,
    pub project: String,
    pub repo: String,
    pub mission: String,
    pub acceptance: String,
    pub preset: PresetSnapshotRef,
    pub lead: String,
    pub seats: Vec<TeamSeat>,
    pub fallbacks: Vec<ExecutionTuple>,
    pub grants: Vec<TeamGrant>,
    pub created_at: String,
    pub creation_request_id: String,
    pub staging_uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TeamLifecycle { Pending, Active, Failed, Archived }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamFailure {
    pub code: String,
    pub completed_moves: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamStateFile {
    pub schema_version: u32,
    pub state: TeamLifecycle,
    pub generation: u64,
    pub epic_id: Option<String>,
    pub failure: Option<TeamFailure>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreationRequestDTO {
    pub schema_version: u32,
    pub request_id: String,
    pub team: String,
    pub project: String,
    pub repo: String,
    pub snapshot_sha256: String,
    pub expected_generation: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamCapabilities {
    pub cancel: bool,
    pub activate: bool,
    pub start: bool,
    pub checkpoint: bool,
    pub replace: bool,
    pub archive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamSeatView {
    /// Immutable tuple from the activated team snapshot.
    pub configured: TeamSeat,
    /// Current owner request/observation. `configured` inside OwnerSummary is
    /// the current incarnation request and may be an approved fallback.
    pub observed_owner: Option<OwnerSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamView {
    pub snapshot: TeamSnapshot,
    pub state: TeamStateFile,
    pub seats: Vec<TeamSeatView>,
    pub capabilities: TeamCapabilities,
}

/// Operator-approved normal launch, not a claim of mission readiness. Native
/// preflight still requires the exact supported tuple and real model observation.
/// Never replace this with a caller field, environment override, or UI flag.
pub(crate) fn managed_launch_enabled(harness: &Harness) -> bool {
    matches!(harness, Harness::Codex | Harness::Claude)
}

pub(crate) fn managed_execution_enabled(tuple: &ExecutionTuple) -> bool {
    managed_launch_enabled(&tuple.harness)
        && (tuple.harness == Harness::Codex
            || (crate::team_claude_launch::is_exact_claude_model(&tuple.model) && tuple.reasoning.is_none()))
}

fn team_capabilities(state: &TeamLifecycle, seats: &[TeamSeatView]) -> TeamCapabilities {
    let pending = *state == TeamLifecycle::Pending;
    let active = *state == TeamLifecycle::Active;
    let start = active && seats.iter().any(|seat| {
        let configured = seat.configured.tuple();
        managed_execution_enabled(&configured)
            && seat.observed_owner.as_ref().is_some_and(|owner| {
                let fresh = owner.generation == 0
                    && owner.state == OwnerState::Stale
                    && owner.process_count == 0;
                // Recoverable Claude bootstrap: quarantined g1 (first bootstrap) or
                // g2 (one recovery that failed the same way), never observed, thread
                // never bound. process_count is persisted history, not liveness (a
                // Gone root is still counted); the native proof decides. Nothing
                // later than g2 is recoverable.
                let recoverable = (owner.generation == 1 || owner.generation == 2)
                    && owner.state == OwnerState::Quarantined
                    && configured.harness == Harness::Claude;
                (fresh || recoverable)
                    && owner.configured == configured
                    && owner.actual.is_none()
                    && !owner.thread_bound
            })
    });
    let replace = active && seats.iter().any(|seat| {
        seat.observed_owner.as_ref().is_some_and(|owner| {
            owner.generation > 0
                && owner.state == OwnerState::Active
                && managed_execution_enabled(&owner.configured)
                && owner.actual.as_ref() == Some(&owner.configured)
                && owner.process_count > 0
                && owner.thread_bound
        })
    });
    TeamCapabilities {
        cancel: pending,
        activate: pending,
        start,
        // Checkpoints are authenticated managed-seat control actions. There is
        // deliberately no operator/UI checkpoint command.
        checkpoint: false,
        replace,
        // This exposes only the read-only checklist. Archive mutation remains
        // an independently authenticated GLaDOS control action.
        archive: active,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTeamResult {
    pub team: TeamView,
    pub creation_request: CreationRequestDTO,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CancelPendingInput {
    pub team: String,
    pub expected_generation: u64,
    pub creation_request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CancelPendingResult {
    pub team: String,
    pub cancelled: bool,
    pub rejected_snapshot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivateTeamInput {
    pub team: String,
    pub expected_generation: u64,
    pub creation_request_id: String,
    pub epic_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WriteCheckpointInput {
    pub schema_version: u32,
    pub payload: CheckpointPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointReceipt {
    pub checkpoint_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentReplaceInput {
    pub target_seat: String,
    /// Target owner-generation CAS selector, never caller authority.
    pub expected_generation: u64,
    pub selection: ExecutionTuple,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BootstrapSeatInput {
    pub team: String,
    pub seat: String,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrepareReplacementInput {
    pub team: String,
    pub seat: String,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartReplacementInput {
    pub team: String,
    pub seat: String,
    pub expected_generation: u64,
    pub preparation_id: String,
    pub selection: ExecutionTuple,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCheckState {
    Pending,
    Verified,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RuntimeBlocker {
    pub code: String,
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReplacementChecks {
    pub process_stop: RuntimeCheckState,
    pub revocation: RuntimeCheckState,
    pub remote_effects: RuntimeCheckState,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReplacementView {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub phase: ReplacementPhase,
    pub checkpoint_recovery: CheckpointRecovery,
    pub checks: ReplacementChecks,
    pub owner: Option<OwnerSummary>,
    pub blockers: Vec<RuntimeBlocker>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreparedReplacementView {
    #[serde(flatten)]
    pub replacement: ReplacementView,
    pub preparation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapPhase {
    Starting,
    Started,
    Blocked,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BootstrapView {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub phase: BootstrapPhase,
    pub owner: Option<OwnerSummary>,
    pub blockers: Vec<RuntimeBlocker>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveTeamInput {
    pub team: String,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArchiveChecks {
    pub reconciliation: RuntimeCheckState,
    pub reviews: RuntimeCheckState,
    pub metrics: RuntimeCheckState,
    pub process_stop: RuntimeCheckState,
    pub revocation: RuntimeCheckState,
    pub remote_effects: RuntimeCheckState,
    pub worktrees: RuntimeCheckState,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArchiveView {
    pub team: String,
    pub generation: u64,
    pub state: String,
    pub checks: ArchiveChecks,
    pub blockers: Vec<RuntimeBlocker>,
}

/// GLaDOS-only archival stop selectors; no actor, loss policy or new tuple.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StopSeatInput {
    pub team: String,
    pub seat: String,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetireSeatInput {
    pub team:String, pub seat:String, pub expected_generation:u64,
    pub accept_checkpoint_loss:bool,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetireSeatView {
    pub team:String, pub seat:String, pub generation:u64,
    pub process_stop:RuntimeCheckState, pub revocation:RuntimeCheckState,
    pub mission:RuntimeCheckState, pub owner_state:OwnerState,
    pub checkpoint_recovery:CheckpointRecovery,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StopSeatView {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub phase: ReplacementPhase,
    pub checkpoint_recovery: CheckpointRecovery,
    pub owner_state: OwnerState,
    pub blockers: Vec<RuntimeBlocker>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidateCheckpointInput {
    pub team: String,
    pub seat: String,
    pub expected_generation: u64,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValidateCheckpointView {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub seq: u64,
    pub validation: &'static str,
}

/// One diagnostic attempt only; no tuple, prompt or caller authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClaudeStartupSmokeInput {
    pub team: String,
    pub seat: String,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ClaudeStartupSmokeView {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub model: String,
    pub reasoning_observation: &'static str,
    pub startup: &'static str,
    pub cleanup: &'static str,
    pub mcp_readiness: &'static str,
    pub public_enabled: bool,
}

/// Startup and submitted kickoff are facts separate from mission readiness.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ClaudeInboxProbeView {
    pub cleanup: &'static str,
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub model: String,
    pub reasoning_observation: &'static str,
    pub startup: &'static str,
    pub kickoff: &'static str,
    pub owner_state: OwnerState,
    pub mcp_readiness: &'static str,
    pub public_enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReconciledClaudeStartupView {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub owner_state: OwnerState,
    pub startup: &'static str,
    pub cleanup: &'static str,
    pub public_enabled: bool,
}

/// Target selectors only. The authenticated lead's team, seat and generation
/// are derived from the current canonical capability and never accepted here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InspectRemoteInput {
    pub target_seat: String,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResolveRemoteInput {
    pub target_seat: String,
    pub expected_generation: u64,
    pub resolution: ResolutionRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", content = "input", rename_all = "snake_case", deny_unknown_fields)]
pub enum TeamControlRequest {
    Catalog,
    ListRepositories,
    SaveRepository(SaveRepositoryInput),
    Create(CreateTeamInput),
    ListPending,
    ListTeams,
    BootstrapSeat(BootstrapSeatInput),
    StopSeat(StopSeatInput),
    RetireSeat(RetireSeatInput),
    ReconcileStoppedRetirement(RetireSeatInput),
    ValidateCheckpoint(ValidateCheckpointInput),
    ClaudeStartupSmoke(ClaudeStartupSmokeInput),
    ClaudeInboxProbe(ClaudeStartupSmokeInput),
    ReconcileClaudeStartup(ClaudeStartupSmokeInput),
    Approve(ActivateTeamInput),
    Cancel(CancelPendingInput),
    Checkpoint(WriteCheckpointInput),
    InspectRemote(InspectRemoteInput),
    ResolveRemote(ResolveRemoteInput),
    Replace(AgentReplaceInput),
    Archive(ArchiveTeamInput),
    RollbackArchive(ArchiveTeamInput),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "action", content = "result", rename_all = "snake_case")]
pub enum TeamControlResponse {
    Catalog(TeamCatalog),
    ListRepositories(RepositoryRegistryView),
    SaveRepository(RepositoryRegistryView),
    Create(CreateTeamResult),
    ListPending(Vec<TeamView>),
    ListTeams(Vec<TeamView>),
    BootstrapSeat(BootstrapView),
    StopSeat(StopSeatView),
    RetireSeat(RetireSeatView),
    ReconcileStoppedRetirement(RetireSeatView),
    ValidateCheckpoint(ValidateCheckpointView),
    ClaudeStartupSmoke(ClaudeStartupSmokeView),
    ClaudeInboxProbe(ClaudeInboxProbeView),
    ReconcileClaudeStartup(ReconciledClaudeStartupView),
    Approve(TeamView),
    Cancel(CancelPendingResult),
    Checkpoint(CheckpointReceipt),
    InspectRemote(RemoteInventoryView),
    ResolveRemote(ResolutionReceipt),
    Replace(ReplacementView),
    Archive(ArchiveView),
    RollbackArchive(TeamView),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagedSeatState {
    Active { team: String, generation: u64 },
    Pending,
    Failed,
    Archived,
}

#[derive(Debug, Clone)]
pub struct TeamPaths {
    home: PathBuf,
    project: PathBuf,
    teams: PathBuf,
    staging: PathBuf,
    agents: PathBuf,
    owners: PathBuf,
    team_locks: PathBuf,
    shipped_presets: PathBuf,
    local_presets: PathBuf,
    roles: PathBuf,
}

impl TeamPaths {
    pub fn new(home: PathBuf, project: PathBuf) -> Self {
        let teams = home.join(".aperture/teams");
        Self {
            staging: teams.join(".staging"),
            local_presets: teams.join("presets"),
            agents: home.join(".claude/aperture"),
            owners: home.join(".aperture/run/owner"),
            team_locks: home.join(".aperture/run/team-locks"),
            shipped_presets: project.join("teams/presets"),
            roles: project.join("roles"),
            home,
            project,
            teams,
        }
    }

    fn ensure_runtime_roots(&self) -> TeamResult<()> {
        for path in [&self.teams, &self.staging, &self.local_presets, &self.agents, &self.owners, &self.team_locks] {
            ensure_private_dir(path).map_err(TeamError::from_message)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TeamEngine {
    pub paths: TeamPaths,
}

impl TeamEngine {
    pub fn new(home: PathBuf, project: PathBuf) -> Self {
        Self { paths: TeamPaths::new(home, project) }
    }

    fn now() -> String { Utc::now().to_rfc3339() }

    pub fn catalog(&self) -> TeamResult<TeamCatalog> {
        let roles = self.role_catalog()?;
        Ok(TeamCatalog {
            roles,
            repositories: repository_catalog(&self.paths.home)?,
            execution_tuples: execution_catalog(),
            limits: TeamLimits {
                max_seats: MAX_SEATS,
                max_fallbacks: MAX_FALLBACKS,
                max_role_skills: MAX_ROLE_SKILLS,
                max_preset_bytes: MAX_PRESET_BYTES,
                max_template_bytes: MAX_TEMPLATE_BYTES,
                max_rendered_seat_bytes: MAX_RENDERED_SEAT_BYTES,
                max_rendered_team_bytes: MAX_RENDERED_TEAM_BYTES,
                max_display_scalars: MAX_DISPLAY_SCALARS,
                max_display_bytes: MAX_DISPLAY_BYTES,
                max_mission_scalars: MAX_MISSION_SCALARS,
                max_mission_bytes: MAX_MISSION_BYTES,
            },
        })
    }

    fn role_catalog(&self) -> TeamResult<Vec<RoleCatalogEntry>> {
        validate_repo_root(&self.paths.project)?;
        validate_repo_root(&self.paths.roles)?;
        let mut roles = Vec::new();
        let entries = fs::read_dir(&self.paths.roles).map_err(|_| TeamError::preset("role catalog unavailable"))?;
        for entry in entries {
            let entry = entry.map_err(|_| TeamError::preset("role catalog unreadable"))?;
            let id = entry.file_name().to_string_lossy().to_string();
            if !valid_short_id(&id, 10) { continue; }
            let role = entry.path();
            if !real_dir_inside(&self.paths.roles, &role)
                || !real_file_inside(&role, &role.join("prompt.md.tmpl"))
                || !real_file_inside(&role, &role.join("core/SKILL.md"))
                || !real_file_inside(&role, &role.join("skills.txt"))
            {
                continue;
            }
            roles.push(RoleCatalogEntry { id: id.clone(), display_name: title_case(&id) });
        }
        roles.sort_by(|a, b| a.id.cmp(&b.id));
        if roles.is_empty() { return Err(TeamError::preset("role catalog has no valid roles")); }
        Ok(roles)
    }

    pub fn list_presets(&self) -> TeamResult<Vec<TeamPreset>> {
        let role_ids: HashSet<String> = self.role_catalog()?.into_iter().map(|r| r.id).collect();
        let mut merged = BTreeMap::<String, TeamPreset>::new();
        self.read_preset_dir(&self.paths.shipped_presets, PresetSource::Shipped, &role_ids, &mut merged)?;
        if self.paths.local_presets.exists() {
            self.read_preset_dir(&self.paths.local_presets, PresetSource::Local, &role_ids, &mut merged)?;
        }
        Ok(merged.into_values().collect())
    }

    fn read_preset_dir(
        &self,
        root: &Path,
        source: PresetSource,
        role_ids: &HashSet<String>,
        merged: &mut BTreeMap<String, TeamPreset>,
    ) -> TeamResult<()> {
        if !root.exists() {
            return if source == PresetSource::Shipped { Err(TeamError::preset("shipped preset catalog unavailable")) } else { Ok(()) };
        }
        match source {
            PresetSource::Shipped => validate_repo_root(root)?,
            PresetSource::Local => ensure_private_dir(root).map_err(TeamError::from_message)?,
        }
        for entry in fs::read_dir(root).map_err(|_| TeamError::preset("preset catalog unreadable"))? {
            let entry = entry.map_err(|_| TeamError::preset("preset catalog unreadable"))?;
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) != Some("json") { continue; }
            let id = path.file_stem().and_then(|v| v.to_str()).ok_or_else(|| TeamError::preset("invalid preset filename"))?;
            if !valid_short_id(id, 31) { return Err(TeamError::preset("invalid preset id")); }
            match source {
                PresetSource::Shipped if !real_file_inside(root, &path) => return Err(TeamError::preset("unsafe shipped preset")),
                PresetSource::Local => crate::journal::validate_private_file(&path).map_err(TeamError::from_message)?,
                _ => {}
            }
            let bytes = read_bounded(&path, MAX_PRESET_BYTES)?;
            let stored: StoredPreset = serde_json::from_slice(&bytes).map_err(|_| TeamError::preset("malformed preset"))?;
            if stored.id != id { return Err(TeamError::preset("preset id does not match filename")); }
            if let Some(declared) = &stored.source {
                if *declared != source { return Err(TeamError::preset("preset source does not match catalog")); }
            }
            validate_preset_fields(
                stored.schema_version,
                &stored.id,
                &stored.display_name,
                &stored.mission_placeholder,
                &stored.acceptance_placeholder,
                &stored.seats,
                stored.lead_index,
                &stored.fallbacks,
                role_ids,
            )?;
            merged.insert(stored.id.clone(), stored.into_public(source.clone(), sha256(&bytes)));
        }
        Ok(())
    }

    pub fn save_preset(&self, actor: &AuthenticatedActor, input: SavePresetInput) -> TeamResult<TeamPreset> {
        if actor.principal() != "operator" { return Err(TeamError::state("operator context required")); }
        self.paths.ensure_runtime_roots()?;
        let role_ids: HashSet<String> = self.role_catalog()?.into_iter().map(|r| r.id).collect();
        let p = &input.preset;
        validate_preset_fields(p.schema_version, &p.id, &p.display_name, &p.mission_placeholder, &p.acceptance_placeholder, &p.seats, p.lead_index, &p.fallbacks, &role_ids)?;
        let all = self.list_presets()?;
        let existing = all.iter().find(|value| value.id == p.id);
        match (existing, input.expected_sha256.as_deref()) {
            (None, None) => {}
            (Some(current), Some(expected)) if expected == current.sha256 => {}
            _ => return Err(TeamError::new("E_PRESET_CONFLICT", "preset changed or already exists")),
        }
        let path = self.paths.local_presets.join(format!("{}.json", p.id));
        if path.exists() { crate::journal::validate_private_file(&path).map_err(TeamError::from_message)?; }
        let mut bytes = serde_json::to_vec_pretty(p).map_err(|_| TeamError::preset("preset cannot be serialized"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_PRESET_BYTES { return Err(TeamError::budget("preset exceeds byte budget")); }
        write_private_bytes_atomic(&path, &bytes, path.exists()).map_err(TeamError::from_message)?;
        Ok(TeamPreset {
            schema_version: p.schema_version,
            id: p.id.clone(), display_name: p.display_name.clone(),
            mission_placeholder: p.mission_placeholder.clone(), acceptance_placeholder: p.acceptance_placeholder.clone(),
            seats: p.seats.clone(), lead_index: p.lead_index, fallbacks: p.fallbacks.clone(),
            source: PresetSource::Local, sha256: sha256(&bytes),
        })
    }

    pub fn create_team(&self, actor: &AuthenticatedActor, input: CreateTeamInput) -> TeamResult<CreateTeamResult> {
        if actor.principal() != "operator" && !actor.is_glados() {
            return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "operator or authenticated glados context required"));
        }
        if actor.is_glados() { actor.revalidate_before_mutation().map_err(TeamError::from_message)?; }
        self.paths.ensure_runtime_roots()?;
        validate_team_name(&input.team)?;
        if !valid_project_label(&input.project) { return Err(TeamError::name("project label is malformed")); }
        repository_registry::resolve_selected(&self.paths.home, &input.project, &input.repo)?;
        validate_text(&input.mission, MAX_MISSION_SCALARS, MAX_MISSION_BYTES, "mission")?;
        validate_text(&input.acceptance, MAX_MISSION_SCALARS, MAX_MISSION_BYTES, "acceptance")?;
        let role_ids: HashSet<String> = self.role_catalog()?.into_iter().map(|r| r.id).collect();
        if input.seats.is_empty() || input.seats.len() > MAX_SEATS || input.lead_index >= input.seats.len() {
            return Err(TeamError::preset("invalid seat count or lead index"));
        }
        if input.fallbacks.len() > MAX_FALLBACKS { return Err(TeamError::budget("too many fallbacks")); }
        for seat in &input.seats {
            if !role_ids.contains(&seat.role) { return Err(TeamError::preset("unknown role")); }
            validate_execution_tuple(&seat.tuple())?;
        }
        for fallback in &input.fallbacks { validate_execution_tuple(fallback)?; }

        let presets = self.list_presets()?;
        let preset = match input.preset_id.as_deref() {
            Some(id) => Some(presets.iter().find(|p| p.id == id).ok_or_else(|| TeamError::preset("selected preset is unavailable"))?),
            None => None,
        };
        let seats = derive_seats(&input.team, &input.seats)?;
        self.validate_collisions(&input.team, &seats)?;
        if actor.is_glados() { actor.revalidate_before_mutation().map_err(TeamError::from_message)?; }
        let _team_lock = try_lock(&self.paths.team_locks, &input.team).map_err(TeamError::from_message)?;
        // All admissions take team -> catalog; the catalog writer never takes a team lock.
        let _catalog_lock = repository_registry::lock(&self.paths.home)?;
        repository_registry::resolve_selected(&self.paths.home, &input.project, &input.repo)?;
        self.validate_collisions(&input.team, &seats)?;

        let staging_uuid = Uuid::new_v4().to_string();
        let request_id = Uuid::new_v4().to_string();
        let now = Self::now();
        let snapshot = TeamSnapshot {
            schema_version: 1,
            team: input.team.clone(),
            project: input.project.clone(),
            repo: input.repo.clone(),
            mission: input.mission.clone(),
            acceptance: input.acceptance.clone(),
            preset: PresetSnapshotRef { id: preset.map(|p| p.id.clone()), sha256: preset.map(|p| p.sha256.clone()) },
            lead: seats[input.lead_index].name.clone(),
            seats,
            fallbacks: input.fallbacks.clone(),
            grants: Vec::new(),
            created_at: now.clone(),
            creation_request_id: request_id.clone(),
            staging_uuid: staging_uuid.clone(),
        };
        let snapshot_bytes = json_bytes(&snapshot)?;
        let request = CreationRequestDTO {
            schema_version: 1,
            request_id,
            team: input.team.clone(),
            project: input.project.clone(),
            repo: input.repo.clone(),
            snapshot_sha256: sha256(&snapshot_bytes),
            expected_generation: 0,
            created_at: now.clone(),
        };
        let state = TeamStateFile {
            schema_version: 1,
            state: TeamLifecycle::Pending,
            generation: 0,
            epic_id: None,
            failure: None,
            updated_at: now,
        };

        let stage_root = self.paths.staging.join(&staging_uuid);
        let staged_team = stage_root.join("team");
        let result = (|| {
            if actor.is_glados() { actor.revalidate_before_mutation().map_err(TeamError::from_message)?; }
            ensure_private_dir(&staged_team).map_err(TeamError::from_message)?;
            ensure_private_dir(&stage_root.join("seats")).map_err(TeamError::from_message)?;
            write_private_bytes_atomic(&staged_team.join("team.json"), &snapshot_bytes, false).map_err(TeamError::from_message)?;
            write_private_json_atomic(&staged_team.join("state.json"), &state, false).map_err(TeamError::from_message)?;
            write_private_json_atomic(&staged_team.join("activation_request.json"), &request, false).map_err(TeamError::from_message)?;

            let mut total_bytes = snapshot_bytes.len();
            for seat in &snapshot.seats {
                total_bytes = total_bytes.checked_add(self.render_seat(&snapshot, seat, &stage_root.join("seats").join(&seat.name))?)
                    .ok_or_else(|| TeamError::budget("rendered team byte budget overflow"))?;
                if total_bytes > MAX_RENDERED_TEAM_BYTES { return Err(TeamError::budget("rendered team exceeds byte budget")); }
            }
            sync_tree(&stage_root)?;
            let destination = self.paths.teams.join(&input.team);
            if actor.is_glados() { actor.revalidate_before_mutation().map_err(TeamError::from_message)?; }
            rename_no_replace(&staged_team, &destination).map_err(TeamError::from_message)?;
            sync_parent(&staged_team).map_err(TeamError::from_message)?;
            sync_parent(&destination).map_err(TeamError::from_message)?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = remove_private_tree(&stage_root, &self.paths.staging);
            return Err(error);
        }
        let view = self.read_team_view(&input.team)?;
        Ok(CreateTeamResult { team: view, creation_request: request })
    }

    fn validate_collisions(&self, team: &str, seats: &[TeamSeat]) -> TeamResult<()> {
        for path in [self.paths.teams.join(team), self.paths.teams.join("archive").join(team)] {
            if fs::symlink_metadata(&path).is_ok() { return Err(TeamError::new("E_NAME_COLLISION", "team name already exists")); }
        }
        for seat in seats {
            if fs::symlink_metadata(self.paths.agents.join(&seat.name)).is_ok()
                || fs::symlink_metadata(self.paths.owners.join(format!("{}.json", seat.name))).is_ok()
            {
                return Err(TeamError::new("E_NAME_COLLISION", "seat name already exists"));
            }
        }
        Ok(())
    }

    fn render_seat(&self, team: &TeamSnapshot, seat: &TeamSeat, destination: &Path) -> TeamResult<usize> {
        let role_root = self.paths.roles.join(&seat.role);
        if !real_dir_inside(&self.paths.roles, &role_root) { return Err(TeamError::new("E_PATH_UNSAFE", "role source escaped catalog")); }
        let template_path = role_root.join("prompt.md.tmpl");
        let core_path = role_root.join("core/SKILL.md");
        let skills_path = role_root.join("skills.txt");
        for path in [&template_path, &core_path, &skills_path] {
            if !real_file_inside(&role_root, path) { return Err(TeamError::new("E_PATH_UNSAFE", "role source is unsafe")); }
        }
        let template = String::from_utf8(read_bounded(&template_path, MAX_TEMPLATE_BYTES)?).map_err(|_| TeamError::preset("role template is not UTF-8"))?;
        validate_template_grammar(&template)?;
        let mut prompt = template;
        for (key, value) in [
            ("seat_name", seat.name.as_str()),
            ("team_name", team.team.as_str()),
            ("project", team.project.as_str()),
            ("lead_name", team.lead.as_str()),
            ("role", seat.role.as_str()),
        ] {
            prompt = prompt.replace(&format!("{{{{{key}}}}}"), value);
        }
        if prompt.contains("{{") || prompt.contains("}}") || prompt.contains("${") {
            return Err(TeamError::preset("template contains unresolved or executable syntax"));
        }
        let mission_json = serde_json::to_string(&serde_json::json!({
            "mission": team.mission,
            "acceptance": team.acceptance,
        })).map_err(|_| TeamError::preset("mission cannot be delimited"))?;
        prompt.push_str("\n\n# Assigned mission (bounded JSON data)\n\n");
        prompt.push_str(&mission_json);
        prompt.push('\n');
        match seat.harness {
            Harness::Claude => prompt.push_str("\n# Inbox protocol\n\nUse the Claude hook-delivered BEADS inbox. Process each message before acknowledgement. Never start the Codex bridge.\n"),
            Harness::Codex => prompt.push_str("\n# Inbox protocol\n\nThe Aperture app-server bridge delivers BEADS turns. Use get_messages and acknowledge each processed message. Never start a Claude monitor.\n"),
        }

        ensure_private_dir(destination).map_err(TeamError::from_message)?;
        ensure_private_dir(&destination.join("skills")).map_err(TeamError::from_message)?;
        let manifest_model = match seat.harness { Harness::Claude => seat.model.clone(), Harness::Codex => format!("codex/{}", seat.model) };
        let manifest = serde_json::json!({
            "name": seat.name,
            "emoji": "",
            "model": manifest_model,
            "window": seat.name,
            "role": seat.role,
            "kind": match seat.harness { Harness::Claude => "claude-code", Harness::Codex => "codex" },
            "enabled": true
        });
        let manifest_bytes = json_value_bytes(&manifest)?;
        let team_marker = json_value_bytes(&serde_json::json!({"team":team.team,"role":seat.role,"schema_version":1}))?;
        let resident = format!("constitution\n{}-core\n", seat.role);
        let source_skills = parse_role_skills(&String::from_utf8(read_bounded(&skills_path, MAX_TEMPLATE_BYTES)?).map_err(|_| TeamError::preset("skills catalog is not UTF-8"))?, &seat.role)?;
        let skills_file = format!("{}\n", source_skills.join("\n"));
        let files: [(&str, Vec<u8>); 5] = [
            ("manifest.json", manifest_bytes),
            ("prompt.md", prompt.into_bytes()),
            ("TEAM", team_marker),
            ("resident.txt", resident.into_bytes()),
            ("skills.txt", skills_file.into_bytes()),
        ];
        let mut total = 0usize;
        for (name, bytes) in files {
            total = total.checked_add(bytes.len()).ok_or_else(|| TeamError::budget("seat byte budget overflow"))?;
            if total > MAX_RENDERED_SEAT_BYTES { return Err(TeamError::budget("rendered seat exceeds byte budget")); }
            write_private_bytes_atomic(&destination.join(name), &bytes, false).map_err(TeamError::from_message)?;
        }

        let role_core_dest = destination.join("skills").join(format!("{}-core", seat.role));
        total = total.checked_add(copy_private_tree_bounded(&role_root.join("core"), &role_core_dest, MAX_RENDERED_SEAT_BYTES - total)?)
            .ok_or_else(|| TeamError::budget("seat byte budget overflow"))?;
        for skill in source_skills {
            if skill == format!("{}-core", seat.role) { continue; }
            let source = self.paths.project.join(".claude/skills").join(&skill);
            if !real_dir_inside(&self.paths.project.join(".claude/skills"), &source) {
                return Err(TeamError::preset("role references an unknown skill"));
            }
            let dest = destination.join("skills").join(&skill);
            let remaining = MAX_RENDERED_SEAT_BYTES.checked_sub(total).ok_or_else(|| TeamError::budget("rendered seat exceeds byte budget"))?;
            total = total.checked_add(copy_private_tree_bounded(&source, &dest, remaining)?)
                .ok_or_else(|| TeamError::budget("seat byte budget overflow"))?;
        }
        if total > MAX_RENDERED_SEAT_BYTES { return Err(TeamError::budget("rendered seat exceeds byte budget")); }
        sync_tree(destination)?;
        Ok(total)
    }

    pub fn list_teams(&self) -> TeamResult<Vec<TeamView>> {
        self.paths.ensure_runtime_roots()?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.paths.teams).map_err(|_| TeamError::io("team registry unreadable"))? {
            let entry = entry.map_err(|_| TeamError::io("team registry unreadable"))?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || matches!(name.as_str(), "archive" | "presets") { continue; }
            if !valid_short_id(&name, 16) || !real_dir_inside(&self.paths.teams, &entry.path()) {
                return Err(TeamError::new("E_PATH_UNSAFE", "team registry contains an unsafe entry"));
            }
            names.push(name);
        }
        names.sort();
        names.into_iter().map(|name| self.read_team_view(&name)).collect()
    }

    fn read_team_view(&self, team: &str) -> TeamResult<TeamView> {
        validate_team_name(team)?;
        let dir = self.paths.teams.join(team);
        if !dir.exists() { return Err(TeamError::new("E_TEAM_NOT_FOUND", "team is not registered")); }
        if !real_dir_inside(&self.paths.teams, &dir) { return Err(TeamError::new("E_PATH_UNSAFE", "team directory is unsafe")); }
        let snapshot: TeamSnapshot = read_private_json(&dir.join("team.json")).map_err(TeamError::from_message)?;
        let state: TeamStateFile = read_private_json(&dir.join("state.json")).map_err(TeamError::from_message)?;
        validate_stored_team(team, &snapshot, &state)?;
        let mut seats = Vec::with_capacity(snapshot.seats.len());
        for configured in &snapshot.seats {
            let observed_owner = match state.state {
                TeamLifecycle::Active => {
                    let owner = OwnerStore::new(self.paths.owners.clone()).summary(&configured.name).map_err(TeamError::from_message)?;
                    let snapshot_tuple = configured.tuple();
                    if (owner.generation == 0 && owner.configured != snapshot_tuple)
                        || (owner.generation > 0 && owner.configured != snapshot_tuple && !snapshot.fallbacks.contains(&owner.configured))
                    {
                        return Err(TeamError::state("owner requested tuple is outside the immutable team policy"));
                    }
                    Some(owner)
                }
                _ => None,
            };
            seats.push(TeamSeatView { configured: configured.clone(), observed_owner });
        }
        let capabilities = team_capabilities(&state.state, &seats);
        Ok(TeamView {
            snapshot,
            state,
            seats,
            capabilities,
        })
    }

    pub fn cancel_pending(&self, actor: &AuthenticatedActor, input: CancelPendingInput) -> TeamResult<CancelPendingResult> {
        if actor.principal() != "operator" && !actor.is_glados() {
            return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "operator or exact glados context required"));
        }
        self.paths.ensure_runtime_roots()?;
        validate_team_name(&input.team)?;
        let _team_lock = try_lock(&self.paths.team_locks, &input.team).map_err(TeamError::from_message)?;
        if !self.paths.teams.join(&input.team).exists() {
            if let Some(id) = self.find_rejected(&input)? {
                return Ok(CancelPendingResult { team: input.team, cancelled: true, rejected_snapshot_id: id });
            }
            return Err(TeamError::new("E_TEAM_NOT_FOUND", "pending team is not registered"));
        }
        let view = self.read_team_view(&input.team)?;
        if view.state.state != TeamLifecycle::Pending || view.state.generation != input.expected_generation {
            return Err(TeamError::new("E_GENERATION_MISMATCH", "pending team generation changed"));
        }
        if view.snapshot.creation_request_id != input.creation_request_id {
            return Err(TeamError::state("activation request changed"));
        }
        let team_dir = self.paths.teams.join(&input.team);
        if fs::symlink_metadata(team_dir.join("journal.json")).is_ok() {
            return Err(TeamError::state("activation is already in progress"));
        }
        let request: CreationRequestDTO = read_private_json(&team_dir.join("activation_request.json")).map_err(TeamError::from_message)?;
        validate_request(&view.snapshot, &view.state, &request)?;
        let rejected_root = self.paths.staging.join("rejected").join(&view.snapshot.staging_uuid);
        ensure_private_dir(&rejected_root).map_err(TeamError::from_message)?;
        let destination = rejected_root.join("snapshot");
        rename_no_replace(&team_dir, &destination).map_err(TeamError::from_message)?;
        sync_parent(&team_dir).map_err(TeamError::from_message)?;
        sync_parent(&destination).map_err(TeamError::from_message)?;
        let stage = self.paths.staging.join(&view.snapshot.staging_uuid);
        remove_private_tree(&stage, &self.paths.staging)?;
        Ok(CancelPendingResult { team: input.team, cancelled: true, rejected_snapshot_id: view.snapshot.staging_uuid })
    }

    fn find_rejected(&self, input: &CancelPendingInput) -> TeamResult<Option<String>> {
        let root = self.paths.staging.join("rejected");
        if !root.exists() { return Ok(None); }
        ensure_private_dir(&root).map_err(TeamError::from_message)?;
        let mut seen = 0usize;
        for entry in fs::read_dir(&root).map_err(|_| TeamError::io("rejected snapshots unreadable"))? {
            seen += 1;
            if seen > 1_024 { return Err(TeamError::state("rejected snapshot index is ambiguous")); }
            let entry = entry.map_err(|_| TeamError::io("rejected snapshots unreadable"))?;
            let id = entry.file_name().to_string_lossy().to_string();
            if Uuid::parse_str(&id).is_err() || !real_dir_inside(&root, &entry.path()) { return Err(TeamError::new("E_PATH_UNSAFE", "invalid rejected snapshot")); }
            let snapshot_dir = entry.path().join("snapshot");
            let snapshot: TeamSnapshot = match read_private_json(&snapshot_dir.join("team.json")) { Ok(v) => v, Err(_) => continue };
            let state: TeamStateFile = read_private_json(&snapshot_dir.join("state.json")).map_err(TeamError::from_message)?;
            if snapshot.team == input.team && snapshot.creation_request_id == input.creation_request_id {
                if state.generation != input.expected_generation || state.state != TeamLifecycle::Pending {
                    return Err(TeamError::state("rejected snapshot identity mismatch"));
                }
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    pub(crate) fn activate(&self, actor: &AuthenticatedActor, input: ActivateTeamInput) -> TeamResult<TeamView> {
        if !actor.is_glados() { return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "exact glados actor required")); }
        validate_team_name(&input.team)?;
        validate_epic_id(&input.epic_id)?;
        // This must remain before root creation and lock acquisition: the
        // canonical bearer path may have been atomically replaced since open.
        actor.revalidate_before_mutation().map_err(TeamError::from_message)?;
        self.paths.ensure_runtime_roots()?;
        let _team_lock = try_lock(&self.paths.team_locks, &input.team).map_err(TeamError::from_message)?;
        let mut view = self.read_team_view(&input.team)?;
        if view.snapshot.creation_request_id != input.creation_request_id {
            return Err(TeamError::state("activation request changed"));
        }
        let team_dir = self.paths.teams.join(&input.team);
        let journal_path = team_dir.join("journal.json");
        let request: CreationRequestDTO = read_private_json(&team_dir.join("activation_request.json")).map_err(TeamError::from_message)?;
        validate_request(&view.snapshot, &view.state, &request)?;
        if view.state.state == TeamLifecycle::Active {
            if view.state.generation == input.expected_generation + 1 && view.state.epic_id.as_deref() == Some(&input.epic_id) {
                if journal_path.exists() {
                    let mut seat_names: Vec<_> = view.snapshot.seats.iter().map(|seat| seat.name.clone()).collect();
                    seat_names.sort();
                    let owner_store = OwnerStore::new(self.paths.owners.clone());
                    let mut seat_locks: Vec<AdvisoryLock> = Vec::with_capacity(seat_names.len());
                    for seat in &seat_names { seat_locks.push(owner_store.lock(seat).map_err(TeamError::from_message)?); }
                    let roots = JournalRoots {
                        teams: self.paths.teams.clone(), staging: self.paths.staging.clone(),
                        agents: self.paths.agents.clone(), owner: self.paths.owners.clone(),
                    };
                    apply_or_recover_journal(&journal_path, &roots).map_err(TeamError::from_message)?;
                    for seat in &seat_names {
                        let marker = self.paths.agents.join(seat).join(".complete");
                        if !marker.exists() {
                            write_private_bytes_atomic(&marker, b"complete\n", false).map_err(TeamError::from_message)?;
                        }
                    }
                    remove_journal(&journal_path).map_err(TeamError::from_message)?;
                    remove_private_tree(&self.paths.staging.join(&view.snapshot.staging_uuid), &self.paths.staging)?;
                    drop(seat_locks);
                    return self.read_team_view(&input.team);
                }
                return Ok(view);
            }
            return Err(TeamError::new("E_GENERATION_MISMATCH", "team is already active with different authority"));
        }
        if view.state.state != TeamLifecycle::Pending || view.state.generation != input.expected_generation {
            return Err(TeamError::new("E_GENERATION_MISMATCH", "pending team generation changed"));
        }
        // Availability is deliberately checked again under the team lock. A
        // catalog entry disappearing between create and approval must not
        // publish a team whose immutable repository cannot be resolved.
        let _catalog_lock = repository_registry::lock(&self.paths.home)?;
        repository_registry::resolve_selected(&self.paths.home, &view.snapshot.project, &view.snapshot.repo)?;

        let mut seat_names: Vec<_> = view.snapshot.seats.iter().map(|seat| seat.name.clone()).collect();
        seat_names.sort();
        let owner_store = OwnerStore::new(self.paths.owners.clone());
        let mut _seat_locks: Vec<AdvisoryLock> = Vec::with_capacity(seat_names.len());
        for seat in &seat_names { _seat_locks.push(owner_store.lock(seat).map_err(TeamError::from_message)?); }
        if !journal_path.exists() {
            let owner_stage = self.paths.staging.join(&view.snapshot.staging_uuid).join("owners");
            ensure_private_dir(&owner_stage).map_err(TeamError::from_message)?;
            for seat in &view.snapshot.seats {
                let record = OwnerStore::initial_record(actor, &seat.name, seat.tuple()).map_err(TeamError::from_message)?;
                write_private_json_atomic(&owner_stage.join(format!("{}.json", seat.name)), &record, false).map_err(TeamError::from_message)?;
            }
            sync_tree(&owner_stage)?;
            let mut moves = Vec::with_capacity(view.snapshot.seats.len() * 2);
            for seat in &seat_names {
                moves.push(JournalMove {
                    from_root: JournalRoot::Staging,
                    from_rel: format!("{}/seats/{seat}", view.snapshot.staging_uuid),
                    to_root: JournalRoot::Agents,
                    to_rel: seat.clone(),
                    kind: JournalObjectKind::Directory,
                });
            }
            for seat in &seat_names {
                moves.push(JournalMove {
                    from_root: JournalRoot::Staging,
                    from_rel: format!("{}/owners/{seat}.json", view.snapshot.staging_uuid),
                    to_root: JournalRoot::Owner,
                    to_rel: format!("{seat}.json"),
                    kind: JournalObjectKind::File,
                });
            }
            write_journal(
                &journal_path,
                &Journal {
                    schema_version: 1,
                    operation: JournalOperation::Activate,
                    team: input.team.clone(),
                    uuid: view.snapshot.staging_uuid.clone(),
                    moves,
                    step: 0,
                    preimage_sha256: request.snapshot_sha256.clone(),
                    archive_approval: None,
                    archive_preimage: vec![],
                    archive_mutable_files: vec![],
                },
            )
            .map_err(TeamError::from_message)?;
        }
        let roots = JournalRoots {
            teams: self.paths.teams.clone(), staging: self.paths.staging.clone(),
            agents: self.paths.agents.clone(), owner: self.paths.owners.clone(),
        };
        if let Err(error) = apply_or_recover_journal(&journal_path, &roots) {
            let completed_moves = crate::journal::read_journal(&journal_path).map(|j| j.step).unwrap_or(0);
            view.state.state = TeamLifecycle::Failed;
            view.state.failure = Some(TeamFailure { code: "E_JOURNAL_INCONSISTENT".into(), completed_moves });
            view.state.updated_at = Self::now();
            let _ = write_private_json_atomic(&team_dir.join("state.json"), &view.state, true);
            return Err(TeamError::from_message(error));
        }
        for seat in &seat_names {
            let marker = self.paths.agents.join(seat).join(".complete");
            if !marker.exists() {
                write_private_bytes_atomic(&marker, b"complete\n", false).map_err(TeamError::from_message)?;
            }
        }
        view.state.state = TeamLifecycle::Active;
        view.state.generation = input.expected_generation + 1;
        view.state.epic_id = Some(input.epic_id);
        view.state.failure = None;
        view.state.updated_at = Self::now();
        write_private_json_atomic(&team_dir.join("state.json"), &view.state, true).map_err(TeamError::from_message)?;
        sync_dir(&team_dir).map_err(TeamError::from_message)?;
        remove_journal(&journal_path).map_err(TeamError::from_message)?;
        let stage = self.paths.staging.join(&view.snapshot.staging_uuid);
        remove_private_tree(&stage, &self.paths.staging)?;
        drop(_seat_locks);
        self.read_team_view(&input.team)
    }
}

fn execution_catalog() -> Vec<ExecutionTuple> {
    let mut out = vec![
        ExecutionTuple { harness: Harness::Claude, model: "opus".into(), reasoning: None },
        ExecutionTuple { harness: Harness::Claude, model: "sonnet".into(), reasoning: None },
        ExecutionTuple { harness: Harness::Claude, model: "claude-sonnet-5".into(), reasoning: None },
        ExecutionTuple { harness: Harness::Claude, model: "claude-fable-5-1".into(), reasoning: None },
        ExecutionTuple { harness: Harness::Claude, model: "claude-opus-5".into(), reasoning: None },
    ];
    let efforts = [ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Xhigh, ReasoningEffort::Max, ReasoningEffort::Ultra];
    for model in ["gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra"] {
        for effort in &efforts { out.push(ExecutionTuple { harness: Harness::Codex, model: model.into(), reasoning: Some(effort.clone()) }); }
    }
    for effort in efforts.iter().take(5) { out.push(ExecutionTuple { harness: Harness::Codex, model: "gpt-5.6-luna".into(), reasoning: Some(effort.clone()) }); }
    for effort in efforts.iter().take(4) { out.push(ExecutionTuple { harness: Harness::Codex, model: "gpt-5.5".into(), reasoning: Some(effort.clone()) }); }
    out
}

fn validate_execution_tuple(value: &ExecutionTuple) -> TeamResult<()> {
    if !valid_model(&value.model) || !execution_catalog().contains(value) {
        return Err(TeamError::preset("execution tuple is not in the allowlist"));
    }
    Ok(())
}

fn validate_preset_fields(
    schema_version: u32,
    id: &str,
    display_name: &str,
    mission: &str,
    acceptance: &str,
    seats: &[PresetSeat],
    lead_index: usize,
    fallbacks: &[ExecutionTuple],
    role_ids: &HashSet<String>,
) -> TeamResult<()> {
    if schema_version != 1 || !valid_short_id(id, 31) { return Err(TeamError::preset("unsupported preset schema or id")); }
    validate_text(display_name, MAX_DISPLAY_SCALARS, MAX_DISPLAY_BYTES, "display name")?;
    validate_text(mission, MAX_DISPLAY_SCALARS, MAX_DISPLAY_BYTES, "mission placeholder")?;
    validate_text(acceptance, MAX_DISPLAY_SCALARS, MAX_DISPLAY_BYTES, "acceptance placeholder")?;
    if seats.is_empty() || seats.len() > MAX_SEATS || lead_index >= seats.len() { return Err(TeamError::preset("invalid seat count or lead index")); }
    if fallbacks.len() > MAX_FALLBACKS { return Err(TeamError::budget("too many fallbacks")); }
    for seat in seats {
        if !role_ids.contains(&seat.role) || !valid_short_id(&seat.role, 10) { return Err(TeamError::preset("unknown role")); }
        validate_execution_tuple(&seat.tuple())?;
    }
    for fallback in fallbacks { validate_execution_tuple(fallback)?; }
    Ok(())
}

fn valid_short_id(value: &str, max: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty() && bytes.len() <= max && bytes.iter().enumerate().all(|(index, byte)| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && (*byte == b'_' || *byte == b'-'))
    })
}

fn valid_model(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty() && bytes.len() <= 80 && bytes.iter().enumerate().all(|(index, byte)| {
        byte.is_ascii_alphanumeric() || (index > 0 && matches!(*byte, b'.' | b'_' | b':' | b'/' | b'-'))
    })
}

fn validate_text(value: &str, max_scalars: usize, max_bytes: usize, field: &str) -> TeamResult<()> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().count() > max_scalars {
        return Err(TeamError::preset(&format!("invalid {field}")));
    }
    if value.chars().any(|c| c.is_control()
        || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    {
        return Err(TeamError::preset(&format!("unsafe {field}")));
    }
    Ok(())
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() { Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(), None => String::new() }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_bounded(path: &Path, max: usize) -> TeamResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| TeamError::io("source file unavailable"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() as usize > max {
        return Err(TeamError::budget("source file exceeds budget or is unsafe"));
    }
    let mut file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path).map_err(|_| TeamError::io("source file unavailable"))?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file).take((max + 1) as u64).read_to_end(&mut bytes).map_err(|_| TeamError::io("source file unreadable"))?;
    if bytes.len() > max { return Err(TeamError::budget("source file exceeds budget")); }
    Ok(bytes)
}

fn validate_repo_root(path: &Path) -> TeamResult<()> {
    let meta = fs::symlink_metadata(path).map_err(|_| TeamError::preset("repository source unavailable"))?;
    if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(TeamError::new("E_PATH_UNSAFE", "repository source is unsafe"));
    }
    Ok(())
}

fn valid_repo_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_project_label(value: &str) -> bool {
    value.strip_prefix("project:").map(valid_repo_key).unwrap_or(false)
}

// A private, previously admitted snapshot keeps its binding independently of
// mutable offers. Disabling an offer must never hide or strand an active team.
pub(crate) fn repository_binding_is_wellformed(project: &str, repo: &str) -> bool {
    valid_project_label(project) && valid_repo_key(repo)
}

fn repository_is_available(home: &Path, repo: &str) -> bool {
    if !valid_repo_key(repo) { return false; }
    let projects = home.join("projects");
    let root = projects.join(repo);
    let Ok(projects_meta) = fs::symlink_metadata(&projects) else { return false; };
    let Ok(root_meta) = fs::symlink_metadata(&root) else { return false; };
    let Ok(git_meta) = fs::symlink_metadata(root.join(".git")) else { return false; };
    let uid = unsafe { libc::geteuid() };
    projects_meta.is_dir()
        && !projects_meta.file_type().is_symlink()
        && projects_meta.uid() == uid
        && projects_meta.mode() & 0o022 == 0
        && root_meta.is_dir()
        && !root_meta.file_type().is_symlink()
        && root_meta.uid() == uid
        && root_meta.mode() & 0o022 == 0
        && !git_meta.file_type().is_symlink()
        && git_meta.uid() == uid
        && git_meta.mode() & 0o022 == 0
        && (git_meta.is_dir() || (git_meta.is_file() && git_meta.nlink() == 1))
}

fn repository_catalog(home: &Path) -> TeamResult<Vec<RepositoryCatalogEntry>> {
    Ok(repository_registry::read(home)?.repositories.into_iter().filter(|r| r.enabled).map(|r| RepositoryCatalogEntry {
        available: repository_is_available(home, &r.repo),
        project: r.project, repo: r.repo, display_name: r.display_name,
    }).collect())
}

/// Resolve an already-admitted immutable snapshot binding, not a new offer.
pub(crate) fn resolve_repository(home: &Path, project: &str, repo: &str) -> TeamResult<PathBuf> {
    if repo.is_empty() { return Err(TeamError::new("E_REPO_REQUIRED", "repository selection is required")); }
    if !repository_binding_is_wellformed(project, repo) {
        return Err(TeamError::new("E_REPO_NOT_IN_CATALOG", "stored repository binding is malformed"));
    }
    if !repository_is_available(home, repo) {
        return Err(TeamError::new("E_REPO_UNAVAILABLE", "selected repository is unavailable"));
    }
    Ok(home.join("projects").join(repo))
}

fn real_dir_inside(root: &Path, path: &Path) -> bool {
    let meta = fs::symlink_metadata(path).ok();
    let canonical = fs::canonicalize(path).ok();
    let root = fs::canonicalize(root).ok();
    matches!(meta, Some(m) if m.is_dir() && !m.file_type().is_symlink() && m.uid() == unsafe { libc::geteuid() })
        && matches!((root, canonical), (Some(r), Some(p)) if p.starts_with(&r))
}

fn real_file_inside(root: &Path, path: &Path) -> bool {
    let meta = fs::symlink_metadata(path).ok();
    let canonical = fs::canonicalize(path).ok();
    let root = fs::canonicalize(root).ok();
    matches!(meta, Some(m) if m.is_file() && !m.file_type().is_symlink() && m.uid() == unsafe { libc::geteuid() } && m.nlink() == 1)
        && matches!((root, canonical), (Some(r), Some(p)) if p.starts_with(&r))
}

fn validate_team_name(value: &str) -> TeamResult<()> {
    if !valid_short_id(value, 16) || matches!(value, "glados" | "wheatley" | "peppy" | "operator" | "watchdog" | "shared" | "archive" | "presets") {
        return Err(TeamError::name("invalid or reserved team id"));
    }
    Ok(())
}

fn derive_seats(team: &str, requested: &[PresetSeat]) -> TeamResult<Vec<TeamSeat>> {
    let mut counts = BTreeMap::<String, usize>::new();
    let mut names = HashSet::new();
    let mut result = Vec::with_capacity(requested.len());
    for seat in requested {
        let count = counts.entry(seat.role.clone()).or_default();
        *count += 1;
        let suffix = if *count == 1 { String::new() } else { format!("-{count}") };
        let name = format!("{team}-{}{}", seat.role, suffix);
        if !is_valid_seat_name(&name)
            || matches!(name.as_str(), "glados" | "wheatley" | "peppy" | "operator" | "watchdog")
            || !names.insert(name.clone())
        {
            return Err(TeamError::name("derived seat name is invalid or collides"));
        }
        result.push(TeamSeat {
            name,
            role: seat.role.clone(),
            harness: seat.harness.clone(),
            model: seat.model.clone(),
            reasoning: seat.reasoning.clone(),
        });
    }
    Ok(result)
}

fn json_bytes<T: Serialize>(value: &T) -> TeamResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|_| TeamError::io("JSON serialization failed"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn json_value_bytes(value: &serde_json::Value) -> TeamResult<Vec<u8>> { json_bytes(value) }

fn validate_template_grammar(template: &str) -> TeamResult<()> {
    let allowed: HashSet<&str> = ["seat_name", "team_name", "project", "lead_name", "role"].into_iter().collect();
    if template.contains("${") || template.contains("{{#") || template.contains("{{>") || template.contains("{{!") {
        return Err(TeamError::preset("template contains expansion/include syntax"));
    }
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| TeamError::preset("unterminated template variable"))?;
        let key = &after[..end];
        if !allowed.contains(key) { return Err(TeamError::preset("template variable is not allowlisted")); }
        rest = &after[end + 2..];
    }
    if rest.contains("}}") { return Err(TeamError::preset("unmatched template delimiter")); }
    Ok(())
}

fn parse_role_skills(text: &str, role: &str) -> TeamResult<Vec<String>> {
    let mut skills = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if !valid_short_id(line, 63) || !seen.insert(line.to_string()) {
            return Err(TeamError::preset("invalid or duplicate role skill id"));
        }
        skills.push(line.to_string());
    }
    if skills.len() > MAX_ROLE_SKILLS
        || !skills.iter().any(|s| s == "constitution")
        || !skills.iter().any(|s| s == &format!("{role}-core"))
    {
        return Err(TeamError::preset("role skill catalog violates resident contract"));
    }
    Ok(skills)
}

pub(crate) fn copy_private_tree_bounded(source: &Path, destination: &Path, budget: usize) -> TeamResult<usize> {
    if !real_dir_inside(source, source) { return Err(TeamError::new("E_PATH_UNSAFE", "skill source is unsafe")); }
    ensure_private_dir(destination).map_err(TeamError::from_message)?;
    let mut entries: Vec<_> = fs::read_dir(source).map_err(|_| TeamError::io("skill source unreadable"))?
        .collect::<Result<Vec<_>, _>>().map_err(|_| TeamError::io("skill source unreadable"))?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut total = 0usize;
    for entry in entries {
        let source_path = entry.path();
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text.is_empty() || name_text == "." || name_text == ".." { return Err(TeamError::new("E_PATH_UNSAFE", "invalid skill path")); }
        let meta = fs::symlink_metadata(&source_path).map_err(|_| TeamError::io("skill source unavailable"))?;
        if meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
            return Err(TeamError::new("E_PATH_UNSAFE", "skill source contains symlink or foreign owner"));
        }
        let dest_path = destination.join(&name);
        if meta.is_dir() {
            let remaining = budget.checked_sub(total).ok_or_else(|| TeamError::budget("skill tree exceeds budget"))?;
            total = total.checked_add(copy_private_tree_bounded(&source_path, &dest_path, remaining)?)
                .ok_or_else(|| TeamError::budget("skill tree exceeds budget"))?;
        } else if meta.is_file() && meta.nlink() == 1 {
            let remaining = budget.checked_sub(total).ok_or_else(|| TeamError::budget("skill tree exceeds budget"))?;
            let bytes = read_bounded(&source_path, remaining)?;
            total = total.checked_add(bytes.len()).ok_or_else(|| TeamError::budget("skill tree exceeds budget"))?;
            write_private_bytes_atomic(&dest_path, &bytes, false).map_err(TeamError::from_message)?;
        } else {
            return Err(TeamError::new("E_PATH_UNSAFE", "skill source contains unsupported object"));
        }
    }
    Ok(total)
}

fn sync_tree(root: &Path) -> TeamResult<()> {
    let mut dirs = vec![root.to_path_buf()];
    let mut index = 0usize;
    while index < dirs.len() {
        let dir = dirs[index].clone();
        index += 1;
        let meta = fs::symlink_metadata(&dir).map_err(|_| TeamError::io("staging tree unavailable"))?;
        if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(TeamError::new("E_PERMISSION_UNSAFE", "unsafe staging directory"));
        }
        for entry in fs::read_dir(&dir).map_err(|_| TeamError::io("staging tree unreadable"))? {
            let path = entry.map_err(|_| TeamError::io("staging tree unreadable"))?.path();
            let child = fs::symlink_metadata(&path).map_err(|_| TeamError::io("staging object unavailable"))?;
            if child.file_type().is_symlink() || child.uid() != unsafe { libc::geteuid() } {
                return Err(TeamError::new("E_PATH_UNSAFE", "unsafe staging object"));
            }
            if child.is_dir() { dirs.push(path); }
            else if child.is_file() && child.nlink() == 1 && child.mode() & 0o077 == 0 { File::open(path).and_then(|f| f.sync_all()).map_err(|_| TeamError::io("file fsync failed"))?; }
            else { return Err(TeamError::new("E_PERMISSION_UNSAFE", "unsafe staging file")); }
        }
    }
    for dir in dirs.into_iter().rev() { sync_dir(&dir).map_err(TeamError::from_message)?; }
    Ok(())
}

fn remove_private_tree(path: &Path, fixed_root: &Path) -> TeamResult<()> {
    if !path.starts_with(fixed_root) || path == fixed_root { return Err(TeamError::new("E_PATH_UNSAFE", "cleanup escaped fixed root")); }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(TeamError::io("cleanup target unavailable")),
    };
    if meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(TeamError::new("E_PATH_UNSAFE", "cleanup target is unsafe"));
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path).map_err(|_| TeamError::io("cleanup target unreadable"))? {
            remove_private_tree(&entry.map_err(|_| TeamError::io("cleanup target unreadable"))?.path(), fixed_root)?;
        }
        fs::remove_dir(path).map_err(|_| TeamError::io("cleanup directory failed"))?;
    } else if meta.is_file() && meta.nlink() == 1 {
        fs::remove_file(path).map_err(|_| TeamError::io("cleanup file failed"))?;
    } else {
        return Err(TeamError::new("E_PATH_UNSAFE", "cleanup object is unsafe"));
    }
    sync_parent(path).map_err(TeamError::from_message)
}

fn validate_stored_team(team: &str, snapshot: &TeamSnapshot, state: &TeamStateFile) -> TeamResult<()> {
    if snapshot.schema_version != 1 || state.schema_version != 1 || snapshot.team != team || !valid_project_label(&snapshot.project) {
        return Err(TeamError::state("team snapshot identity is invalid"));
    }
    if !repository_binding_is_wellformed(&snapshot.project, &snapshot.repo) {
        return Err(TeamError::state("team snapshot repository binding is invalid"));
    }
    validate_team_name(&snapshot.team)?;
    validate_text(&snapshot.mission, MAX_MISSION_SCALARS, MAX_MISSION_BYTES, "mission")?;
    validate_text(&snapshot.acceptance, MAX_MISSION_SCALARS, MAX_MISSION_BYTES, "acceptance")?;
    if snapshot.seats.is_empty() || snapshot.seats.len() > MAX_SEATS || !snapshot.seats.iter().any(|seat| seat.name == snapshot.lead) {
        return Err(TeamError::state("team snapshot has invalid seats or lead"));
    }
    let mut names = HashSet::new();
    for seat in &snapshot.seats {
        if !is_valid_seat_name(&seat.name) || !valid_short_id(&seat.role, 10) || !names.insert(seat.name.clone()) {
            return Err(TeamError::state("team snapshot has invalid seat identity"));
        }
        validate_execution_tuple(&seat.tuple())?;
    }
    for fallback in &snapshot.fallbacks { validate_execution_tuple(fallback)?; }
    if snapshot.fallbacks.len() > MAX_FALLBACKS || !snapshot.grants.is_empty() {
        return Err(TeamError::state("team snapshot has invalid fallback or grant authority"));
    }
    if Uuid::parse_str(&snapshot.creation_request_id).is_err() || Uuid::parse_str(&snapshot.staging_uuid).is_err() {
        return Err(TeamError::state("team snapshot generated ids are invalid"));
    }
    match state.state {
        TeamLifecycle::Pending if state.generation == 0 && state.epic_id.is_none() && state.failure.is_none() => {}
        TeamLifecycle::Active if state.generation == 1 && state.epic_id.as_deref().is_some_and(valid_epic_id) && state.failure.is_none() => {}
        TeamLifecycle::Failed if state.failure.is_some() => {}
        TeamLifecycle::Archived => {}
        _ => return Err(TeamError::state("team lifecycle fields are inconsistent")),
    }
    Ok(())
}

fn validate_request(snapshot: &TeamSnapshot, state: &TeamStateFile, request: &CreationRequestDTO) -> TeamResult<()> {
    let bytes = json_bytes(snapshot)?;
    if request.schema_version != 1
        || request.request_id != snapshot.creation_request_id
        || request.team != snapshot.team
        || request.project != snapshot.project
        || request.repo != snapshot.repo
        || request.snapshot_sha256 != sha256(&bytes)
        || request.expected_generation != 0
        || (state.state == TeamLifecycle::Pending && state.generation != request.expected_generation)
    {
        return Err(TeamError::state("activation request does not bind the current snapshot"));
    }
    Ok(())
}

fn valid_epic_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("aperture-") else { return false; };
    !suffix.is_empty() && suffix.len() <= 32 && suffix.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn validate_epic_id(value: &str) -> TeamResult<()> {
    if valid_epic_id(value) { Ok(()) } else { Err(TeamError::state("authorized epic id is invalid")) }
}

/// Classify a seat for the legacy launcher guard. `Ok(None)` is reserved for
/// genuine standing agents. Any team evidence that cannot be reconciled is an
/// error so legacy start/restart/stop/model-update fails closed.
pub(crate) fn classify_managed_seat(home: &Path, seat: &str) -> TeamResult<Option<ManagedSeatState>> {
    if !is_valid_seat_name(seat) { return Err(TeamError::name("invalid seat id")); }
    let teams_root = home.join(".aperture/teams");
    let journal_root = home.join(".aperture/run/team-journals");
    match fs::symlink_metadata(&journal_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0 => {}
        _ => return Err(TeamError::new("E_PATH_UNSAFE", "team archive journal root is unsafe")),
    }
    let agents_root = home.join(".claude/aperture");
    let marker = agents_root.join(seat).join("TEAM");
    let marker_exists = fs::symlink_metadata(&marker).is_ok();
    let mut found: Option<ManagedSeatState> = None;
    match fs::symlink_metadata(&teams_root) {
        Ok(_) => {
            scan_team_memberships(&teams_root, &journal_root, seat, false, &mut found)?;
            let archive = teams_root.join("archive");
            match fs::symlink_metadata(&archive) {
                Ok(_) => scan_team_memberships(&archive, &journal_root, seat, true, &mut found)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(TeamError::io("team archive registry unreadable")),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(TeamError::io("team registry unreadable")),
    }
    match (&found, marker_exists) {
        (Some(ManagedSeatState::Active { team, generation }), true) => {
            let complete = agents_root.join(seat).join(".complete");
            let team_dir = teams_root.join(team);
            if !real_file_inside(&agents_root.join(seat), &marker)
                || !real_file_inside(&agents_root.join(seat), &complete)
                || fs::symlink_metadata(team_dir.join("journal.json")).is_ok()
            {
                return Err(TeamError::state("active team seat has incomplete or unsafe runtime evidence"));
            }
            Ok(Some(ManagedSeatState::Active { team: team.clone(), generation: *generation }))
        }
        (Some(ManagedSeatState::Active { .. }), false) => Err(TeamError::state("active team seat is missing its runtime marker")),
        (Some(state), false) => Ok(Some(state.clone())),
        (Some(_), true) => Err(TeamError::state("non-active team seat has runtime evidence")),
        (None, true) => Err(TeamError::state("orphaned TEAM marker")),
        (None, false) => Ok(None),
    }
}

fn scan_team_memberships(
    root: &Path,
    journal_root: &Path,
    seat: &str,
    archived_root: bool,
    found: &mut Option<ManagedSeatState>,
) -> TeamResult<()> {
    validate_repo_or_private_dir(root)?;
    for entry in fs::read_dir(root).map_err(|_| TeamError::io("team registry unreadable"))? {
        let entry = entry.map_err(|_| TeamError::io("team registry unreadable"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || (!archived_root && matches!(name.as_str(), "archive" | "presets")) { continue; }
        if !valid_short_id(&name, 16) || !real_dir_inside(root, &entry.path()) {
            return Err(TeamError::new("E_PATH_UNSAFE", "team registry contains an unsafe entry"));
        }
        let snapshot: TeamSnapshot = read_private_json(&entry.path().join("team.json")).map_err(TeamError::from_message)?;
        let state: TeamStateFile = read_private_json(&entry.path().join("state.json")).map_err(TeamError::from_message)?;
        validate_stored_team(&name, &snapshot, &state)?;
        if snapshot.seats.iter().any(|candidate| candidate.name == seat) {
            if fs::symlink_metadata(journal_root.join(format!("{name}.archive.json"))).is_ok() {
                return Err(TeamError::new(
                    "E_JOURNAL_INCONSISTENT",
                    "team archive is incomplete",
                ));
            }
            if found.is_some() { return Err(TeamError::state("seat belongs to multiple team snapshots")); }
            *found = Some(if archived_root || state.state == TeamLifecycle::Archived {
                ManagedSeatState::Archived
            } else {
                match state.state {
                    TeamLifecycle::Active => ManagedSeatState::Active { team: snapshot.team, generation: state.generation },
                    TeamLifecycle::Pending => ManagedSeatState::Pending,
                    TeamLifecycle::Failed => ManagedSeatState::Failed,
                    TeamLifecycle::Archived => ManagedSeatState::Archived,
                }
            });
        }
    }
    Ok(())
}

fn validate_repo_or_private_dir(path: &Path) -> TeamResult<()> {
    let meta = fs::symlink_metadata(path).map_err(|_| TeamError::io("registry unavailable"))?;
    if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(TeamError::new("E_PATH_UNSAFE", "registry root is unsafe"));
    }
    Ok(())
}

fn engine_from_state(state: &tauri::State<'_, Arc<Mutex<AppState>>>) -> TeamResult<TeamEngine> {
    let state = state.lock().map_err(|_| TeamError::io("application state unavailable"))?;
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"));
    Ok(TeamEngine::new(home, PathBuf::from(&state.project_dir)))
}

#[tauri::command]
pub fn team_get_catalog(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> TeamResult<TeamCatalog> {
    engine_from_state(&state)?.catalog()
}

#[tauri::command]
pub fn team_list_presets(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> TeamResult<Vec<TeamPreset>> {
    engine_from_state(&state)?.list_presets()
}

#[tauri::command]
pub fn team_save_preset(input: SavePresetInput, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> TeamResult<TeamPreset> {
    engine_from_state(&state)?.save_preset(&AuthenticatedActor::operator_ui(), input)
}

#[tauri::command]
pub fn team_create(input: CreateTeamInput, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> TeamResult<CreateTeamResult> {
    engine_from_state(&state)?.create_team(&AuthenticatedActor::operator_ui(), input)
}

#[tauri::command]
pub fn team_list(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> TeamResult<Vec<TeamView>> {
    engine_from_state(&state)?.list_teams()
}

#[tauri::command]
pub fn team_cancel_pending(input: CancelPendingInput, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> TeamResult<CancelPendingResult> {
    engine_from_state(&state)?.cancel_pending(&AuthenticatedActor::operator_ui(), input)
}

fn replacement_error(error: ReplacementError) -> TeamError {
    let message = match error {
        ReplacementError::GenerationMismatch => "runtime generation changed",
        ReplacementError::InvalidSnapshot | ReplacementError::StopUnverified => "owned process stop could not be verified",
        ReplacementError::UnownedProcess => "an unowned process blocks this operation",
        ReplacementError::RevocationUnverified => "message authority revocation could not be verified",
        ReplacementError::RemoteUncertain => "remote effects remain uncertain",
        ReplacementError::AuthorizationRequired => "this replacement is not authorized",
        ReplacementError::FreshThreadUnverified => "a fresh thread could not be verified",
        ReplacementError::ModelUnverified => "the requested runtime tuple was not verified",
        ReplacementError::StartCleanupUnverified => "failed-start cleanup could not be verified",
        ReplacementError::NativeFailure => "native runtime operation failed",
        ReplacementError::RepoBindingUnavailable => "repository binding is unavailable",
        ReplacementError::CheckpointUnavailable => "checkpoint validation is unavailable",
        ReplacementError::LaunchUnavailable => "native launch is unavailable",
        ReplacementError::Deadline => "runtime deadline elapsed",
        ReplacementError::OutcomeUnknown => "native control outcome is unknown",
        ReplacementError::PreparationExpired => "replacement preparation expired",
        ReplacementError::WorktreeUnbound => "replacement worktree is not authoritatively bound",
    };
    TeamError::new(error.code(), message)
}

fn selection_from_tuple(tuple: &ExecutionTuple) -> TeamResult<StartSelection> {
    let harness = serde_json::to_value(&tuple.harness)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| TeamError::state("execution harness is invalid"))?;
    let reasoning = tuple
        .reasoning
        .as_ref()
        .map(|value| {
            serde_json::to_value(value)
                .ok()
                .and_then(|encoded| encoded.as_str().map(str::to_owned))
                .ok_or_else(|| TeamError::state("execution reasoning is invalid"))
        })
        .transpose()?;
    Ok(StartSelection { harness, model: tuple.model.clone(), reasoning })
}

fn verified_replacement_checks() -> ReplacementChecks {
    ReplacementChecks {
        process_stop: RuntimeCheckState::Verified,
        revocation: RuntimeCheckState::Verified,
        remote_effects: RuntimeCheckState::Verified,
    }
}

fn owner_summary(home: &Path, seat: &str) -> TeamResult<OwnerSummary> {
    OwnerStore::new(home.join(".aperture/run/owner"))
        .summary(seat)
        .map_err(TeamError::from_message)
}

fn archive_check(blockers: &[RuntimeBlocker], codes: &[&str]) -> RuntimeCheckState {
    if blockers.iter().any(|b| codes.contains(&b.code.as_str())) { RuntimeCheckState::Blocked } else { RuntimeCheckState::Verified }
}

struct ArchiveInspection {
    view: ArchiveView,
    approval: crate::journal::ArchiveJournalApproval,
    snapshot: TeamSnapshot,
    state: TeamStateFile,
}

// Diagnostic retirement proves managed teardown, not mission/epic success.
fn diagnostic_archive_checks() -> ArchiveChecks {
    ArchiveChecks {
        reconciliation: RuntimeCheckState::Unknown, reviews: RuntimeCheckState::Unknown,
        metrics: RuntimeCheckState::Unknown, process_stop: RuntimeCheckState::Verified,
        revocation: RuntimeCheckState::Verified, remote_effects: RuntimeCheckState::Unknown,
        worktrees: RuntimeCheckState::Unknown,
    }
}

fn collect_archive(engine: &TeamEngine, input: &ArchiveTeamInput) -> TeamResult<ArchiveInspection> {
    validate_team_name(&input.team)?;
    if input.expected_generation == 0 { return Err(TeamError::new("E_GENERATION_MISMATCH", "archive generation is invalid")); }
    let view = engine.read_team_view(&input.team)?;
    if view.state.state != TeamLifecycle::Active || view.state.generation != input.expected_generation {
        return Err(TeamError::new("E_GENERATION_MISMATCH", "archive team generation changed"));
    }
    // A Quarantined owner selects a verification path, never grants authority.
    // Mixed/normal teams fail there; no caller can select diagnostic retirement.
    let owners = OwnerStore::new(engine.paths.owners.clone());
    let quarantined = view.snapshot.seats.iter().map(|s| owners.read_owner(&s.name))
        .collect::<Result<Vec<_>, _>>().map_err(TeamError::from_message)?
        .iter().any(|o| o.state == OwnerState::Quarantined);
    if quarantined {
        let approval = crate::team_archive::diagnostic::inspect(&engine.paths.home, &view.snapshot, &view.state)
            .map_err(TeamError::from_message)?;
        return Ok(ArchiveInspection {
            view: ArchiveView {team: input.team.clone(), generation: input.expected_generation,
                state: "pending".into(), checks: diagnostic_archive_checks(), blockers: vec![]},
            approval, snapshot: view.snapshot, state: view.state,
        });
    }
    if crate::team_archive::retirement::has_facts(&engine.paths.home,&view.snapshot).map_err(TeamError::from_message)? {
        let approval=crate::team_archive::retirement::inspect(&engine.paths.home,&view.snapshot,&view.state)
            .map_err(TeamError::from_message)?;
        return Ok(ArchiveInspection {view:ArchiveView {team:input.team.clone(),generation:input.expected_generation,
            state:"pending".into(),checks:diagnostic_archive_checks(),blockers:vec![]},approval,snapshot:view.snapshot,state:view.state});
    }
    let epic = view.state.epic_id.as_deref().ok_or_else(|| TeamError::new("E_RECONCILIATION_INCOMPLETE", "active team epic is unavailable"))?;
    let seats: Vec<_> = view.snapshot.seats.iter().map(|s| s.name.clone()).collect();
    let beads = crate::team_archive::beads::collect_native(&engine.paths.home, &input.team, input.expected_generation, epic, &[])
        .map_err(|e| TeamError::new(e.code(), e.message()))?;
    let native = crate::team_archive::native::inspect_native(&engine.paths.home, &input.team, input.expected_generation, &[])
        .map_err(|e| TeamError::new(&e.code, "archive runtime inventory is unavailable"))?;
    let mut blockers: Vec<RuntimeBlocker> = beads.structural_blockers(&seats).into_iter()
        .map(|b| RuntimeBlocker { code:b.code, reference:b.reference }).collect();
    let owner_sha256 = native.seats.iter().map(|seat| (seat.seat.clone(), seat.owner_sha256.clone())).collect();
    let owner_store = OwnerStore::new(engine.paths.owners.clone());
    let owner_states = view.snapshot.seats.iter().map(|seat| {
        let owner = owner_store.read_owner(&seat.name).map_err(TeamError::from_message)?;
        let state = match owner.state {
            OwnerState::Active => "active",
            OwnerState::Stale => "stale",
            _ => return Err(TeamError::new("E_ARCHIVE_OWNER_INVALID", "archive owner is not terminal")),
        };
        Ok((seat.name.clone(), state.to_string()))
    }).collect::<TeamResult<Vec<_>>>()?;
    for seat in native.seats {
        blockers.extend(seat.blockers.into_iter().map(|b| RuntimeBlocker { code:b.code, reference:b.reference }));
    }
    blockers.sort_by(|a,b| (&a.code,&a.reference).cmp(&(&b.code,&b.reference)));
    blockers.dedup_by(|a,b| a.code == b.code && a.reference == b.reference);
    let checks = ArchiveChecks {
        reconciliation: archive_check(&blockers, &["E_RECONCILIATION_INCOMPLETE","E_RECONCILIATION_COVERAGE","E_CREATION_GATE_VIOLATION","E_UNFINISHED_SEAT_WORK","E_COMPLETED_WITHOUT_EVIDENCE","E_CANCEL_UNAPPROVED","E_TRANSFER_UNACCEPTED","E_OPEN_CHILDREN"]),
        reviews: archive_check(&blockers, &["E_REVIEW_MISSING"]),
        metrics: archive_check(&blockers, &["E_METRIC_UNMET"]),
        process_stop: archive_check(&blockers, &["E_STOP_UNVERIFIED","E_ARCHIVE_OWNER_INVALID","E_ARCHIVE_OWNER_UNSUPPORTED","E_ARCHIVE_NEVER_STARTED_UNVERIFIED"]),
        revocation: archive_check(&blockers, &["E_REVOCATION_UNVERIFIED"]),
        remote_effects: archive_check(&blockers, &["E_REMOTE_UNCERTAIN"]),
        worktrees: archive_check(&blockers, &["E_WORKTREE_UNPROTECTED","E_REPO_BINDING_UNAVAILABLE"]),
    };
    let public = ArchiveView { team:input.team.clone(), generation:input.expected_generation,
        state:if blockers.is_empty(){"pending".into()}else{"blocked".into()}, checks, blockers };
    Ok(ArchiveInspection {
        view: public,
        approval: crate::journal::ArchiveJournalApproval {
            category: crate::journal::ArchiveCategory::Mission,
            generation: input.expected_generation,
            epic_id: epic.into(),
            record_sha256: beads.record_sha256,
            inventory_sha256: beads.inventory_sha256,
            native_sha256: native.sha256,
            owner_sha256,
            owner_states,
            owner_post_sha256: vec![],
            transition_at: String::new(),
            approved_by: "glados".into(),
        },
        snapshot: view.snapshot,
        state: view.state,
    })
}

fn inspect_archive(engine: &TeamEngine, input: &ArchiveTeamInput) -> TeamResult<ArchiveView> {
    collect_archive(engine, input).map(|result| result.view)
}

fn permit_store(
    state: &tauri::State<'_, Arc<Mutex<AppState>>>,
) -> TeamResult<Arc<Mutex<crate::state::RuntimePermitStore>>> {
    state
        .lock()
        .map(|state| Arc::clone(&state.team_preparations))
        .map_err(|_| TeamError::io("application state unavailable"))
}

#[tauri::command]
pub fn team_bootstrap_seat(
    input: BootstrapSeatInput,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> TeamResult<BootstrapView> {
    bootstrap_seat(&engine_from_state(&state)?, &AuthenticatedActor::operator_ui(), input)
}

fn claude_inbox_probe(engine: &TeamEngine, actor: &AuthenticatedActor, input: ClaudeStartupSmokeInput) -> TeamResult<ClaudeInboxProbeView> {
    // Auth precedes even selector validation. The input cannot choose a
    // prompt, mode, actor, model or a retained/old generation.
    if !actor.is_glados() { return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required")); }
    actor.revalidate_before_mutation().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability changed"))?;
    validate_team_name(&input.team)?;
    if !is_valid_seat_name(&input.seat) || input.expected_generation != 0 {
        return Err(TeamError::new("E_GENERATION_MISMATCH", "fresh inbox diagnostic seat required"));
    }
    if managed_launch_enabled(&Harness::Claude) {
        return Err(TeamError::state("inbox probe is not an ordinary launch"));
    }
    let view = engine.read_team_view(&input.team)?;
    if view.state.state != TeamLifecycle::Active
        || view.snapshot.seats.iter().filter(|s| s.name == input.seat && s.harness == Harness::Claude
            && s.model == crate::team_claude_launch::MODEL && s.reasoning.is_none()).count() != 1 {
        return Err(TeamError::state("approved exact Sonnet diagnostic seat required"));
    }
    let probe = crate::team_replacement::native::bootstrap_claude_inbox_probe_authorized(
        &engine.paths.home, actor, &input.team, &input.seat, input.expected_generation).map_err(replacement_error)?;
    project_claude_inbox_probe(&input, probe)
}

fn project_claude_inbox_probe(input: &ClaudeStartupSmokeInput,
    probe: crate::team_replacement::native::ClaudeInboxProbe) -> TeamResult<ClaudeInboxProbeView> {
    if probe.team != input.team || probe.seat != input.seat || input.expected_generation != 0
        || probe.generation != 1 || probe.actual.harness != Harness::Claude
        || probe.actual.model != crate::team_claude_launch::MODEL || probe.actual.reasoning.is_some() {
        return Err(TeamError::new("E_CONTROL_UNKNOWN", "inbox probe readback mismatch; inspect without retry"));
    }
    Ok(ClaudeInboxProbeView {team:probe.team, seat:probe.seat, generation:probe.generation,
        model:probe.actual.model, reasoning_observation:"not_observed", startup:"verified",
        kickoff:"sent", owner_state:OwnerState::Quarantined, cleanup:"verified", mcp_readiness:"pending_verification", public_enabled:false})
}

fn claude_startup_smoke(engine: &TeamEngine, actor: &AuthenticatedActor, input: ClaudeStartupSmokeInput) -> TeamResult<ClaudeStartupSmokeView> {
    if !actor.is_glados() {
        return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required"));
    }
    actor.revalidate_before_mutation().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability changed"))?;
    validate_team_name(&input.team)?;
    if !is_valid_seat_name(&input.seat) || input.expected_generation != 0 {
        return Err(TeamError::new("E_GENERATION_MISMATCH", "fresh diagnostic seat required"));
    }
    // This diagnostic never weakens the public launch policy or becomes a
    // second bootstrap entrypoint for ordinary mission workers.
    if managed_launch_enabled(&Harness::Claude) {
        return Err(TeamError::state("startup-only diagnostic is not an ordinary launch"));
    }
    let view = engine.read_team_view(&input.team)?;
    if view.state.state != TeamLifecycle::Active
        || view.snapshot.seats.iter().filter(|s| s.name == input.seat
            && s.harness == Harness::Claude && s.model == crate::team_claude_launch::MODEL
            && s.reasoning.is_none()).count() != 1
    { return Err(TeamError::state("approved exact Sonnet diagnostic seat required")); }
    let diagnostic = bootstrap_claude_smoke_authorized(&engine.paths.home, actor,
        &input.team, &input.seat, input.expected_generation, &[]).map_err(replacement_error)?;
    project_claude_startup_smoke(&input, diagnostic)
}

fn project_claude_startup_smoke(
    input: &ClaudeStartupSmokeInput,
    diagnostic: crate::team_replacement::native::ClaudeSmokeDiagnostic,
) -> TeamResult<ClaudeStartupSmokeView> {
    if diagnostic.team != input.team || diagnostic.seat != input.seat
        || input.expected_generation != 0 || diagnostic.generation != 1
        || diagnostic.actual.harness != Harness::Claude
        || diagnostic.actual.model != crate::team_claude_launch::MODEL
        || diagnostic.actual.reasoning.is_some()
    { return Err(TeamError::new("E_CONTROL_UNKNOWN", "diagnostic readback mismatch; inspect before any further action")); }
    Ok(ClaudeStartupSmokeView {
        team: diagnostic.team, seat: diagnostic.seat, generation: diagnostic.generation,
        model: diagnostic.actual.model, reasoning_observation: "not_observed",
        startup: "verified", cleanup: "quarantined", mcp_readiness: "not_run", public_enabled: false,
    })
}

fn validate_checkpoint(engine: &TeamEngine, actor: &AuthenticatedActor, input: ValidateCheckpointInput) -> TeamResult<ValidateCheckpointView> {
    if !actor.is_glados() {
        return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required"));
    }
    actor.revalidate_before_mutation().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability changed"))?;
    validate_team_name(&input.team)?;
    if !is_valid_seat_name(&input.seat) || input.expected_generation == 0 || input.seq == 0 {
        return Err(TeamError::new("E_GENERATION_MISMATCH", "checkpoint selectors are invalid"));
    }
    let view = engine.read_team_view(&input.team)?;
    if view.state.state != TeamLifecycle::Active
        || view.snapshot.seats.iter().filter(|s| s.name == input.seat).count() != 1
    { return Err(TeamError::state("active team seat required")); }
    let result = crate::team_replacement::native::validate_checkpoint_glados(
        &engine.paths.home, actor,
        &RemoteTarget { team: input.team.clone(), seat: input.seat.clone(), expected_generation: input.expected_generation },
        input.seq, &[]).map_err(replacement_error)?;
    use crate::team_checkpoint::CheckpointValidation;
    let validation = match result {
        CheckpointValidation::Ok => "ok",
        CheckpointValidation::Divergent { .. } => "divergent",
        CheckpointValidation::Rejected { .. } => "rejected",
        CheckpointValidation::Pending => return Err(TeamError::new("E_CONTROL_UNKNOWN", "validation was not confirmed; inspect before retry")),
    };
    Ok(ValidateCheckpointView { team: input.team, seat: input.seat,
        generation: input.expected_generation, seq: input.seq, validation })
}

fn retire_seat(engine:&TeamEngine,actor:&AuthenticatedActor,input:RetireSeatInput)->TeamResult<RetireSeatView> {
    retire_seat_checked(engine, actor, input, false)
}

fn retire_seat_checked(engine:&TeamEngine,actor:&AuthenticatedActor,input:RetireSeatInput,reconcile:bool)->TeamResult<RetireSeatView> {
    if !actor.is_glados() {return Err(TeamError::new("E_CONTROL_UNAUTHORIZED","GLaDOS capability required"));}
    actor.revalidate_before_mutation().map_err(TeamError::from_message)?;
    validate_team_name(&input.team)?;
    if !is_valid_seat_name(&input.seat) || input.expected_generation==0 {return Err(TeamError::new("E_GENERATION_MISMATCH","invalid retirement selector"));}
    let view=engine.read_team_view(&input.team)?;
    if view.state.state!=TeamLifecycle::Active || view.snapshot.seats.iter().filter(|s|s.name==input.seat).count()!=1 {return Err(TeamError::state("active team seat required"));}
    let operation = if reconcile { crate::team_replacement::native::reconcile_stopped_retirement }
        else { crate::team_replacement::native::stop_for_retirement };
    let recovery=operation(&engine.paths.home,actor,&input.team,&input.seat,
        input.expected_generation,input.accept_checkpoint_loss).map_err(replacement_error)?;
    let owner=owner_summary(&engine.paths.home,&input.seat)?;
    if owner.generation!=input.expected_generation || owner.state!=OwnerState::Active {return Err(TeamError::new("E_CONTROL_UNKNOWN","retirement readback changed"));}
    Ok(RetireSeatView {team:input.team,seat:input.seat,generation:owner.generation,owner_state:owner.state,
        process_stop:RuntimeCheckState::Verified,revocation:RuntimeCheckState::Verified,mission:RuntimeCheckState::Unknown,checkpoint_recovery:recovery})
}

fn stop_seat(engine: &TeamEngine, actor: &AuthenticatedActor, input: StopSeatInput) -> TeamResult<StopSeatView> {
    if !actor.is_glados() {
        return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required"));
    }
    actor.revalidate_before_mutation().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability changed"))?;
    validate_team_name(&input.team)?;
    if !is_valid_seat_name(&input.seat) || input.expected_generation == 0 {
        return Err(TeamError::new("E_GENERATION_MISMATCH", "stop selectors are invalid"));
    }
    let view = engine.read_team_view(&input.team)?;
    if view.state.state != TeamLifecycle::Active
        || view.snapshot.seats.iter().filter(|s| s.name == input.seat).count() != 1
    { return Err(TeamError::state("active team seat required")); }
    let recovery = stop_for_archive(&engine.paths.home, actor, &input.team, &input.seat,
        input.expected_generation, &[]).map_err(replacement_error)?;
    let owner = owner_summary(&engine.paths.home, &input.seat)?;
    if recovery != CheckpointRecovery::Valid || owner.generation != input.expected_generation
        || owner.state != OwnerState::Active
    { return Err(TeamError::new("E_CONTROL_UNKNOWN", "stop readback is inconsistent; inspect before retry")); }
    Ok(StopSeatView { team: input.team, seat: input.seat, generation: owner.generation,
        phase: ReplacementPhase::Ready, checkpoint_recovery: recovery,
        owner_state: owner.state, blockers: vec![] })
}

fn bootstrap_seat(engine: &TeamEngine, actor: &AuthenticatedActor, input: BootstrapSeatInput) -> TeamResult<BootstrapView> {
    validate_team_name(&input.team)?;
    // 0 = first start; 1 | 2 = explicit recovery of a Claude bootstrap that
    // ended Quarantined unobserved at that generation (native proof decides,
    // never this selector). 3 and above are never selectors.
    if !is_valid_seat_name(&input.seat) || input.expected_generation > 2 {
        return Err(TeamError::new("E_GENERATION_MISMATCH", "bootstrap selectors are invalid"));
    }
    let started = bootstrap_authorized(
        &engine.paths.home,
        actor,
        &input.team,
        &input.seat,
        input.expected_generation,
    )
    .map_err(replacement_error)?;
    let owner = owner_summary(&engine.paths.home, &input.seat)?;
    if owner.generation != started.generation || owner.state != crate::state::OwnerState::Active {
        return Err(TeamError::new("E_CONTROL_UNKNOWN", "bootstrap owner readback is inconsistent"));
    }
    Ok(BootstrapView {
        team: input.team,
        seat: input.seat,
        generation: started.generation,
        phase: BootstrapPhase::Started,
        owner: Some(owner),
        blockers: vec![],
    })
}

#[tauri::command]
pub fn team_prepare_replacement(
    input: PrepareReplacementInput,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> TeamResult<PreparedReplacementView> {
    validate_team_name(&input.team)?;
    if !is_valid_seat_name(&input.seat) || input.expected_generation == 0 {
        return Err(TeamError::new("E_GENERATION_MISMATCH", "replacement selectors are invalid"));
    }
    let engine = engine_from_state(&state)?;
    let permits = permit_store(&state)?;
    {
        let mut permits = permits.lock().map_err(|_| TeamError::io("replacement permit store unavailable"))?;
        permits
            .reserve(&input.team, &input.seat, input.expected_generation)
            .map_err(TeamError::from_message)?;
    }
    let prepared = prepare_operator(
        &engine.paths.home,
        &AuthenticatedActor::operator_ui(),
        &input.team,
        &input.seat,
        input.expected_generation,
        &[],
    );
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Ok(mut permits) = permits.lock() {
                permits.cancel_reservation(&input.team, &input.seat, input.expected_generation);
            }
            return Err(replacement_error(error));
        }
    };
    let recovery = prepared.recovery();
    let preparation_id = Uuid::new_v4().to_string();
    {
        let mut permits = permits.lock().map_err(|_| TeamError::io("replacement permit store unavailable"))?;
        permits.publish(preparation_id.clone(), prepared).map_err(TeamError::from_message)?;
    }
    let owner = owner_summary(&engine.paths.home, &input.seat)?;
    if owner.generation != input.expected_generation {
        return Err(TeamError::new("E_CONTROL_UNKNOWN", "prepared owner readback is inconsistent"));
    }
    Ok(PreparedReplacementView {
        replacement: ReplacementView {
            team: input.team,
            seat: input.seat,
            generation: input.expected_generation,
            phase: ReplacementPhase::Ready,
            checkpoint_recovery: recovery,
            checks: verified_replacement_checks(),
            owner: Some(owner),
            blockers: vec![],
        },
        preparation_id: Some(preparation_id),
    })
}

#[tauri::command]
pub fn team_start_replacement(
    input: StartReplacementInput,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> TeamResult<ReplacementView> {
    validate_team_name(&input.team)?;
    if !is_valid_seat_name(&input.seat)
        || input.expected_generation == 0
        || Uuid::parse_str(&input.preparation_id).is_err()
    {
        return Err(TeamError::new("E_PREPARATION_EXPIRED", "replacement preparation selectors are invalid"));
    }
    let engine = engine_from_state(&state)?;
    let permits = permit_store(&state)?;
    let permit = permits
        .lock()
        .map_err(|_| TeamError::io("replacement permit store unavailable"))?
        .take(
            &input.preparation_id,
            &input.team,
            &input.seat,
            input.expected_generation,
        )
        .map_err(TeamError::from_message)?;
    let recovery = permit.recovery();
    let selection = selection_from_tuple(&input.selection)?;
    let started = start_operator(&AuthenticatedActor::operator_ui(), permit, &selection)
        .map_err(replacement_error)?;
    let owner = owner_summary(&engine.paths.home, &input.seat)?;
    if owner.generation != started.generation || owner.state != crate::state::OwnerState::Active {
        return Err(TeamError::new("E_CONTROL_UNKNOWN", "replacement owner readback is inconsistent"));
    }
    Ok(ReplacementView {
        team: input.team,
        seat: input.seat,
        generation: started.generation,
        phase: ReplacementPhase::Started,
        checkpoint_recovery: recovery,
        checks: verified_replacement_checks(),
        owner: Some(owner),
        blockers: vec![],
    })
}

/// Read-only archive checklist. Mutation remains disabled until the
/// authenticated GLaDOS finalizer and rollback journal are composed.
#[tauri::command]
pub fn team_archive(input: ArchiveTeamInput, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> TeamResult<ArchiveView> {
    inspect_archive(&engine_from_state(&state)?, &input)
}

/// Common headless control entrypoint. The caller supplies selectors only;
/// actor/env fields are not part of the tagged schema and cannot construct
/// `AuthenticatedActor`.
pub fn team_control_headless(input_json: &str) -> TeamResult<TeamControlResponse> {
    let request: TeamControlRequest = serde_json::from_str(input_json).map_err(|_| TeamError::state("invalid team control request"))?;
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"));
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or_else(|| TeamError::io("project root unavailable"))?.to_path_buf();
    let engine = TeamEngine::new(home, project);
    match request {
        TeamControlRequest::Catalog => {
            authenticate_glados_control().map_err(TeamError::from_message)?;
            engine.catalog().map(TeamControlResponse::Catalog)
        }
        TeamControlRequest::ListRepositories => {
            authenticate_glados_control().map_err(TeamError::from_message)?;
            engine.list_repositories().map(TeamControlResponse::ListRepositories)
        }
        TeamControlRequest::SaveRepository(input) => {
            let actor = authenticate_glados_control().map_err(TeamError::from_message)?;
            engine.save_repository(&actor, input).map(TeamControlResponse::SaveRepository)
        }
        TeamControlRequest::Create(input) => {
            let actor = authenticate_glados_control().map_err(TeamError::from_message)?;
            engine.create_team(&actor, input).map(TeamControlResponse::Create)
        }
        TeamControlRequest::ClaudeInboxProbe(input) => {
            let actor = authenticate_glados_control().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required"))?;
            claude_inbox_probe(&engine, &actor, input).map(TeamControlResponse::ClaudeInboxProbe)
        }
        TeamControlRequest::ClaudeStartupSmoke(input) => {
            let actor = authenticate_glados_control().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required"))?;
            claude_startup_smoke(&engine, &actor, input).map(TeamControlResponse::ClaudeStartupSmoke)
        }
        TeamControlRequest::ReconcileClaudeStartup(input) => {
            let actor = authenticate_glados_control().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required"))?;
            crate::team_replacement::native::reconcile_stopped_claude_smoke(
                &engine.paths.home, &actor, &input.team, &input.seat, input.expected_generation,
            ).map_err(replacement_error)?;
            Ok(TeamControlResponse::ReconcileClaudeStartup(ReconciledClaudeStartupView {
                team:input.team, seat:input.seat, generation:input.expected_generation,
                owner_state:OwnerState::Quarantined, startup:"not_verified", cleanup:"stopped_reconciled", public_enabled:false,
            }))
        }
        TeamControlRequest::ValidateCheckpoint(input) => {
            let actor = authenticate_glados_control().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required"))?;
            validate_checkpoint(&engine, &actor, input).map(TeamControlResponse::ValidateCheckpoint)
        }
        TeamControlRequest::RetireSeat(input) => {
            let actor=authenticate_glados_control().map_err(TeamError::from_message)?;
            retire_seat(&engine,&actor,input).map(TeamControlResponse::RetireSeat)
        }
        TeamControlRequest::ReconcileStoppedRetirement(input) => {
            let actor=authenticate_glados_control().map_err(TeamError::from_message)?;
            retire_seat_checked(&engine,&actor,input,true).map(TeamControlResponse::ReconcileStoppedRetirement)
        }
        TeamControlRequest::StopSeat(input) => {
            let actor = authenticate_glados_control().map_err(|_| TeamError::new("E_CONTROL_UNAUTHORIZED", "GLaDOS capability required"))?;
            stop_seat(&engine, &actor, input).map(TeamControlResponse::StopSeat)
        }
        TeamControlRequest::BootstrapSeat(input) => {
            let actor = authenticate_glados_control().map_err(TeamError::from_message)?;
            bootstrap_seat(&engine, &actor, input).map(TeamControlResponse::BootstrapSeat)
        }
        TeamControlRequest::ListTeams => {
            authenticate_glados_control().map_err(TeamError::from_message)?;
            engine.list_teams().map(TeamControlResponse::ListTeams)
        }
        TeamControlRequest::ListPending => {
            authenticate_glados_control().map_err(TeamError::from_message)?;
            engine.list_teams().map(|teams| TeamControlResponse::ListPending(teams.into_iter().filter(|team| team.state.state == TeamLifecycle::Pending).collect()))
        }
        TeamControlRequest::Approve(input) => {
            let actor = authenticate_glados_control().map_err(TeamError::from_message)?;
            engine.activate(&actor, input).map(TeamControlResponse::Approve)
        }
        TeamControlRequest::Cancel(input) => {
            let actor = authenticate_glados_control().map_err(TeamError::from_message)?;
            engine.cancel_pending(&actor, input).map(TeamControlResponse::Cancel)
        }
        TeamControlRequest::Checkpoint(input) => {
            let actor = authenticate_seat_control().map_err(TeamError::from_message)?;
            let view = engine.read_team_view(actor.team())?;
            if view.state.state != TeamLifecycle::Active || !view.snapshot.seats.iter().any(|seat| seat.name == actor.seat()) {
                return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "managed seat is not active in its team"));
            }
            let configured = view.snapshot.seats.iter().find(|seat| seat.name == actor.seat())
                .ok_or_else(|| TeamError::new("E_CONTROL_UNAUTHORIZED", "managed seat is not in its team"))?;
            let harness = serde_json::to_value(&configured.harness).map_err(|_| TeamError::io("checkpoint context unavailable"))?
                .as_str().ok_or_else(|| TeamError::io("checkpoint context unavailable"))?.to_string();
            let context = CheckpointContext {
                team: actor.team().to_string(), seat: actor.seat().to_string(), generation: actor.generation(),
                authenticated_generation: actor.generation(), harness, writer: CheckpointWriter::Explicit,
            };
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| TeamError::io("checkpoint clock unavailable"))?.as_millis().try_into()
                .map_err(|_| TeamError::io("checkpoint clock unavailable"))?;
            let entry = crate::team_checkpoint::native::write_native(
                &engine.paths.home, &context, input.schema_version, input.payload, now, &[],
                || actor.revalidate_before_effect().map_err(|_| CheckpointError::Generation),
            ).map_err(checkpoint_error)?;
            Ok(TeamControlResponse::Checkpoint(CheckpointReceipt { checkpoint_id: entry.checkpoint_id, status: "pending".into() }))
        }
        TeamControlRequest::InspectRemote(input) => {
            let actor = authenticate_seat_control().map_err(TeamError::from_message)?;
            let target = RemoteTarget {
                team: actor.team().to_string(),
                seat: input.target_seat,
                expected_generation: input.expected_generation,
            };
            inspect_authorized(
                &engine.paths.home,
                ResolutionAuthority::Lead(&actor),
                &target,
                &[],
            )
            .map(TeamControlResponse::InspectRemote)
            .map_err(remote_error)
        }
        TeamControlRequest::ResolveRemote(input) => {
            let actor = authenticate_seat_control().map_err(TeamError::from_message)?;
            let target = RemoteTarget {
                team: actor.team().to_string(),
                seat: input.target_seat,
                expected_generation: input.expected_generation,
            };
            resolve_native(
                &engine.paths.home,
                ResolutionAuthority::Lead(&actor),
                &target,
                &input.resolution,
                &[],
            )
            .map(TeamControlResponse::ResolveRemote)
            .map_err(remote_error)
        }
        TeamControlRequest::Replace(input) => {
            let actor = authenticate_seat_control().map_err(TeamError::from_message)?;
            let target_seat = input.target_seat;
            let target = RemoteTarget {
                team: actor.team().to_string(),
                seat: target_seat.clone(),
                expected_generation: input.expected_generation,
            };
            let selection = selection_from_tuple(&input.selection)?;
            let result = replace_authorized(
                &engine.paths.home,
                ReplacementAuthority::Lead(&actor),
                target,
                &selection,
                &[],
            )
            .map_err(replacement_error)?;
            let owner = owner_summary(&engine.paths.home, &target_seat)?;
            if owner.generation != result.started.generation
                || owner.state != crate::state::OwnerState::Active
            {
                return Err(TeamError::new(
                    "E_CONTROL_UNKNOWN",
                    "replacement owner readback is inconsistent",
                ));
            }
            Ok(TeamControlResponse::Replace(ReplacementView {
                team: actor.team().to_string(),
                seat: target_seat,
                generation: result.started.generation,
                phase: ReplacementPhase::Started,
                checkpoint_recovery: result.checkpoint_recovery,
                checks: verified_replacement_checks(),
                owner: Some(owner),
                blockers: vec![],
            }))
        }
        TeamControlRequest::Archive(input) => {
            let actor = authenticate_glados_control().map_err(TeamError::from_message)?;
            let category;
            if crate::team_archive_finalize::has_journal(&engine.paths.home, &input.team) {
                category = crate::team_archive_finalize::recover_approval(&engine.paths.home, &input.team, input.expected_generation)
                    .map_err(TeamError::from_message)?.category;
                crate::team_archive_finalize::finalize(
                    &engine.paths.home, &actor, &input.team, input.expected_generation, None,
                ).map_err(TeamError::from_message)?;
            } else {
                let inspection = collect_archive(&engine, &input)?;
                category = inspection.approval.category;
                if !inspection.view.blockers.is_empty() {
                    return Ok(TeamControlResponse::Archive(inspection.view));
                }
                crate::team_archive_finalize::finalize(
                    &engine.paths.home, &actor, &input.team, input.expected_generation,
                    Some(crate::team_archive_finalize::FreshArchive {
                        snapshot: &inspection.snapshot,
                        state: &inspection.state,
                        approval: &inspection.approval,
                    }),
                ).map_err(TeamError::from_message)?;
            }
            Ok(TeamControlResponse::Archive(ArchiveView {
                team: input.team, generation: input.expected_generation, state: "archived".into(),
                checks: if category != crate::journal::ArchiveCategory::Mission {
                    diagnostic_archive_checks()
                } else { ArchiveChecks {
                    reconciliation: RuntimeCheckState::Verified, reviews: RuntimeCheckState::Verified,
                    metrics: RuntimeCheckState::Verified, process_stop: RuntimeCheckState::Verified,
                    revocation: RuntimeCheckState::Verified, remote_effects: RuntimeCheckState::Verified,
                    worktrees: RuntimeCheckState::Verified,
                } }, blockers: vec![],
            }))
        }
        TeamControlRequest::RollbackArchive(input) => {
            let actor = authenticate_glados_control().map_err(TeamError::from_message)?;
            crate::team_archive_finalize::rollback(
                &engine.paths.home,
                &actor,
                &input.team,
                input.expected_generation,
            )
            .map_err(TeamError::from_message)?;
            engine
                .read_team_view(&input.team)
                .map(TeamControlResponse::RollbackArchive)
        }
    }
}

fn remote_error(error: RemoteError) -> TeamError {
    let code = error.code();
    let message = match code {
        "E_CONTROL_UNAUTHORIZED" => "remote-effect authority is unavailable",
        "E_GENERATION_MISMATCH" => "remote-effect target generation changed",
        "E_REMOTE_RESOLUTION_INVALID" => "remote-effect resolution is invalid",
        "E_REMOTE_RESOLUTION_CONFLICT" => "remote-effect resolution conflicts with durable history",
        _ => "remote-effect inventory is uncertain",
    };
    TeamError::new(code, message)
}

fn checkpoint_error(error: CheckpointError) -> TeamError {
    match error {
        CheckpointError::Invalid | CheckpointError::Unsafe | CheckpointError::HookHarness => TeamError::new("E_CHECKPOINT_INVALID", "checkpoint payload is invalid"),
        CheckpointError::Generation => TeamError::new("E_CONTROL_UNAUTHORIZED", "checkpoint authority changed"),
        CheckpointError::Io => TeamError::new("E_CHECKPOINT_IO", "checkpoint could not be persisted"),
        CheckpointError::Corrupt => TeamError::new("E_CHECKPOINT_CORRUPT", "checkpoint history is invalid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("team_repository_catalog_tests.rs");
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Barrier;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aperture-teams-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        for component in ["projects", "projects/aperture", "projects/aperture/.git"] {
            let directory = path.join(component);
            fs::create_dir(&directory).unwrap();
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }

    fn project_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
    }

    fn fullstack_input(team: &str) -> CreateTeamInput {
        CreateTeamInput {
            team: team.into(),
            project: "project:aperture".into(),
            repo: "aperture".into(),
            mission: "Implement the approved bounded transaction.".into(),
            acceptance: "Source review and isolated evidence pass.".into(),
            preset_id: Some("fullstack".into()),
            seats: vec![
                PresetSeat { role:"backend".into(), harness:Harness::Codex, model:"gpt-6-astra".into(), reasoning:Some(ReasoningEffort::High) },
                PresetSeat { role:"frontend".into(), harness:Harness::Claude, model:"opus".into(), reasoning:None },
                PresetSeat { role:"qa".into(), harness:Harness::Claude, model:"sonnet".into(), reasoning:None },
            ],
            lead_index: 0,
            fallbacks: vec![ExecutionTuple { harness:Harness::Codex, model:"gpt-5.6-sol".into(), reasoning:Some(ReasoningEffort::High) }],
        }
    }

    fn prepare_glados(home: &Path) {
        let token_root = home.join(".aperture/run/hub-tokens");
        ensure_private_dir(&token_root).unwrap();
        fs::write(token_root.join("glados.token"), b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        fs::set_permissions(token_root.join("glados.token"), fs::Permissions::from_mode(0o600)).unwrap();
        let glados = home.join(".claude/aperture/glados");
        ensure_private_dir(&glados).unwrap();
        write_private_bytes_atomic(&glados.join("prompt.md"), b"test\n", false).unwrap();
        write_private_json_atomic(&glados.join("manifest.json"), &serde_json::json!({
            "name":"GLaDOS","model":"sonnet","window":"glados","role":"orchestrator","enabled":true
        }), false).unwrap();
    }

    struct EnvRestore { values: Vec<(&'static str, Option<std::ffi::OsString>)> }
    impl EnvRestore {
        fn set(home: &Path) -> Self {
            let keys = ["HOME", "APERTURE_AGENTS_DIR", "APERTURE_TEAMS_DIR", "APERTURE_HUB_TOKEN_FILE"];
            let values = keys.into_iter().map(|key| (key, std::env::var_os(key))).collect();
            std::env::set_var("HOME", home);
            std::env::set_var("APERTURE_AGENTS_DIR", home.join(".claude/aperture"));
            std::env::set_var("APERTURE_TEAMS_DIR", home.join(".aperture/teams"));
            Self { values }
        }
    }

    fn bind_active_worker_with_token(home: &Path, seat: &TeamSeat, token: &[u8]) -> PathBuf {
        let token_path = home.join(".aperture/run/hub-tokens").join(format!("{}.token", seat.name));
        fs::write(&token_path, token).unwrap();
        fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600)).unwrap();
        let owner_path = home.join(".aperture/run/owner").join(format!("{}.json", seat.name));
        let mut owner: crate::owner::OwnerRecord = read_private_json(&owner_path).unwrap();
        owner.generation = 1;
        owner.state = crate::state::OwnerState::Active;
        owner.incarnation = Some(crate::owner::Incarnation {
            pid:123, start_time:456, thread_id:"thread-1".into(), token_id:format!("{:x}", Sha256::digest(token)),
            harness:seat.harness.clone(), model:seat.model.clone(), reasoning:seat.reasoning.clone(), observed:true,
            processes:vec![crate::owner::ProcessIdentity { pid:123,start_time:456,ppid:1,pgid:123,cmdline_sha256:"a".repeat(64),cwd:"/tmp/worktree".into() }],
        });
        write_private_json_atomic(&owner_path, &owner, true).unwrap();
        token_path
    }

    fn bind_active_worker(home: &Path, seat: &TeamSeat) -> PathBuf {
        bind_active_worker_with_token(
            home,
            seat,
            b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
    }

    fn capability_seat(
        name: &str,
        harness: Harness,
        owner: Option<OwnerSummary>,
    ) -> TeamSeatView {
        let is_codex = harness == Harness::Codex;
        TeamSeatView {
            configured: TeamSeat {
                name: name.into(),
                role: "backend".into(),
                model: if is_codex { "gpt-6-astra".into() } else { "sonnet".into() },
                reasoning: is_codex.then_some(ReasoningEffort::High),
                harness,
            },
            observed_owner: owner,
        }
    }

    fn capability_owner(
        configured: &TeamSeat,
        generation: u64,
        state: OwnerState,
    ) -> OwnerSummary {
        let active = state == OwnerState::Active;
        OwnerSummary {
            generation,
            state,
            since: "fixture-time".into(),
            configured: configured.tuple(),
            actual: active.then(|| configured.tuple()),
            process_count: u32::from(active),
            thread_bound: active,
        }
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (key, value) in self.values.drain(..) {
                match value { Some(value) => std::env::set_var(key, value), None => std::env::remove_var(key) }
            }
        }
    }

    include!("team_project_runtime_tests.rs");

    #[test]
    fn claude_inbox_probe_shared_wire_separates_input_from_readiness() {
        let req: TeamControlRequest = serde_json::from_str(include_str!("../../tests/fixtures/team-claude-inbox-request.json")).unwrap();
        let TeamControlRequest::ClaudeInboxProbe(input) = req else { panic!("wrong action") };
        let make = || crate::team_replacement::native::ClaudeInboxProbe {
            team:input.team.clone(),seat:input.seat.clone(),generation:1,
            actual:ExecutionTuple {harness:Harness::Claude,model:crate::team_claude_launch::MODEL.into(),reasoning:None},
        };
        let response = TeamControlResponse::ClaudeInboxProbe(project_claude_inbox_probe(&input,make()).unwrap());
        let expected: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/team-claude-inbox-response.json")).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(),expected);
        for case in 0..6 {
            let mut p = make();
            match case {0=>p.team="other".into(),1=>p.seat="other".into(),2=>p.generation=2,
                3=>p.actual.harness=Harness::Codex,4=>p.actual.model="sonnet".into(),_=>p.actual.reasoning=Some(ReasoningEffort::High)};
            assert_eq!(project_claude_inbox_probe(&input,p).unwrap_err().code,"E_CONTROL_UNKNOWN");
        }
    }

    #[test]
    fn retirement_control_auth_precedes_selectors_and_never_accepts_proof() {
        let _guard=crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        for action in ["retire_seat", "reconcile_stopped_retirement"] {
        let home=temp_root("retirement-control");let _env=EnvRestore::set(&home);
        let request=r#"{"action":"retire_seat","input":{"team":"t1","seat":"t1-qa","expected_generation":0,"accept_checkpoint_loss":true}}"#;
        let request=request.replace("retire_seat", action);
        assert_eq!(team_control_headless(&request).unwrap_err().code,"E_CONTROL_UNAUTHORIZED");
        prepare_glados(&home);
        assert_eq!(team_control_headless(&request).unwrap_err().code,"E_GENERATION_MISMATCH");
        for key in ["actor","pid","token","checkpoint","result","force","owner","model"] {
            let mut v:serde_json::Value=serde_json::from_str(&request).unwrap();v["input"][key]="forged".into();
            assert!(serde_json::from_value::<TeamControlRequest>(v).is_err());
        }
        let mut v:serde_json::Value=serde_json::from_str(&request).unwrap();v["input"].as_object_mut().unwrap().remove("accept_checkpoint_loss");
        assert!(serde_json::from_value::<TeamControlRequest>(v).is_err());
        assert!(!home.join(".aperture/run/owner/t1-qa.json").exists());
        fs::remove_dir_all(home).unwrap();
        }
    }

    #[test]
    fn claude_inbox_probe_auth_precedes_selectors_and_input_is_never_caller_owned() {
        let _guard=crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home=temp_root("inbox-probe-control");let _env=EnvRestore::set(&home);
        let request=r#"{"action":"claude_inbox_probe","input":{"team":"probe","seat":"probe-qa","expected_generation":1}}"#;
        assert_eq!(team_control_headless(request).unwrap_err().code,"E_CONTROL_UNAUTHORIZED");
        prepare_glados(&home);
        assert_eq!(team_control_headless(request).unwrap_err().code,"E_GENERATION_MISMATCH");
        for key in ["actor","model","reasoning","prompt","timeout","force","token","mode","pane","window"] {
            let mut v:serde_json::Value=serde_json::from_str(request).unwrap();v["input"][key]="forged".into();
            assert!(serde_json::from_value::<TeamControlRequest>(v).is_err());
        }
        assert!(!home.join(".aperture/run/owner/probe-qa.json").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn claude_startup_smoke_shared_wire_never_claims_mcp_or_public_readiness() {
        let request: TeamControlRequest = serde_json::from_str(include_str!("../../tests/fixtures/team-claude-smoke-request.json")).unwrap();
        let TeamControlRequest::ClaudeStartupSmoke(input) = request else { panic!("wrong action") };
        let diagnostic = crate::team_replacement::native::ClaudeSmokeDiagnostic {
            team: input.team.clone(), seat: input.seat.clone(), generation: 1,
            actual: ExecutionTuple { harness: Harness::Claude, model: crate::team_claude_launch::MODEL.into(), reasoning: None },
        };
        let response = TeamControlResponse::ClaudeStartupSmoke(project_claude_startup_smoke(&input, diagnostic).unwrap());
        let expected: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/team-claude-smoke-response.json")).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), expected);
        for case in 0..6 {
            let mut d = crate::team_replacement::native::ClaudeSmokeDiagnostic {
                team: input.team.clone(), seat: input.seat.clone(), generation: 1,
                actual: ExecutionTuple { harness: Harness::Claude, model: crate::team_claude_launch::MODEL.into(), reasoning: None },
            };
            match case {
                0 => d.team = "other".into(), 1 => d.seat = "other".into(), 2 => d.generation = 2,
                3 => d.actual.harness = Harness::Codex, 4 => d.actual.model = "sonnet".into(),
                _ => d.actual.reasoning = Some(ReasoningEffort::High),
            }
            assert_eq!(project_claude_startup_smoke(&input, d).unwrap_err().code, "E_CONTROL_UNKNOWN");
        }
    }

    #[test]
    fn stopped_recovery_control_is_authenticated_strict_and_never_reports_startup_pass() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("claude-recovery-control"); let _env = EnvRestore::set(&home);
        let request = r#"{"action":"reconcile_claude_startup","input":{"team":"smoke","seat":"smoke-qa","expected_generation":0}}"#;
        assert_eq!(team_control_headless(request).unwrap_err().code,"E_CONTROL_UNAUTHORIZED");
        prepare_glados(&home);
        assert_eq!(team_control_headless(request).unwrap_err().code,"E_GENERATION_MISMATCH");
        for key in ["actor","proof","pid","token","nonce","force","model"] {
            let mut forged:serde_json::Value=serde_json::from_str(request).unwrap();
            forged["input"][key]="forged".into();
            assert!(serde_json::from_value::<TeamControlRequest>(forged).is_err());
        }
        let result=serde_json::to_value(TeamControlResponse::ReconcileClaudeStartup(ReconciledClaudeStartupView {
            team:"smoke".into(),seat:"smoke-qa".into(),generation:1,owner_state:OwnerState::Quarantined,
            startup:"not_verified",cleanup:"stopped_reconciled",public_enabled:false,
        })).unwrap();
        assert_eq!(result["result"]["startup"],"not_verified");
        assert_eq!(result["result"]["owner_state"],"quarantined");
        assert!(!home.join(".aperture/run/owner/smoke-qa.json").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn claude_startup_smoke_control_requires_real_capability_and_fresh_selectors() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("claude-smoke-control");
        let _env = EnvRestore::set(&home);
        // Invalid generation proves authentication runs BEFORE selector checks;
        // this test never admits or launches a provider process.
        let request = r#"{"action":"claude_startup_smoke","input":{"team":"smoke","seat":"smoke-qa","expected_generation":1}}"#;
        assert_eq!(team_control_headless(request).unwrap_err().code, "E_CONTROL_UNAUTHORIZED");
        prepare_glados(&home);
        assert_eq!(team_control_headless(request).unwrap_err().code, "E_GENERATION_MISMATCH");
        // Normal launch replaces the diagnostic launch surface, not its history.
        for action in ["claude_startup_smoke", "claude_inbox_probe"] {
            let fresh = serde_json::json!({"action":action,"input":{"team":"smoke","seat":"smoke-qa","expected_generation":0}});
            assert_eq!(team_control_headless(&fresh.to_string()).unwrap_err().code, "E_STATE_CONFLICT");
        }
        for key in ["actor", "model", "reasoning", "prompt", "timeout", "force", "token"] {
            let mut forged: serde_json::Value = serde_json::from_str(request).unwrap();
            forged["input"][key] = serde_json::json!("forged");
            assert!(serde_json::from_value::<TeamControlRequest>(forged).is_err());
        }
        assert!(!home.join(".aperture/run/owner/smoke-qa.json").exists());
        assert!(!home.join(".aperture/teams/smoke").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn glados_stop_shared_wire_is_ready_not_started_or_archived() {
        let input: TeamControlRequest = serde_json::from_str(include_str!("../../tests/fixtures/team-stop-request.json")).unwrap();
        assert!(matches!(input, TeamControlRequest::StopSeat(StopSeatInput { expected_generation: 1, .. })));
        let response = TeamControlResponse::StopSeat(StopSeatView {
            team: "mural".into(), seat: "mural-frontend".into(), generation: 1,
            phase: ReplacementPhase::Ready, checkpoint_recovery: CheckpointRecovery::Valid,
            owner_state: OwnerState::Active, blockers: vec![],
        });
        let expected: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/team-stop-response.json")).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), expected);
    }

    #[test]
    fn glados_checkpoint_wire_and_authority_are_selectors_only() {
        let request: TeamControlRequest = serde_json::from_str(include_str!("../../tests/fixtures/team-checkpoint-validation-request.json")).unwrap();
        assert!(matches!(request, TeamControlRequest::ValidateCheckpoint(ValidateCheckpointInput { expected_generation:1, seq:2, .. })));
        let view = TeamControlResponse::ValidateCheckpoint(ValidateCheckpointView {
            team:"mural".into(), seat:"mural-frontend".into(), generation:1, seq:2, validation:"ok",
        });
        assert_eq!(serde_json::to_value(&view).unwrap(), serde_json::from_str::<serde_json::Value>(include_str!("../../tests/fixtures/team-checkpoint-validation-response.json")).unwrap());
        for key in ["actor", "path", "worktree", "observation", "result", "validator_kind", "force", "token"] {
            let mut forged = serde_json::to_value(&request).unwrap();
            forged["input"][key] = serde_json::json!("forged");
            assert!(serde_json::from_value::<TeamControlRequest>(forged).is_err());
        }
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("glados-checkpoint-control");
        let _env = EnvRestore::set(&home);
        let invalid = r#"{"action":"validate_checkpoint","input":{"team":"t1","seat":"t1-frontend","expected_generation":0,"seq":1}}"#;
        assert_eq!(team_control_headless(invalid).unwrap_err().code, "E_CONTROL_UNAUTHORIZED");
        prepare_glados(&home);
        assert_eq!(team_control_headless(invalid).unwrap_err().code, "E_GENERATION_MISMATCH");
        let target = RemoteTarget { team:"t1".into(), seat:"t1-frontend".into(), expected_generation:1 };
        for actor in [AuthenticatedActor::operator_ui(), AuthenticatedActor::launcher()] {
            assert_eq!(crate::team_replacement::native::validate_checkpoint_glados(&home, &actor, &target, 1, &[]).unwrap_err(), ReplacementError::AuthorizationRequired);
        }
        let actor = authenticate_glados_control().unwrap();
        write_private_bytes_atomic(&home.join(".aperture/run/hub-tokens/glados.token"), b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", true).unwrap();
        assert_eq!(crate::team_replacement::native::validate_checkpoint_glados(&home, &actor, &target, 1, &[]).unwrap_err(), ReplacementError::AuthorizationRequired);
        assert!(!home.join(".aperture/teams/t1").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn glados_stop_control_requires_real_capability_and_rejects_loss_overrides() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("glados-stop-control");
        let _env = EnvRestore::set(&home);
        let request = r#"{"action":"stop_seat","input":{"team":"t1","seat":"t1-frontend","expected_generation":0}}"#;
        assert_eq!(team_control_headless(request).unwrap_err().code, "E_CONTROL_UNAUTHORIZED");
        prepare_glados(&home);
        assert_eq!(team_control_headless(request).unwrap_err().code, "E_GENERATION_MISMATCH");
        for key in ["actor", "model", "timeout", "force", "discard_context", "token"] {
            let mut forged: serde_json::Value = serde_json::from_str(request).unwrap();
            forged["input"][key] = serde_json::json!("forged");
            assert!(serde_json::from_value::<TeamControlRequest>(forged).is_err());
        }
        for actor in [AuthenticatedActor::operator_ui(), AuthenticatedActor::launcher()] {
            assert_eq!(stop_for_archive(&home, &actor, "t1", "t1-frontend", 1, &[]).unwrap_err(), ReplacementError::AuthorizationRequired);
        }
        let actor = authenticate_glados_control().unwrap();
        assert_eq!(stop_for_archive(&home, &actor, "t1", "t1-frontend", 0, &[]).unwrap_err(), ReplacementError::GenerationMismatch);
        // Even a real GLaDOS archive principal cannot reuse the replacement
        // entrypoint to gain start authority or degraded checkpoint recovery.
        let target = RemoteTarget { team: "t1".into(), seat: "t1-frontend".into(), expected_generation: 1 };
        assert_eq!(replace_authorized(&home, ReplacementAuthority::GladosArchive(&actor), target,
            &StartSelection { harness: "codex".into(), model: "gpt-6-astra".into(), reasoning: Some("high".into()) }, &[]).unwrap_err(),
            ReplacementError::AuthorizationRequired);
        let token = home.join(".aperture/run/hub-tokens/glados.token");
        write_private_bytes_atomic(&token, b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", true).unwrap();
        assert_eq!(stop_for_archive(&home, &actor, "t1", "t1-frontend", 1, &[]).unwrap_err(), ReplacementError::AuthorizationRequired);
        assert!(!home.join(".aperture/run/owner/t1-frontend.json").exists());
        assert!(!home.join(".aperture/teams/t1").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn glados_bootstrap_receipt_matches_shared_native_wire_fixture() {
        let tuple = ExecutionTuple { harness: Harness::Codex, model: "gpt-5.6-sol".into(), reasoning: Some(ReasoningEffort::High) };
        let response = TeamControlResponse::BootstrapSeat(BootstrapView {
            team: "mural".into(), seat: "mural-frontend".into(), generation: 1,
            phase: BootstrapPhase::Started, owner: Some(OwnerSummary {
                generation: 1, state: crate::state::OwnerState::Active, since: "2026-09-21T00:00:00Z".into(),
                configured: tuple.clone(), actual: Some(tuple), process_count: 1, thread_bound: true,
            }), blockers: vec![],
        });
        let expected: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/team-bootstrap-response.json")).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), expected);
    }

    #[test]
    fn glados_bootstrap_control_uses_real_capability_and_strict_selectors() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("glados-bootstrap-control");
        let _env = EnvRestore::set(&home);
        let request = r#"{"action":"bootstrap_seat","input":{"team":"t1","seat":"t1-frontend","expected_generation":1}}"#;
        assert_eq!(team_control_headless(request).unwrap_err().code, "E_CONTROL_UNAUTHORIZED");
        prepare_glados(&home);
        // The real authenticated child reaches the recovery proof, which finds no
        // owner record for the seat (not the recoverable generation), not a fake
        // operator actor; selector 2 reaches the same proof; 3 fails at validation.
        assert_eq!(team_control_headless(request).unwrap_err().code, "E_GENERATION_MISMATCH");
        assert_eq!(team_control_headless(&request.replace("\"expected_generation\":1", "\"expected_generation\":2")).unwrap_err().code, "E_GENERATION_MISMATCH");
        assert_eq!(team_control_headless(&request.replace("\"expected_generation\":1", "\"expected_generation\":3")).unwrap_err().code, "E_GENERATION_MISMATCH");
        for key in ["actor", "model", "timeout", "generation", "token"] {
            let mut forged: serde_json::Value = serde_json::from_str(request).unwrap();
            forged["input"][key] = serde_json::json!("forged");
            assert!(serde_json::from_value::<TeamControlRequest>(forged).is_err());
        }
        let actor = authenticate_glados_control().unwrap();
        // Selectors 1 and 2 are the recovery entries: with GLaDOS authenticated they
        // reach the native proof, which finds no owner record; 3 is never a selector
        // and is refused before any lock or read.
        assert_eq!(bootstrap_authorized(&home, &actor, "t1", "t1-frontend", 1).unwrap_err(),
            crate::team_replacement::ReplacementError::GenerationMismatch);
        assert_eq!(bootstrap_authorized(&home, &actor, "t1", "t1-frontend", 2).unwrap_err(),
            crate::team_replacement::ReplacementError::GenerationMismatch);
        assert_eq!(bootstrap_authorized(&home, &actor, "t1", "t1-frontend", 3).unwrap_err(),
            crate::team_replacement::ReplacementError::GenerationMismatch);
        // Capability is valid at preflight; a replacement before final commit
        // must fail under the same lock activation uses, without owner mutation.
        drop(crate::team_replacement::native::lock_activation(&home, "t1", Some(&actor)).unwrap());
        let token = home.join(".aperture/run/hub-tokens/glados.token");
        let replaced = token.with_extension("replacement");
        fs::write(&replaced, b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        fs::set_permissions(&replaced, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(replaced, token).unwrap();
        assert_eq!(bootstrap_authorized(&home, &actor, "t1", "t1-frontend", 0).unwrap_err(),
            crate::team_replacement::ReplacementError::AuthorizationRequired);
        assert!(matches!(crate::team_replacement::native::lock_activation(&home, "t1", Some(&actor)),
            Err(crate::team_replacement::ReplacementError::AuthorizationRequired)));
        assert!(!home.join(".aperture/run/owner/t1-frontend.json").exists());
        assert!(!home.join(".aperture/teams/t1").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn repository_registry_lock_serializes_admission_and_competing_saves() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("registry-lock");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        registry_fixture_repo(&home, "eunenem-engine");
        let engine = TeamEngine::new(home.clone(), project_root());
        let actor = authenticate_glados_control().unwrap();
        let initial = engine.list_repositories().unwrap();
        let pending = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("lock-pending")).unwrap();
        let held = repository_registry::lock(&home).unwrap();
        assert_eq!(engine.save_repository(&actor, registry_fixture_input(&initial.sha256, true)).unwrap_err().code, "E_LOCK_HELD");
        assert_eq!(engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("lock-denied")).unwrap_err().code, "E_LOCK_HELD");
        assert_eq!(engine.activate(&actor, ActivateTeamInput {
            team: "lock-pending".into(), expected_generation: 0,
            creation_request_id: pending.creation_request.request_id, epic_id: "aperture-fixture".into(),
        }).unwrap_err().code, "E_LOCK_HELD");
        assert!(!home.join(".aperture/teams/lock-denied").exists());
        assert_eq!(engine.read_team_view("lock-pending").unwrap().state.state, TeamLifecycle::Pending);
        drop(held);
        let barrier = std::sync::Arc::new(Barrier::new(2));
        let handles: Vec<_> = ["Engine A", "Engine B"].into_iter().map(|display| {
            let home = home.clone(); let barrier = barrier.clone(); let hash = initial.sha256.clone();
            std::thread::spawn(move || {
                let engine = TeamEngine::new(home, project_root());
                let actor = authenticate_glados_control().unwrap();
                let mut input = registry_fixture_input(&hash, true); input.display_name = display.into();
                barrier.wait();
                engine.save_repository(&actor, input)
            })
        }).collect();
        let results: Vec<_> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let error = results.into_iter().find_map(Result::err).unwrap();
        assert!(matches!(error.code.as_str(), "E_LOCK_HELD" | "E_REPOSITORY_CONFLICT"));
        assert_eq!(engine.list_repositories().unwrap().repositories.len(), REPOSITORY_SEEDS.len() + 1);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn repository_control_matches_shared_real_request_and_response() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("registry-wire");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        ensure_private_dir(&home.join("projects/eunenem-engine/.git")).unwrap();
        let request = include_str!("../../mcp-server/test/fixtures/repository-save-request.json");
        let expected: serde_json::Value = serde_json::from_str(include_str!("../../mcp-server/test/fixtures/repository-save-response.json")).unwrap();
        let actual = serde_json::to_value(team_control_headless(request).unwrap()).unwrap();
        assert_eq!(actual, expected, "real Rust output must match the fixture parsed by MCP");
        let listed = serde_json::to_value(team_control_headless(r#"{"action":"list_repositories"}"#).unwrap()).unwrap();
        assert_eq!(listed["result"], expected["result"]);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn conversational_create_uses_authenticated_control_and_stays_pending() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("conversation-create");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let json = include_str!("../../mcp-server/test/fixtures/team-create-request.json");
        // The exact request fixture is also consumed by the MCP tests.
        let response = team_control_headless(json).unwrap();
        let TeamControlResponse::Create(created) = response else { panic!("expected pending create response") };
        assert_eq!(created.team.state.state, TeamLifecycle::Pending);
        assert_eq!(created.team.state.generation, 0);
        assert!(created.team.state.epic_id.is_none());
        assert!(!created.team.capabilities.start);
        assert_eq!(created.team.snapshot.seats.len(), 2);
        for seat in &created.team.snapshot.seats {
            assert!(!home.join(".claude/aperture").join(&seat.name).exists());
            assert!(!home.join(".aperture/run/owner").join(format!("{}.json",seat.name)).exists());
            assert!(!home.join(".aperture/run/hub-tokens").join(format!("{}.token",seat.name)).exists());
        }
        assert!(matches!(team_control_headless(r#"{"action":"catalog"}"#).unwrap(), TeamControlResponse::Catalog(_)));
        assert!(team_control_headless(json).is_err(), "duplicate creation cannot silently retry");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn conversational_create_rejects_forgery_and_missing_capability_before_write() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("conversation-denied");
        let _env = EnvRestore::set(&home);
        let json = include_str!("../../mcp-server/test/fixtures/team-create-request.json");
        assert!(team_control_headless(json).is_err());
        assert!(!home.join(".aperture/teams").exists());
        prepare_glados(&home);
        for field in ["actor", "grants", "created_at", "creation_request_id", "source"] {
            let mut forged: serde_json::Value = serde_json::from_str(json).unwrap();
            forged["input"][field] = serde_json::json!("glados");
            assert!(team_control_headless(&forged.to_string()).is_err());
            assert!(!home.join(".aperture/teams").exists());
        }
        let engine = TeamEngine::new(home.clone(), project_root());
        let TeamControlRequest::Create(input) = serde_json::from_str(json).unwrap() else { unreachable!() };
        assert!(engine.create_team(&AuthenticatedActor::launcher(), input).is_err());
        assert!(!home.join(".aperture/teams").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn conversational_create_revalidates_replaced_capability_before_any_write() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("conversation-replaced-capability");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let actor = authenticate_glados_control().unwrap();
        let token = home.join(".aperture/run/hub-tokens/glados.token");
        fs::rename(&token, token.with_extension("old")).unwrap();
        fs::write(&token, b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        let engine = TeamEngine::new(home.clone(), project_root());
        assert!(engine.create_team(&actor, fullstack_input("replaced")).is_err());
        assert!(!home.join(".aperture/teams").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn real_templates_render_pending_then_activate_atomically() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("activate");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("t1")).unwrap();
        assert_eq!(created.team.state.state, TeamLifecycle::Pending);
        assert_eq!(created.team.capabilities, TeamCapabilities {
            cancel: true, activate: true, start: false, checkpoint: false, replace: false, archive: false,
        });
        assert_eq!(created.team.seats.len(), 3);
        for seat in &created.team.snapshot.seats {
            assert!(!home.join(".claude/aperture").join(&seat.name).exists());
            let staged = home.join(".aperture/teams/.staging").join(&created.team.snapshot.staging_uuid).join("seats").join(&seat.name);
            assert!(staged.join("prompt.md").is_file());
            assert_eq!(fs::read_to_string(staged.join("resident.txt")).unwrap(), format!("constitution\n{}-core\n", seat.role));
            let prompt = fs::read_to_string(staged.join("prompt.md")).unwrap();
            assert!(prompt.contains(&format!("**{}**", seat.name)));
            assert!(prompt.contains("Assigned mission (bounded JSON data)"));
            assert!(!prompt.contains("{{"));
        }
        let actor = authenticate_glados_control().unwrap();
        let active = engine.activate(&actor, ActivateTeamInput {
            team:"t1".into(), expected_generation:0,
            creation_request_id:created.creation_request.request_id.clone(), epic_id:"aperture-4rsnc".into(),
        }).unwrap();
        assert_eq!(active.state.state, TeamLifecycle::Active);
        assert_eq!(active.state.generation, 1);
        assert_eq!(active.capabilities, TeamCapabilities {
            cancel: false, activate: false, start: true, checkpoint: false, replace: false, archive: true,
        });
        for seat in &active.seats {
            let runtime = home.join(".claude/aperture").join(&seat.configured.name);
            assert!(runtime.join(".complete").is_file());
            assert!(runtime.join("TEAM").is_file());
            assert_eq!(seat.observed_owner.as_ref().unwrap().generation, 0);
            assert_eq!(classify_managed_seat(&home, &seat.configured.name).unwrap(), Some(ManagedSeatState::Active { team:"t1".into(), generation:1 }));
        }
        assert_eq!(crate::agent_loader::load_agents_from_disk().keys().filter(|name| name.starts_with("t1-")).count(), 3);
        ensure_private_dir(&home.join(".aperture/run/team-journals")).unwrap();
        write_private_json_atomic(
            &home.join(".aperture/run/team-journals/t1.archive.json"),
            &serde_json::json!({"fixture": true}),
            false,
        ).unwrap();
        for seat in &active.seats {
            assert_eq!(
                classify_managed_seat(&home, &seat.configured.name).unwrap_err().code,
                "E_JOURNAL_INCONSISTENT",
            );
        }
        assert_eq!(classify_managed_seat(&home, "standing").unwrap(), None);
        assert_eq!(crate::agent_loader::load_agents_from_disk().keys().filter(|name| name.starts_with("t1-")).count(), 0);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn normal_sonnet_admission_is_literal_and_legacy_aliases_do_not_launch() {
        assert!(managed_launch_enabled(&Harness::Codex));
        assert!(managed_launch_enabled(&Harness::Claude));
        let tuple = ExecutionTuple { harness: Harness::Claude, model: "claude-sonnet-5".into(), reasoning: None };
        assert!(validate_execution_tuple(&tuple).is_ok());
        for model in ["opus", "sonnet"] {
            assert!(validate_execution_tuple(&ExecutionTuple { model: model.into(), ..tuple.clone() }).is_ok(), "legacy snapshots remain readable");
        }
        assert!(validate_execution_tuple(&ExecutionTuple { reasoning: Some(ReasoningEffort::High), ..tuple.clone() }).is_err());
        assert!(validate_execution_tuple(&ExecutionTuple { harness: Harness::Codex, ..tuple.clone() }).is_err());
        assert!(validate_execution_tuple(&ExecutionTuple { model: "claude-sonnet-guessed".into(), ..tuple.clone() }).is_err());
        for model in ["claude-fable-5-1", "claude-opus-5"] {
            let exact = ExecutionTuple { model: model.into(), ..tuple.clone() };
            assert!(validate_execution_tuple(&exact).is_ok(), "{model} is an authorized exact literal");
            assert!(validate_execution_tuple(&ExecutionTuple { reasoning: Some(ReasoningEffort::High), ..exact.clone() }).is_err());
            assert!(managed_execution_enabled(&exact));
            let mut seat = capability_seat("t1-qa", Harness::Claude, None);
            seat.configured.model = model.into();
            seat.configured.reasoning = None;
            seat.observed_owner = Some(capability_owner(&seat.configured, 0, OwnerState::Stale));
            assert!(team_capabilities(&TeamLifecycle::Active, &[seat]).start, "{model} fresh exact seat is startable");
        }
        for model in ["fable", "claude-fable-5", "claude-opus-5-5", "claude-fable-5-1[1m]", "claude-opus-5 ", "CLAUDE-OPUS-5"] {
            let bad = ExecutionTuple { model: model.into(), ..tuple.clone() };
            assert!(validate_execution_tuple(&bad).is_err(), "{model} must not be admitted");
            assert!(!managed_execution_enabled(&bad), "{model} must not launch");
        }
        assert_eq!(execution_catalog().iter().filter(|t| t.harness == Harness::Claude && crate::team_claude_launch::is_exact_claude_model(&t.model)).count(), 3);
        let mut seat = capability_seat("t1-qa", Harness::Claude, None);
        seat.configured.model = tuple.model;
        seat.configured.reasoning = None;
        seat.observed_owner = Some(capability_owner(&seat.configured, 0, OwnerState::Stale));
        let caps = team_capabilities(&TeamLifecycle::Active, &[seat.clone()]);
        assert!(caps.start && !caps.replace, "fresh exact normal Sonnet is startable, not observed or replaceable");
        seat.observed_owner = Some(capability_owner(&seat.configured, 1, OwnerState::Active));
        let running = team_capabilities(&TeamLifecycle::Active, &[seat.clone()]);
        assert!(!running.start && running.replace);
        seat.observed_owner.as_mut().unwrap().actual = None;
        assert!(!team_capabilities(&TeamLifecycle::Active, &[seat]).replace);
        for model in ["opus", "sonnet", "claude-sonnet-guessed"] {
            let mut old = capability_seat("t1-qa", Harness::Claude, None);
            old.configured.model = model.into();
            old.observed_owner = Some(capability_owner(&old.configured, 0, OwnerState::Stale));
            assert!(!team_capabilities(&TeamLifecycle::Active, &[old]).start);
        }
        // Recoverable Claude bootstrap: quarantined g1 (first bootstrap) or g2 (its
        // one recovery that failed the same way), never observed, thread never
        // bound. process_count is history (the Gone root is still recorded), so it
        // is not required to be zero; the native proof decides liveness. g3 is
        // never recoverable.
        let mut recoverable = capability_seat("t1-qa", Harness::Claude, None);
        recoverable.configured.model = "claude-sonnet-5".into();
        recoverable.configured.reasoning = None;
        recoverable.observed_owner = Some(capability_owner(&recoverable.configured, 1, OwnerState::Quarantined));
        recoverable.observed_owner.as_mut().unwrap().process_count = 1;
        assert!(team_capabilities(&TeamLifecycle::Active, &[recoverable.clone()]).start, "quarantined unobserved g1 Claude is startable");
        let mut second = recoverable.clone();
        second.observed_owner.as_mut().unwrap().generation = 2;
        assert!(team_capabilities(&TeamLifecycle::Active, &[second]).start, "quarantined unobserved g2 Claude is startable once more");
        for corrupt in [
            |o: &mut OwnerSummary| o.thread_bound = true,
            |o: &mut OwnerSummary| o.actual = Some(o.configured.clone()),
            |o: &mut OwnerSummary| o.generation = 3,
            |o: &mut OwnerSummary| o.state = OwnerState::Stale,
        ] {
            let mut invalid = recoverable.clone();
            corrupt(invalid.observed_owner.as_mut().unwrap());
            assert!(!team_capabilities(&TeamLifecycle::Active, &[invalid]).start);
        }
        let mut codex = capability_seat("t1-backend", Harness::Codex, None);
        codex.observed_owner = Some(capability_owner(&codex.configured, 1, OwnerState::Quarantined));
        assert!(!team_capabilities(&TeamLifecycle::Active, &[codex]).start, "recovery is a Claude first-bootstrap closure only");
    }

    #[test]
    fn capabilities_are_action_and_seat_eligible_without_enabling_legacy_claude_aliases() {
        let mut codex = capability_seat("t1-backend", Harness::Codex, None);
        codex.observed_owner = Some(capability_owner(
            &codex.configured,
            0,
            OwnerState::Stale,
        ));
        let mut claude = capability_seat("t1-qa", Harness::Claude, None);
        claude.observed_owner = Some(capability_owner(
            &claude.configured,
            0,
            OwnerState::Stale,
        ));

        let mixed = vec![codex.clone(), claude.clone()];
        assert_eq!(
            team_capabilities(&TeamLifecycle::Active, &mixed),
            TeamCapabilities {
                cancel: false,
                activate: false,
                start: true,
                checkpoint: false,
                replace: false,
                archive: true,
            }
        );
        for lifecycle in [
            TeamLifecycle::Pending,
            TeamLifecycle::Failed,
            TeamLifecycle::Archived,
        ] {
            let capabilities = team_capabilities(&lifecycle, &mixed);
            assert!(!capabilities.start);
            assert!(!capabilities.replace);
            assert!(!capabilities.archive);
            assert!(!capabilities.checkpoint);
        }
        for corrupt in [
            |owner: &mut OwnerSummary| owner.actual = Some(owner.configured.clone()),
            |owner: &mut OwnerSummary| owner.process_count = 1,
            |owner: &mut OwnerSummary| owner.thread_bound = true,
        ] {
            let mut invalid = codex.clone();
            corrupt(invalid.observed_owner.as_mut().unwrap());
            assert!(!team_capabilities(&TeamLifecycle::Active, &[invalid]).start);
        }

        codex.observed_owner = Some(capability_owner(
            &codex.configured,
            1,
            OwnerState::Active,
        ));
        let running = team_capabilities(&TeamLifecycle::Active, &[codex.clone(), claude.clone()]);
        assert!(!running.start, "the stale Claude seat is not bootable by the Codex adapter");
        assert!(running.replace);
        assert!(running.archive);

        let fallback = ExecutionTuple {
            harness: Harness::Codex,
            model: "gpt-5.6-sol".into(),
            reasoning: Some(ReasoningEffort::High),
        };
        let owner = codex.observed_owner.as_mut().unwrap();
        owner.configured = fallback.clone();
        owner.actual = Some(fallback);
        assert!(team_capabilities(&TeamLifecycle::Active, &[codex.clone()]).replace,
            "an observed authorized Codex fallback remains replacement-eligible");

        codex.observed_owner.as_mut().unwrap().actual = Some(ExecutionTuple {
            harness: Harness::Codex,
            model: "gpt-5.6-luna".into(),
            reasoning: Some(ReasoningEffort::High),
        });
        assert!(!team_capabilities(&TeamLifecycle::Active, &[codex.clone()]).replace);

        codex.observed_owner.as_mut().unwrap().state = OwnerState::Quarantined;
        assert!(!team_capabilities(&TeamLifecycle::Active, &[codex]).replace);
        claude.observed_owner = Some(capability_owner(
            &claude.configured,
            2,
            OwnerState::Active,
        ));
        assert!(!team_capabilities(&TeamLifecycle::Active, &[claude]).replace);
    }

    #[test]
    fn pending_cancel_is_idempotent_and_never_publishes_a_seat() {
        let home = temp_root("cancel");
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("t2")).unwrap();
        let input = CancelPendingInput { team:"t2".into(), expected_generation:0, creation_request_id:created.creation_request.request_id };
        let first = engine.cancel_pending(&AuthenticatedActor::operator_ui(), input.clone()).unwrap();
        let second = engine.cancel_pending(&AuthenticatedActor::operator_ui(), input).unwrap();
        assert_eq!(first, second);
        assert!(!home.join(".aperture/teams/t2").exists());
        assert!(home.join(".aperture/teams/.staging/rejected").join(&first.rejected_snapshot_id).join("snapshot/team.json").is_file());
        assert!(created.team.snapshot.seats.iter().all(|seat| !home.join(".claude/aperture").join(&seat.name).exists()));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn preset_cas_and_authoritative_source_are_enforced() {
        let home = temp_root("preset");
        let engine = TeamEngine::new(home.clone(), project_root());
        let shipped = engine.list_presets().unwrap().into_iter().find(|p| p.id == "fullstack").unwrap();
        assert_eq!(shipped.source, PresetSource::Shipped);
        let write = TeamPresetWrite {
            schema_version:shipped.schema_version, id:shipped.id.clone(), display_name:shipped.display_name.clone(),
            mission_placeholder:shipped.mission_placeholder.clone(), acceptance_placeholder:shipped.acceptance_placeholder.clone(),
            seats:shipped.seats.clone(), lead_index:shipped.lead_index, fallbacks:shipped.fallbacks.clone(),
        };
        assert_eq!(engine.save_preset(&AuthenticatedActor::operator_ui(), SavePresetInput { preset:write.clone(), expected_sha256:None }).unwrap_err().code, "E_PRESET_CONFLICT");
        let saved = engine.save_preset(&AuthenticatedActor::operator_ui(), SavePresetInput { preset:write.clone(), expected_sha256:Some(shipped.sha256) }).unwrap();
        assert_eq!(saved.source, PresetSource::Local);
        assert_eq!(engine.save_preset(&AuthenticatedActor::operator_ui(), SavePresetInput { preset:write, expected_sha256:Some("0".repeat(64)) }).unwrap_err().code, "E_PRESET_CONFLICT");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn repository_catalog_and_create_binding_are_authoritative() {
        let home = temp_root("repo-binding");
        let engine = TeamEngine::new(home.clone(), project_root());
        let catalog = engine.catalog().unwrap();
        assert!(catalog.repositories.iter().any(|entry| {
            entry.project == "project:aperture" && entry.repo == "aperture" && entry.available
        }));
        assert_eq!(catalog.repositories.iter().filter(|entry| entry.project == "project:incluir").count(), 2);
        assert!(!catalog.repositories.iter().any(|entry| entry.project == "project:mempalace"));

        let mut missing = fullstack_input("repo-missing");
        missing.repo.clear();
        assert_eq!(engine.create_team(&AuthenticatedActor::operator_ui(), missing).unwrap_err().code, "E_REPO_REQUIRED");

        let mut path_like = fullstack_input("repo-path");
        path_like.repo = "../aperture".into();
        assert_eq!(engine.create_team(&AuthenticatedActor::operator_ui(), path_like).unwrap_err().code, "E_REPO_NOT_IN_CATALOG");

        let mut wrong_project = fullstack_input("repo-project");
        wrong_project.project = "project:frame".into();
        assert_eq!(engine.create_team(&AuthenticatedActor::operator_ui(), wrong_project).unwrap_err().code, "E_REPO_NOT_IN_CATALOG");

        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("repo-echo")).unwrap();
        assert_eq!(created.team.snapshot.repo, "aperture");
        assert_eq!(created.creation_request.repo, created.team.snapshot.repo);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn repository_disappearance_blocks_activation_before_publication() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("repo-disappears");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("repo-gone")).unwrap();
        fs::remove_dir_all(home.join("projects/aperture/.git")).unwrap();
        let error = engine.activate(&authenticate_glados_control().unwrap(), ActivateTeamInput {
            team: "repo-gone".into(),
            expected_generation: 0,
            creation_request_id: created.creation_request.request_id,
            epic_id: "aperture-4rsnc".into(),
        }).unwrap_err();
        assert_eq!(error.code, "E_REPO_UNAVAILABLE");
        assert_eq!(engine.read_team_view("repo-gone").unwrap().state.state, TeamLifecycle::Pending);
        assert!(created.team.snapshot.seats.iter().all(|seat| !home.join(".claude/aperture").join(&seat.name).exists()));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn invalid_text_tuple_and_symlink_role_fail_before_publication() {
        let home = temp_root("negative");
        let engine = TeamEngine::new(home.clone(), project_root());
        let mut input = fullstack_input("t3");
        input.mission = "bad\u{202e}text".into();
        assert_eq!(engine.create_team(&AuthenticatedActor::operator_ui(), input).unwrap_err().code, "E_PRESET_INVALID");
        assert!(!home.join(".aperture/teams/t3").exists());
        let mut input = fullstack_input("t4");
        input.seats[0].model = "gpt-6-astra-evil".into();
        assert_eq!(engine.create_team(&AuthenticatedActor::operator_ui(), input).unwrap_err().code, "E_PRESET_INVALID");
        assert!(!home.join(".aperture/teams/t4").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn classifier_rejects_orphaned_marker_and_preserves_standing_agent() {
        let home = temp_root("classify");
        ensure_private_dir(&home.join(".claude/aperture/standing")).unwrap();
        assert_eq!(classify_managed_seat(&home, "standing").unwrap(), None);
        write_private_bytes_atomic(&home.join(".claude/aperture/standing/TEAM"), b"{}\n", false).unwrap();
        assert_eq!(classify_managed_seat(&home, "standing").unwrap_err().code, "E_STATE_CONFLICT");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn classifier_rejects_dangling_registry_symlinks_instead_of_falling_back_to_legacy() {
        let home = temp_root("classify-dangling");
        ensure_private_dir(&home.join(".claude/aperture/standing")).unwrap();
        ensure_private_dir(&home.join(".aperture")).unwrap();
        std::os::unix::fs::symlink(home.join("missing-teams"), home.join(".aperture/teams")).unwrap();
        assert_eq!(classify_managed_seat(&home, "standing").unwrap_err().code, "E_PATH_UNSAFE");
        fs::remove_file(home.join(".aperture/teams")).unwrap();
        ensure_private_dir(&home.join(".aperture/teams")).unwrap();
        std::os::unix::fs::symlink(home.join("missing-archive"), home.join(".aperture/teams/archive")).unwrap();
        assert_eq!(classify_managed_seat(&home, "standing").unwrap_err().code, "E_PATH_UNSAFE");
        fs::remove_file(home.join(".aperture/teams/archive")).unwrap();
        ensure_private_dir(&home.join(".aperture/run")).unwrap();
        std::os::unix::fs::symlink(
            home.join("missing-journals"),
            home.join(".aperture/run/team-journals"),
        )
        .unwrap();
        assert_eq!(classify_managed_seat(&home, "standing").unwrap_err().code, "E_PATH_UNSAFE");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn owner_requested_tuple_must_follow_snapshot_or_approved_fallback_policy() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("owner-policy");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("t6")).unwrap();
        let actor = authenticate_glados_control().unwrap();
        engine.activate(&actor, ActivateTeamInput { team:"t6".into(), expected_generation:0, creation_request_id:created.creation_request.request_id, epic_id:"aperture-4rsnc".into() }).unwrap();
        let seat = "t6-backend";
        let owner_path = home.join(".aperture/run/owner").join(format!("{seat}.json"));
        let mut record: crate::owner::OwnerRecord = read_private_json(&owner_path).unwrap();
        record.generation = 1;
        record.state = crate::state::OwnerState::Starting;
        record.requested = ExecutionTuple { harness:Harness::Codex, model:"gpt-5.6-sol".into(), reasoning:Some(ReasoningEffort::High) };
        write_private_json_atomic(&owner_path, &record, true).unwrap();
        assert!(engine.read_team_view("t6").is_ok(), "approved fallback remains observable");
        record.requested = ExecutionTuple { harness:Harness::Codex, model:"gpt-5.6-luna".into(), reasoning:Some(ReasoningEffort::High) };
        write_private_json_atomic(&owner_path, &record, true).unwrap();
        assert_eq!(engine.read_team_view("t6").unwrap_err().code, "E_STATE_CONFLICT");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn active_replay_finishes_journal_cleanup_before_returning_loadable_team() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("active-recovery");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("t7")).unwrap();
        let input = ActivateTeamInput {
            team: "t7".into(), expected_generation: 0,
            creation_request_id: created.creation_request.request_id,
            epic_id: "aperture-4rsnc".into(),
        };
        engine.activate(&authenticate_glados_control().unwrap(), input.clone()).unwrap();
        let view = engine.read_team_view("t7").unwrap();
        let mut seats: Vec<_> = view.snapshot.seats.iter().map(|seat| seat.name.clone()).collect();
        seats.sort();
        let mut moves = Vec::new();
        for seat in &seats {
            moves.push(JournalMove {
                from_root: JournalRoot::Staging,
                from_rel: format!("{}/seats/{seat}", view.snapshot.staging_uuid),
                to_root: JournalRoot::Agents,
                to_rel: seat.clone(),
                kind: JournalObjectKind::Directory,
            });
        }
        for seat in &seats {
            moves.push(JournalMove {
                from_root: JournalRoot::Staging,
                from_rel: format!("{}/owners/{seat}.json", view.snapshot.staging_uuid),
                to_root: JournalRoot::Owner,
                to_rel: format!("{seat}.json"),
                kind: JournalObjectKind::File,
            });
        }
        let team_dir = home.join(".aperture/teams/t7");
        let request: CreationRequestDTO =
            read_private_json(&team_dir.join("activation_request.json")).unwrap();
        write_journal(
            &team_dir.join("journal.json"),
            &Journal {
                schema_version: 1,
                operation: JournalOperation::Activate,
                team: "t7".into(),
                uuid: view.snapshot.staging_uuid.clone(),
                step: moves.len(),
                moves,
                preimage_sha256: request.snapshot_sha256,
                archive_approval: None,
                archive_preimage: vec![],
                archive_mutable_files: vec![],
            },
        )
        .unwrap();
        let marker = home
            .join(".claude/aperture")
            .join(&seats[0])
            .join(".complete");
        fs::remove_file(&marker).unwrap();

        let recovered = engine.activate(&authenticate_glados_control().unwrap(), input).unwrap();
        assert_eq!(recovered.state.state, TeamLifecycle::Active);
        assert!(marker.is_file());
        assert!(!team_dir.join("journal.json").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn headless_control_derives_glados_and_rejects_forged_actor_fields() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("headless-control");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("t8")).unwrap();

        assert!(team_control_headless(r#"{"action":"list_pending","actor":"glados"}"#)
            .unwrap_err()
            .message
            .contains("invalid team control request"));
        let listed = team_control_headless(r#"{"action":"list_pending"}"#).unwrap();
        assert!(matches!(listed, TeamControlResponse::ListPending(ref teams) if teams.len() == 1 && teams[0].snapshot.team == "t8"));

        let cancelled = team_control_headless(&serde_json::to_string(&TeamControlRequest::Cancel(CancelPendingInput {
            team: "t8".into(),
            expected_generation: 0,
            creation_request_id: created.creation_request.request_id,
        })).unwrap()).unwrap();
        assert!(matches!(cancelled, TeamControlResponse::Cancel(CancelPendingResult { cancelled: true, .. })));
        assert!(!home.join(".aperture/teams/t8").exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn managed_worker_checkpoint_derives_identity_and_rejects_authority_fields() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("worker-checkpoint");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("t9")).unwrap();
        engine.activate(&authenticate_glados_control().unwrap(), ActivateTeamInput { team:"t9".into(), expected_generation:0, creation_request_id:created.creation_request.request_id, epic_id:"aperture-4rsnc".into() }).unwrap();
        let seat = created.team.snapshot.seats[0].clone();
        let token_path = bind_active_worker(&home, &seat);
        std::env::set_var("APERTURE_HUB_TOKEN_FILE", &token_path);
        let payload = CheckpointPayload {
            task_id:"aperture-fixture".into(), worktree:"aperture-worktrees/aperture-fixture".into(), branch:"aperture-fixture".into(), head_sha:"a".repeat(40),
            dirty_files:vec![], open_pr:None, running_procs:vec![], decisions:vec![], next_step:"Continue the approved bounded implementation.".into(), remote_effects:vec![],
        };
        let request = TeamControlRequest::Checkpoint(WriteCheckpointInput { schema_version:1, payload });
        let request_json = serde_json::to_string(&request).unwrap();
        let response = team_control_headless(&request_json).unwrap();
        assert!(matches!(response, TeamControlResponse::Checkpoint(CheckpointReceipt { status, .. }) if status == "pending"));
        let checkpoint_dir = home.join(".aperture/teams/t9/checkpoints").join(&seat.name);
        assert_eq!(fs::read_dir(&checkpoint_dir).unwrap().count(), 1);

        let same_inode = authenticate_seat_control().unwrap();
        fs::write(&token_path, b"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc").unwrap();
        assert!(same_inode.revalidate_before_effect().unwrap_err().contains("contents changed"));
        fs::write(&token_path, b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        let replaced_inode = authenticate_seat_control().unwrap();
        let replacement = token_path.with_file_name("replacement.token");
        fs::write(&replacement, b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&replacement, &token_path).unwrap();
        assert!(replaced_inode.revalidate_before_effect().unwrap_err().contains("canonical capability changed"));

        let forged = serde_json::json!({"action":"checkpoint","input":{
            "schema_version":1,
            "payload":serde_json::to_value(match request { TeamControlRequest::Checkpoint(value) => value.payload, _ => unreachable!() }).unwrap(),
            "writer":"glados"
        }});
        assert_eq!(team_control_headless(&forged.to_string()).unwrap_err().code, "E_STATE_CONFLICT");
        assert_eq!(fs::read_dir(&checkpoint_dir).unwrap().count(), 1);

        let copied = token_path.with_file_name("t9-qa.token");
        fs::copy(&token_path, &copied).unwrap();
        fs::set_permissions(&copied, fs::Permissions::from_mode(0o600)).unwrap();
        std::env::set_var("APERTURE_HUB_TOKEN_FILE", &copied);
        assert_eq!(team_control_headless(&request_json).unwrap_err().code, "E_CONTROL_UNAUTHORIZED");

        std::env::set_var("APERTURE_HUB_TOKEN_FILE", &token_path);
        ensure_private_dir(&home.join(".aperture/run/revocations")).unwrap();
        write_private_json_atomic(&home.join(".aperture/run/revocations").join(format!("{}.json", seat.name)), &serde_json::json!({
            "schema_version":1, "seat":seat.name.clone(), "revoked_through_generation":1,
            "revoked_token_ids":[format!("{:x}", Sha256::digest(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"))]
        }), false).unwrap();
        assert_eq!(team_control_headless(&request_json).unwrap_err().code, "E_CONTROL_UNAUTHORIZED");
        assert_eq!(fs::read_dir(&checkpoint_dir).unwrap().count(), 1);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn lead_remote_control_derives_team_and_resolves_only_bound_target_inventory() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("lead-remote-control");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine
            .create_team(&AuthenticatedActor::operator_ui(), fullstack_input("t10"))
            .unwrap();
        engine
            .activate(
                &authenticate_glados_control().unwrap(),
                ActivateTeamInput {
                    team: "t10".into(),
                    expected_generation: 0,
                    creation_request_id: created.creation_request.request_id,
                    epic_id: "aperture-4rsnc".into(),
                },
            )
            .unwrap();
        let lead = created
            .team
            .snapshot
            .seats
            .iter()
            .find(|seat| seat.name == created.team.snapshot.lead)
            .unwrap();
        let target = created
            .team
            .snapshot
            .seats
            .iter()
            .find(|seat| seat.name != lead.name)
            .unwrap();
        let lead_token = bind_active_worker_with_token(
            &home,
            lead,
            b"llllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllll",
        );
        bind_active_worker_with_token(
            &home,
            target,
            b"tttttttttttttttttttttttttttttttttttttttttttttttttttttttttttt",
        );
        let harness = serde_json::to_value(&target.harness)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        crate::team_checkpoint::native::write_native(
            &home,
            &CheckpointContext {
                team: "t10".into(),
                seat: target.name.clone(),
                generation: 1,
                authenticated_generation: 1,
                harness,
                writer: CheckpointWriter::Explicit,
            },
            1,
            CheckpointPayload {
                task_id: "aperture-fixture".into(),
                worktree: "aperture-worktrees/aperture-fixture".into(),
                branch: "aperture-fixture".into(),
                head_sha: "a".repeat(40),
                dirty_files: vec![],
                open_pr: None,
                running_procs: vec![],
                decisions: vec![],
                next_step: "Wait for the approved remote effect decision.".into(),
                remote_effects: vec![crate::team_checkpoint::RemoteEffectRef {
                    kind: "ci".into(),
                    reference: "ci:fixture-1".into(),
                    state: "unknown".into(),
                }],
            },
            1,
            &[],
            || Ok(()),
        )
        .unwrap();
        std::env::set_var("APERTURE_HUB_TOKEN_FILE", &lead_token);

        let inspect = TeamControlRequest::InspectRemote(InspectRemoteInput {
            target_seat: target.name.clone(),
            expected_generation: 1,
        });
        let inventory = match team_control_headless(&serde_json::to_string(&inspect).unwrap()).unwrap() {
            TeamControlResponse::InspectRemote(value) => value,
            _ => panic!("unexpected control response"),
        };
        assert!(!inventory.complete_observation);
        assert_eq!(inventory.effects.len(), 1);
        assert_eq!(inventory.effects[0].reference, "ci:fixture-1");

        let resolve = TeamControlRequest::ResolveRemote(ResolveRemoteInput {
            target_seat: target.name.clone(),
            expected_generation: 1,
            resolution: ResolutionRequest {
                expected_inventory_hash: inventory.inventory_hash,
                scope: crate::team_replacement::remote::ResolutionScope::EffectResolution,
                reference: Some("ci:fixture-1".into()),
                decision: crate::team_replacement::remote::ResolutionDecision::Finished,
                evidence_ref: "beads:fixture-review".into(),
            },
        });
        let receipt = match team_control_headless(&serde_json::to_string(&resolve).unwrap()).unwrap() {
            TeamControlResponse::ResolveRemote(value) => value,
            _ => panic!("unexpected control response"),
        };
        assert_eq!(receipt.source, "authorized_decision");
        assert!(!receipt.complete_observation);
        assert!(!receipt.replay);

        let forged = serde_json::json!({
            "action": "inspect_remote",
            "input": {"target_seat": target.name, "expected_generation": 1, "team": "t10"}
        });
        assert_eq!(team_control_headless(&forged.to_string()).unwrap_err().code, "E_STATE_CONFLICT");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn activation_and_cancel_have_one_durable_winner() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let home = temp_root("race");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let created = engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("t5")).unwrap();
        let request_id = created.creation_request.request_id;
        let activation_actor = authenticate_glados_control().unwrap();
        let cancel_engine = engine.clone();
        let activate_engine = engine.clone();
        let barrier = Arc::new(Barrier::new(3));
        let activate_barrier = barrier.clone();
        let request_for_activate = request_id.clone();
        let activate = std::thread::spawn(move || {
            activate_barrier.wait();
            activate_engine.activate(&activation_actor, ActivateTeamInput { team:"t5".into(), expected_generation:0, creation_request_id:request_for_activate, epic_id:"aperture-4rsnc".into() })
        });
        let cancel_barrier = barrier.clone();
        let cancel = std::thread::spawn(move || {
            cancel_barrier.wait();
            cancel_engine.cancel_pending(&AuthenticatedActor::operator_ui(), CancelPendingInput { team:"t5".into(), expected_generation:0, creation_request_id:request_id })
        });
        barrier.wait();
        let activation_ok = activate.join().unwrap().is_ok();
        let cancel_ok = cancel.join().unwrap().is_ok();
        assert_ne!(activation_ok, cancel_ok);
        if activation_ok {
            assert_eq!(engine.read_team_view("t5").unwrap().state.state, TeamLifecycle::Active);
        } else {
            assert!(!home.join(".aperture/teams/t5").exists());
        }
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn runtime_command_contract_is_strict_and_does_not_expose_thread_identity() {
        let input = serde_json::from_value::<StartReplacementInput>(serde_json::json!({
            "team":"t1", "seat":"t1-backend", "expected_generation":1,
            "preparation_id":"00000000-0000-4000-8000-000000000001",
            "selection":{"harness":"codex","model":"gpt-6-astra","reasoning":"high"}
        })).unwrap();
        let selection = selection_from_tuple(&input.selection).unwrap();
        assert_eq!(selection.harness, "codex");
        assert_eq!(selection.reasoning.as_deref(), Some("high"));

        let forged = serde_json::json!({
            "team":"t1", "seat":"t1-backend", "expected_generation":1,
            "preparation_id":"00000000-0000-4000-8000-000000000001",
            "selection":{"harness":"codex","model":"gpt-6-astra","reasoning":"high"},
            "thread_id":"forged"
        });
        assert!(serde_json::from_value::<StartReplacementInput>(forged).is_err());

        let configured = input.selection.clone();
        let owner = OwnerSummary {
            generation: 1,
            state: crate::state::OwnerState::Active,
            since: "2026-09-20T00:00:00Z".into(),
            configured: configured.clone(),
            actual: Some(configured),
            process_count: 1,
            thread_bound: true,
        };
        let response = PreparedReplacementView {
            replacement: ReplacementView {
                team: "t1".into(),
                seat: "t1-backend".into(),
                generation: 1,
                phase: ReplacementPhase::Ready,
                checkpoint_recovery: CheckpointRecovery::Valid,
                checks: verified_replacement_checks(),
                owner: Some(owner),
                blockers: vec![],
            },
            preparation_id: Some("00000000-0000-4000-8000-000000000001".into()),
        };
        let value = serde_json::to_value(response).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 9);
        for key in ["team", "seat", "generation", "phase", "checkpoint_recovery", "checks", "owner", "blockers", "preparation_id"] {
            assert!(object.contains_key(key), "missing {key}");
        }
        assert!(value.to_string().find("thread_id").is_none());
    }
}
