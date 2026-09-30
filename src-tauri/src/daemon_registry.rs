//! F2-A metadata only. A valid identity is NOT protocol proof or permission to
//! adopt, spawn, signal, or unlink an endpoint. Incomplete history fails closed.
use crate::team_replacement::{ProcessIdentity, ProcessState};
use crate::{controller::ControllerLock, journal, team_process};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, String>;
const SCHEMA: u32 = 1;
const MAX_SLOTS: usize = 128;
const MAX_FACTS: usize = 512;
fn fail(code: &str) -> String {
    format!("{code}: daemon registry is not ready")
}
fn uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value && !id.is_nil())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Provenance {
    // A legacy process has no proven immutable release. Never infer this from
    // the new controller's build SHA, cwd, executable name or current pointer.
    LegacyUnknown,
    Release { release_sha: String },
}
impl Provenance {
    fn validate(&self) -> Result<()> {
        if let Self::Release { release_sha } = self {
            if release_sha.len() != 40
                || !release_sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(fail("E_DAEMON_RECORD"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub pid: u32,
    pub start_time_us: u64,
}
impl Identity {
    pub(crate) fn from_native(p: &ProcessIdentity) -> Result<Self> {
        let (sec, us) = p
            .start_time
            .split_once('.')
            .ok_or_else(|| fail("E_DAEMON_IDENTITY"))?;
        if sec.is_empty()
            || !sec.bytes().all(|v| v.is_ascii_digit())
            || us.len() != 6
            || !us.bytes().all(|v| v.is_ascii_digit())
        {
            return Err(fail("E_DAEMON_IDENTITY"));
        }
        let sec: u64 = sec.parse().map_err(|_| fail("E_DAEMON_IDENTITY"))?;
        let us: u64 = us.parse().map_err(|_| fail("E_DAEMON_IDENTITY"))?;
        let value = Self {
            pid: p.pid,
            start_time_us: sec
                .checked_mul(1_000_000)
                .and_then(|v| v.checked_add(us))
                .ok_or_else(|| fail("E_DAEMON_IDENTITY"))?,
        };
        value.validate()?;
        // Exact roundtrip, not an approximate elapsed-time/birth conversion.
        if value.native().start_time != p.start_time {
            return Err(fail("E_DAEMON_IDENTITY"));
        }
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        if self.pid <= 1 || self.pid > i32::MAX as u32 || self.start_time_us < 1_000_000 {
            return Err(fail("E_DAEMON_IDENTITY"));
        }
        Ok(())
    }
    pub(crate) fn native(self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.pid,
            start_time: format!(
                "{}.{:06}",
                self.start_time_us / 1_000_000,
                self.start_time_us % 1_000_000
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Endpoint {
    Hub { port: u16 }, // Always loopback; never stores a caller-controlled host.
    CodexAppServer { seat: String }, // Socket is derived from the controller run root.
}
impl Endpoint {
    fn slot(&self) -> Result<String> {
        match self {
            Self::Hub { port } if *port != 0 => Ok("hub".into()),
            Self::CodexAppServer { seat } if valid_name(seat) => Ok(format!("codex-{seat}")),
            _ => Err(fail("E_DAEMON_RECORD")),
        }
    }
}
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.ends_with('-')
}
fn valid_slot(name: &str) -> bool {
    name == "hub" || name.strip_prefix("codex-").is_some_and(valid_name)
}

/// Reservation is itself immutable. `previous` makes current's complete history
/// checkable: a new record written before current publication is NOT silently
/// mistaken for an old incarnation. No extra success marker or mutable ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reservation {
    schema_version: u32,
    incarnation: String,
    previous: Option<String>,
    endpoint: Endpoint,
    spawned_by: Identity,
    provenance: Provenance,
    reserved_at_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    schema_version: u32,
    reservation: Reservation,
    process: Identity,
    spawned_at_ms: u64,
}
impl Reservation {
    fn validate(&self, slot: &str) -> Result<()> {
        if self.schema_version != SCHEMA {
            return Err(fail("E_DAEMON_SCHEMA"));
        }
        if !uuid(&self.incarnation)
            || self
                .previous
                .as_deref()
                .is_some_and(|p| !uuid(p) || p == self.incarnation)
            || self.endpoint.slot()? != slot
            || self.reserved_at_ms == 0
        {
            return Err(fail("E_DAEMON_RECORD"));
        }
        self.spawned_by.validate()?;
        self.provenance.validate()
    }
}
impl Record {
    pub(crate) fn identity(&self) -> ProcessIdentity {
        self.process.native()
    }
    pub(crate) fn endpoint(&self) -> &Endpoint {
        &self.reservation.endpoint
    }

    fn validate(&self, slot: &str) -> Result<()> {
        if self.schema_version != SCHEMA {
            return Err(fail("E_DAEMON_SCHEMA"));
        }
        self.reservation.validate(slot)?;
        self.process.validate()?;
        if self.spawned_at_ms < self.reservation.reserved_at_ms {
            return Err(fail("E_DAEMON_RECORD"));
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Inspection {
    pub slot: String,
    pub incarnation: String,
    pub identity: ProcessState,
    pub provenance: Provenance,
    // Intentionally no `Adopted` state. B/C must supply authenticated protocol proof.
}

// C2a is metadata only. These claims are not syscall witnesses or capabilities.
// Explicit wire v2 has NO previous/successor field and cannot enroll v1 slots.
const CODEX_SCHEMA: u32 = 2;
const CODEX_PLAN_ENTRIES: usize = 9; // eight immutable facts plus current
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodePinV2 {
    pub dev: u64,
    pub ino: u64,
    pub uid: u32,
    pub mode: u32,
    pub links: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeTargetV2 {
    pub basename: String,
    pub parents: Vec<NodePinV2>,
    pub leaf: NodePinV2,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SocketPinsV2 {
    pub format_version: u32,
    // Order is the fixed root's ancestor order; no stored arbitrary path.
    // Future native conversion MUST compare every claim with existing pins.
    pub parents: Vec<NodePinV2>,
    pub entry: NodePinV2,
    pub native_target: Option<NativeTargetV2>,
}
impl SocketPinsV2 {
    fn validate(&self) -> Result<()> {
        fn kind(pin: &NodePinV2, expected: u32) -> bool {
            pin.ino != 0 && pin.mode & libc::S_IFMT as u32 == expected
        }
        fn parents(pins: &[NodePinV2]) -> bool {
            !pins.is_empty()
                && pins.len() <= 64
                && pins.iter().all(|p| kind(p, libc::S_IFDIR as u32))
        }
        if self.format_version != 1 || !parents(&self.parents) {
            return Err(fail("E_CODEX_PIN_METADATA"));
        }
        match &self.native_target {
            None if kind(&self.entry, libc::S_IFSOCK as u32) => {}
            Some(target)
                if kind(&self.entry, libc::S_IFLNK as u32)
                    && target.basename.len() == 64
                    && target
                        .basename
                        .bytes()
                        .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
                    && parents(&target.parents)
                    && kind(&target.leaf, libc::S_IFSOCK as u32) => {}
            _ => return Err(fail("E_CODEX_PIN_METADATA")),
        }
        // Structural metadata only; mode/uid/path policy conversion is C2b,
        // not an adoption/deletion proof manufactured by this serializer.
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TermResultV2 {
    ReturnedZero,
    Esrch,
    OtherError,
    Unobserved,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StopOutcomeV2 {
    TermSentThenDaemonGoneDescendantsUnverified,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CleanupOutcomeV2 {
    FixedDirectEntryRemoved,
    FixedLinkRemovedTargetRetained,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CodexEventV2 {
    SpawnIntent {
        endpoint: Endpoint,
        provenance: Provenance,
        budget_entries: usize,
    },
    Spawned {
        process: Identity,
    },
    SocketReady {
        pins: SocketPinsV2,
    },
    StopIntent {},
    TermResult {
        result: TermResultV2,
    },
    StopOutcome {
        outcome: StopOutcomeV2,
    },
    CleanupIntent {},
    CleanupOutcome {
        outcome: CleanupOutcomeV2,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexFactV2 {
    schema_version: u32,
    incarnation: String,
    sequence: usize,
    operation: String,
    authored_by: Identity,
    at_ms: u64,
    event: CodexEventV2,
}
impl CodexFactV2 {
    fn filename(&self) -> String {
        format!("v2-{:03}-{}.json", self.sequence, self.incarnation)
    }
    fn validate(&self) -> Result<()> {
        if self.schema_version != CODEX_SCHEMA {
            return Err(fail("E_DAEMON_SCHEMA"));
        }
        if !uuid(&self.incarnation)
            || !uuid(&self.operation)
            || self.sequence >= 8
            || self.at_ms == 0
        {
            return Err(fail("E_CODEX_METADATA"));
        }
        self.authored_by.validate()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CodexPhaseV2 {
    SpawnIntentUnknown,
    SpawnedUnready,
    PublicationIncomplete,
    ReadyMetadataOnly,
    StopIntentUnknown,
    TermResultUnresolved,
    Unknown,
    TermSentThenDaemonGoneDescendantsUnverified,
    CleanupIntentUnknown,
    FixedDirectEntryRemoved,
    FixedLinkRemovedTargetRetained,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CodexMetadataV2 {
    pub incarnation: String,
    pub process: Option<ProcessIdentity>,
    pub phase: CodexPhaseV2,
    pub entries: usize,
}
/// Value-only observation; opaque revision binds every validated fact/current
/// value, including operation/timestamps not exposed as a caller capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CodexSnapshot {
    pub incarnation: String,
    pub identity: Option<ProcessIdentity>,
    pub phase: CodexPhaseV2,
    pub provenance: Provenance,
    pub pins: Option<SocketPinsV2>,
    revision: [u8; 32],
}
fn budget(used: usize, additional: usize) -> Result<()> {
    if used.checked_add(additional).map_or(true, |n| n > MAX_FACTS) {
        Err(fail("E_DAEMON_CAPACITY"))
    } else {
        Ok(())
    }
}

pub(crate) struct Registry<'a> {
    lease: &'a ControllerLock,
    root: PathBuf,
}
struct History {
    reservations: BTreeMap<String, Reservation>,
    records: BTreeMap<String, Record>,
    current: Option<String>,
    codex: BTreeMap<usize, CodexFactV2>,
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    journal::read_private_json(path).map_err(|_| fail("E_DAEMON_RECORD"))
}
impl<'a> Registry<'a> {
    pub(crate) fn open(lease: &'a ControllerLock) -> Result<Self> {
        let root = journal::validate_component_path(lease.run_dir()?, "daemons", true)
            .map_err(|_| fail("E_DAEMON_PATH"))?;
        journal::ensure_private_dir(&root).map_err(|_| fail("E_DAEMON_PATH"))?;
        Ok(Self { lease, root })
    }
    /// Read-only projection of this registry's own verified root. Never expose
    /// ControllerLock: its shared reference carries capability-rotation and
    /// mutable child authority, which an observation consumer must not recover.
    pub(crate) fn verified_run_dir(&self) -> Result<&Path> {
        self.verify_read_context()?;
        self.lease.run_dir()
    }
    pub(crate) fn verify_read_context(&self) -> Result<()> {
        self.checked_root()
    }
    fn checked_root(&self) -> Result<()> {
        let expected = self.lease.run_dir()?.join("daemons");
        if expected != self.root {
            return Err(fail("E_DAEMON_PATH"));
        }
        crate::controller::private_dir_readonly(&self.root).map_err(|_| fail("E_DAEMON_PATH"))
    }
    fn slot_path(&self, slot: &str, create: bool) -> Result<PathBuf> {
        self.checked_root()?;
        if !valid_slot(slot) {
            return Err(fail("E_DAEMON_PATH"));
        }
        let path = self.root.join(slot);
        if create {
            // Explicit reserve writer only. Read paths below never repair dirs.
            journal::ensure_private_dir(&path).map_err(|_| fail("E_DAEMON_PATH"))?;
        } else {
            crate::controller::private_dir_readonly(&path).map_err(|_| fail("E_DAEMON_PATH"))?;
        }
        Ok(path)
    }
    fn history(&self, slot: &str) -> Result<History> {
        let path = self.slot_path(slot, false)?;
        let mut h = History {
            reservations: BTreeMap::new(),
            records: BTreeMap::new(),
            current: None,
            codex: BTreeMap::new(),
        };
        for (index, entry) in fs::read_dir(&path)
            .map_err(|_| fail("E_DAEMON_PATH"))?
            .enumerate()
        {
            if index >= MAX_FACTS {
                return Err(fail("E_DAEMON_RECORD"));
            }
            let entry = entry.map_err(|_| fail("E_DAEMON_PATH"))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| fail("E_DAEMON_PATH"))?;
            // read_dir supplies a single basename. No creating path validator
            // on a read: a concurrently missing parent must stay missing.
            let file = path.join(&name);
            let meta = fs::symlink_metadata(&file).map_err(|_| fail("E_DAEMON_PATH"))?;
            if meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
                return Err(fail("E_DAEMON_PATH"));
            }
            if name == "current" {
                let id: String = read(&file)?;
                if !uuid(&id) {
                    return Err(fail("E_DAEMON_RECORD"));
                }
                h.current = Some(id);
            } else if name.starts_with("v2-") {
                let fact: CodexFactV2 = read(&file)?;
                fact.validate()?;
                if fact.filename() != name || h.codex.insert(fact.sequence, fact).is_some() {
                    return Err(fail("E_CODEX_METADATA"));
                }
            } else if let Some(id) = name
                .strip_prefix("reservation-")
                .and_then(|s| s.strip_suffix(".json"))
            {
                let r: Reservation = read(&file)?;
                r.validate(slot)?;
                if id != r.incarnation {
                    return Err(fail("E_DAEMON_RECORD"));
                }
                h.reservations.insert(id.to_owned(), r);
            } else if let Some(id) = name.strip_suffix(".json") {
                let r: Record = read(&file)?;
                r.validate(slot)?;
                if id != r.reservation.incarnation {
                    return Err(fail("E_DAEMON_RECORD"));
                }
                h.records.insert(id.to_owned(), r);
            } else {
                // Includes abandoned atomic-write temps: not proof of absence.
                return Err(fail("E_DAEMON_RECORD"));
            }
        }
        Ok(h)
    }
    fn complete<'h>(&self, h: &'h History) -> Result<&'h Record> {
        if !h.codex.is_empty() {
            return Err(fail("E_DAEMON_SCHEMA"));
        }
        if h.reservations.is_empty() || h.records.len() != h.reservations.len() {
            return Err(fail("E_DAEMON_RESERVATION_INCOMPLETE"));
        }
        for (id, reservation) in &h.reservations {
            if h.records.get(id).map(|r| &r.reservation) != Some(reservation) {
                return Err(fail("E_DAEMON_RESERVATION_INCOMPLETE"));
            }
        }
        let current = h
            .current
            .as_ref()
            .ok_or_else(|| fail("E_DAEMON_PUBLICATION_INCOMPLETE"))?;
        let mut seen = BTreeSet::new();
        let mut at = Some(current.as_str());
        while let Some(id) = at {
            if !seen.insert(id) {
                return Err(fail("E_DAEMON_RECORD"));
            }
            let r = h
                .records
                .get(id)
                .ok_or_else(|| fail("E_DAEMON_PUBLICATION_INCOMPLETE"))?;
            at = r.reservation.previous.as_deref();
        }
        if seen.len() != h.records.len() {
            return Err(fail("E_DAEMON_PUBLICATION_INCOMPLETE"));
        }
        h.records
            .get(current)
            .ok_or_else(|| fail("E_DAEMON_PUBLICATION_INCOMPLETE"))
    }
    /// Namespace validity is not adoption eligibility. Retain/count structurally
    /// valid v2 prefixes, Unknown and terminal histories without granting them
    /// readiness, reuse or successor authority. V1 incompleteness still denies.
    fn namespace_slots(&self) -> Result<usize> {
        self.checked_root()?;
        let mut count = 0;
        for entry in fs::read_dir(&self.root).map_err(|_| fail("E_DAEMON_PATH"))? {
            if count >= MAX_SLOTS {
                return Err(fail("E_DAEMON_RECORD"));
            }
            let slot = entry
                .map_err(|_| fail("E_DAEMON_PATH"))?
                .file_name()
                .into_string()
                .map_err(|_| fail("E_DAEMON_PATH"))?;
            let h = self.history(&slot)?;
            if h.codex.is_empty() {
                self.complete(&h)?;
            } else {
                self.codex_view(&slot, &h)?;
            }
            count += 1;
        }
        Ok(count)
    }
    /// Empty means only "no registry facts", NEVER "the endpoint is unoccupied".
    pub(crate) fn inspect(&self) -> Result<Vec<Inspection>> {
        self.inspect_with(team_process::state)
    }
    fn inspect_with(
        &self,
        observe: impl Fn(&ProcessIdentity) -> ProcessState,
    ) -> Result<Vec<Inspection>> {
        self.checked_root()?;
        let mut out = Vec::new();
        for (index, entry) in fs::read_dir(&self.root)
            .map_err(|_| fail("E_DAEMON_PATH"))?
            .enumerate()
        {
            if index >= MAX_SLOTS {
                return Err(fail("E_DAEMON_RECORD"));
            }
            let slot = entry
                .map_err(|_| fail("E_DAEMON_PATH"))?
                .file_name()
                .into_string()
                .map_err(|_| fail("E_DAEMON_PATH"))?;
            let h = self.history(&slot)?;
            if !h.codex.is_empty() {
                let view = self.codex_view(&slot, &h)?;
                if view.phase != CodexPhaseV2::ReadyMetadataOnly {
                    return Err(fail("E_CODEX_OPERATION_UNKNOWN"));
                }
                let provenance = match &h.codex[&0].event {
                    CodexEventV2::SpawnIntent { provenance, .. } => provenance.clone(),
                    _ => return Err(fail("E_CODEX_METADATA")),
                };
                out.push(Inspection {
                    slot,
                    incarnation: view.incarnation,
                    identity: observe(&view.process.ok_or_else(|| fail("E_CODEX_METADATA"))?),
                    provenance,
                });
                continue;
            }
            let r = self.complete(&h)?;
            out.push(Inspection {
                slot,
                incarnation: r.reservation.incarnation.clone(),
                identity: observe(&r.process.native()),
                provenance: r.reservation.provenance.clone(),
            });
        }
        out.sort_by(|a, b| a.slot.cmp(&b.slot));
        Ok(out)
    }
    /// Complete current metadata only; never protocol/adoption authority.
    pub(crate) fn current(&self, slot: &str) -> Result<Option<Record>> {
        self.checked_root()?;
        if !valid_slot(slot) {
            return Err(fail("E_DAEMON_PATH"));
        }
        match fs::symlink_metadata(self.root.join(slot)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(fail("E_DAEMON_PATH")),
            Ok(_) => Ok(Some(self.complete(&self.history(slot)?)?.clone())),
        }
    }
    fn codex_view(&self, slot: &str, h: &History) -> Result<CodexMetadataV2> {
        use CodexEventV2 as E;
        use CodexPhaseV2 as P;
        if !h.reservations.is_empty() || !h.records.is_empty() || h.codex.is_empty() {
            return Err(fail("E_DAEMON_SCHEMA"));
        }
        let first = h.codex.get(&0).ok_or_else(|| fail("E_CODEX_METADATA"))?;
        let (endpoint, provenance) = match &first.event {
            E::SpawnIntent {
                endpoint: Endpoint::CodexAppServer { .. },
                ..
            } => match &first.event {
                E::SpawnIntent {
                    endpoint,
                    provenance,
                    budget_entries,
                } if *budget_entries == CODEX_PLAN_ENTRIES => (endpoint, provenance),
                _ => return Err(fail("E_CODEX_METADATA")),
            },
            _ => return Err(fail("E_CODEX_METADATA")),
        };
        if endpoint.slot()? != slot {
            return Err(fail("E_CODEX_METADATA"));
        }
        provenance.validate()?;
        let mut time = 0;
        let mut process = None;
        let mut phase = P::SpawnIntentUnknown;
        let mut target_retained = false;
        for (index, (sequence, fact)) in h.codex.iter().enumerate() {
            fact.validate()?;
            if *sequence != index || fact.incarnation != first.incarnation || fact.at_ms < time {
                return Err(fail("E_CODEX_METADATA"));
            }
            time = fact.at_ms;
            // Continuations of an effect intent must belong to the same held
            // operation; reacquiring a slot does not resume unknown effects.
            let intent_index = match index {
                1 | 2 => Some(0),
                4 | 5 => Some(3),
                7 => Some(6),
                _ => None,
            };
            if let Some(i) = intent_index {
                let intent = h.codex.get(&i).ok_or_else(|| fail("E_CODEX_METADATA"))?;
                if fact.operation != intent.operation || fact.authored_by != intent.authored_by {
                    return Err(fail("E_CODEX_OPERATION_UNKNOWN"));
                }
            }
            match (index, &fact.event) {
                (0, E::SpawnIntent { .. }) => {}
                (1, E::Spawned { process: p }) => {
                    p.validate()?;
                    process = Some(p.native());
                    phase = P::SpawnedUnready;
                }
                (2, E::SocketReady { pins }) => {
                    pins.validate()?;
                    target_retained = pins.native_target.is_some();
                    phase = P::PublicationIncomplete;
                }
                (3, E::StopIntent {}) => phase = P::StopIntentUnknown,
                (4, E::TermResult { result }) => {
                    phase = if *result == TermResultV2::ReturnedZero {
                        P::TermResultUnresolved
                    } else {
                        P::Unknown
                    }
                }
                (
                    5,
                    E::StopOutcome {
                        outcome: StopOutcomeV2::Unknown,
                    },
                ) => phase = P::Unknown,
                (
                    5,
                    E::StopOutcome {
                        outcome: StopOutcomeV2::TermSentThenDaemonGoneDescendantsUnverified,
                    },
                ) if phase == P::TermResultUnresolved => {
                    phase = P::TermSentThenDaemonGoneDescendantsUnverified
                }
                (6, E::CleanupIntent {})
                    if phase == P::TermSentThenDaemonGoneDescendantsUnverified =>
                {
                    phase = P::CleanupIntentUnknown
                }
                (
                    7,
                    E::CleanupOutcome {
                        outcome: CleanupOutcomeV2::Unknown,
                    },
                ) => phase = P::Unknown,
                (
                    7,
                    E::CleanupOutcome {
                        outcome: CleanupOutcomeV2::FixedDirectEntryRemoved,
                    },
                ) if !target_retained => phase = P::FixedDirectEntryRemoved,
                (
                    7,
                    E::CleanupOutcome {
                        outcome: CleanupOutcomeV2::FixedLinkRemovedTargetRetained,
                    },
                ) if target_retained => phase = P::FixedLinkRemovedTargetRetained,
                _ => return Err(fail("E_CODEX_METADATA")),
            }
        }
        if let Some(id) = &h.current {
            if id != &first.incarnation || h.codex.len() < 3 {
                return Err(fail("E_DAEMON_PUBLICATION_INCOMPLETE"));
            }
            if h.codex.len() == 3 {
                phase = P::ReadyMetadataOnly;
            }
        } else if h.codex.len() > 3 {
            return Err(fail("E_DAEMON_PUBLICATION_INCOMPLETE"));
        }
        let entries = h.codex.len() + usize::from(h.current.is_some());
        budget(entries, 0)?;
        Ok(CodexMetadataV2 {
            incarnation: first.incarnation.clone(),
            process,
            phase,
            entries,
        })
    }
    /// Read metadata, not permission to adopt, replace, signal or unlink.
    pub(crate) fn codex_metadata(&self, seat: &str) -> Result<Option<CodexMetadataV2>> {
        self.checked_root()?;
        let slot = Endpoint::CodexAppServer { seat: seat.into() }.slot()?;
        match fs::symlink_metadata(self.root.join(&slot)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(fail("E_DAEMON_PATH")),
            Ok(_) => self.codex_view(&slot, &self.history(&slot)?).map(Some),
        }
    }
    pub(crate) fn validate_namespace(&self) -> Result<usize> {
        self.namespace_slots()
    }
    /// Verifies a separately acquired operation; never returns its lease/guard.
    pub(crate) fn verify_operation(
        &self,
        operation: &crate::controller::CodexOperation<'_>,
    ) -> Result<()> {
        operation.verify_for(self.lease)?;
        self.checked_root()
    }
    pub(crate) fn codex_snapshot(&self, seat: &str) -> Result<Option<CodexSnapshot>> {
        use sha2::{Digest, Sha256};
        let slot = Endpoint::CodexAppServer { seat: seat.into() }.slot()?;
        self.checked_root()?;
        match fs::symlink_metadata(self.root.join(&slot)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(fail("E_DAEMON_PATH")),
            Ok(_) => {}
        }
        let h = self.history(&slot)?;
        let view = self.codex_view(&slot, &h)?;
        let provenance = match &h.codex[&0].event {
            CodexEventV2::SpawnIntent { provenance, .. } => provenance.clone(),
            _ => return Err(fail("E_CODEX_METADATA")),
        };
        let pins = h.codex.get(&2).and_then(|f| match &f.event {
            CodexEventV2::SocketReady { pins } => Some(pins.clone()),
            _ => None,
        });
        let bytes =
            serde_json::to_vec(&(&h.codex, &h.current)).map_err(|_| fail("E_CODEX_METADATA"))?;
        Ok(Some(CodexSnapshot {
            incarnation: view.incarnation,
            identity: view.process,
            phase: view.phase,
            provenance,
            pins,
            revision: Sha256::digest(bytes).into(),
        }))
    }
    pub(crate) fn recheck_snapshot(&self, seat: &str, expected: &CodexSnapshot) -> Result<()> {
        if self.codex_snapshot(seat)?.as_ref() != Some(expected) {
            return Err(fail("E_CODEX_REVISION_CHANGED"));
        }
        self.verify_read_context()
    }
    /// Keep the complete incarnation history; only a verified coordinator
    /// closure can free the canonical slot for a new explicit Start.
    pub(crate) fn retire_closed_codex(&self,op:&crate::controller::CodexOperation<'_>,
        before:&CodexSnapshot,closed:&crate::agents::coordinator_lifecycle::Closed)->Result<()> {
        let _admission=self.lease.registry_admission()?;
        self.verify_operation(op)?;self.recheck_snapshot(op.seat(),before)?;
        if before.phase!=CodexPhaseV2::ReadyMetadataOnly{return Err(fail("E_CODEX_OPERATION_UNKNOWN"));}
        closed.verifies(op.seat(),before.identity.as_ref().ok_or_else(||fail("E_CODEX_METADATA"))?)?;
        crate::team_terminal::codex_pristine_endpoint(self,op)?;
        let retired=self.lease.run_dir()?.join("coordinator-retired");
        journal::ensure_private_dir(&retired)?;
        if fs::read_dir(&retired).map_err(|_|fail("E_DAEMON_PATH"))?.take(4097).count()>=4096{return Err(fail("E_DAEMON_CAPACITY"));}
        let slot=Endpoint::CodexAppServer{seat:op.seat().into()}.slot()?;
        let dest=retired.join(format!("{}-{}",op.seat(),before.incarnation));
        self.recheck_snapshot(op.seat(),before)?;closed.recheck()?;
        journal::rename_no_replace(&self.root.join(slot),&dest)?;
        journal::sync_dir(&self.root)?;journal::sync_dir(&retired)?;closed.recheck()
    }
    /// Metadata intent only. First C2a writer: one pristine incarnation forever.
    /// No external-effect callback exists; full future operation budget is paid
    /// before this intent, including current and terminal outcome files.
    pub(crate) fn begin_codex_v2(
        &self,
        operation: &crate::controller::CodexOperation<'_>,
        provenance: Provenance,
        at_ms: u64,
    ) -> Result<String> {
        let _admission = self.lease.registry_admission()?;
        operation.verify_for(self.lease)?;
        self.checked_root()?;
        let endpoint = Endpoint::CodexAppServer {
            seat: operation.seat().into(),
        };
        let slot = endpoint.slot()?;
        match fs::symlink_metadata(self.root.join(&slot)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(fail("E_CODEX_NOT_PRISTINE")),
        }
        let slots = self.namespace_slots()?;
        if slots >= MAX_SLOTS {
            return Err(fail("E_DAEMON_CAPACITY"));
        }
        budget(0, CODEX_PLAN_ENTRIES)?;
        #[cfg(test)]
        self.lease.admission_after_capacity();
        provenance.validate()?;
        let fact = CodexFactV2 {
            schema_version: CODEX_SCHEMA,
            incarnation: Uuid::new_v4().to_string(),
            sequence: 0,
            operation: operation.nonce().into(),
            authored_by: Identity::from_native(self.lease.identity()?)?,
            at_ms,
            event: CodexEventV2::SpawnIntent {
                endpoint,
                provenance,
                budget_entries: CODEX_PLAN_ENTRIES,
            },
        };
        fact.validate()?;
        let dir = self.slot_path(&slot, true)?;
        operation.verify_for(self.lease)?;
        journal::write_private_json_atomic(&dir.join(fact.filename()), &fact, false)?;
        Ok(fact.incarnation)
    }
    pub(crate) fn append_codex_v2(
        &self,
        operation: &crate::controller::CodexOperation<'_>,
        incarnation: &str,
        event: CodexEventV2,
        at_ms: u64,
    ) -> Result<()> {
        operation.verify_for(self.lease)?;
        let slot = Endpoint::CodexAppServer {
            seat: operation.seat().into(),
        }
        .slot()?;
        let mut h = self.history(&slot)?;
        let before = self.codex_view(&slot, &h)?;
        if before.incarnation != incarnation {
            return Err(fail("E_CODEX_OPERATION_UNKNOWN"));
        }
        // No successor, replay, new-UUID retry or post-Unknown reconciliation.
        // Only completing the already-held syscall fact with Unknown is allowed.
        if before.phase == CodexPhaseV2::Unknown
            && !(h.codex.len() == 5
                && matches!(
                    event,
                    CodexEventV2::StopOutcome {
                        outcome: StopOutcomeV2::Unknown
                    }
                ))
        {
            return Err(fail("E_CODEX_OPERATION_UNKNOWN"));
        }
        budget(
            before.entries,
            CODEX_PLAN_ENTRIES.saturating_sub(before.entries),
        )?;
        let fact = CodexFactV2 {
            schema_version: CODEX_SCHEMA,
            incarnation: incarnation.into(),
            sequence: h.codex.len(),
            operation: operation.nonce().into(),
            authored_by: Identity::from_native(self.lease.identity()?)?,
            at_ms,
            event,
        };
        h.codex.insert(fact.sequence, fact.clone());
        self.codex_view(&slot, &h)?;
        let dir = self.slot_path(&slot, false)?;
        operation.verify_for(self.lease)?;
        journal::write_private_json_atomic(&dir.join(fact.filename()), &fact, false)
    }
    pub(crate) fn publish_codex_v2(
        &self,
        operation: &crate::controller::CodexOperation<'_>,
        incarnation: &str,
    ) -> Result<()> {
        operation.verify_for(self.lease)?;
        let slot = Endpoint::CodexAppServer {
            seat: operation.seat().into(),
        }
        .slot()?;
        let mut h = self.history(&slot)?;
        let view = self.codex_view(&slot, &h)?;
        let intent = h.codex.get(&0).ok_or_else(|| fail("E_CODEX_METADATA"))?;
        if view.incarnation != incarnation
            || view.phase != CodexPhaseV2::PublicationIncomplete
            || intent.operation != operation.nonce()
            || intent.authored_by != Identity::from_native(self.lease.identity()?)?
        {
            return Err(fail("E_CODEX_OPERATION_UNKNOWN"));
        }
        h.current = Some(incarnation.into());
        self.codex_view(&slot, &h)?;
        let dir = self.slot_path(&slot, false)?;
        operation.verify_for(self.lease)?;
        journal::write_private_json_atomic(&dir.join("current"), &incarnation, false)
    }

    /// Account for both immutable facts and initial current BEFORE reserve or
    /// spawn. No history pruning/UUID rollover is a way around this limit.
    /// Observation only: writers must hold admission and revalidate this budget.
    pub(crate) fn capacity(&self, slot: &str) -> Result<()> {
        let slots = self.namespace_slots()?;
        let facts = match fs::symlink_metadata(self.root.join(slot)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if slots >= MAX_SLOTS {
                    return Err(fail("E_DAEMON_CAPACITY"));
                }
                0
            }
            Err(_) => return Err(fail("E_DAEMON_PATH")),
            Ok(_) => {
                let h = self.history(slot)?;
                self.complete(&h)?;
                h.reservations.len() + h.records.len() + 1
            }
        };
        if facts + if facts == 0 { 3 } else { 2 } > MAX_FACTS {
            return Err(fail("E_DAEMON_CAPACITY"));
        }
        Ok(())
    }
    /// Metadata mutation only, not a spawn permission. An unresolved previous
    /// reservation blocks a new UUID, even if its endpoint happens to be free.
    pub(crate) fn reserve(
        &self,
        endpoint: Endpoint,
        provenance: Provenance,
        at_ms: u64,
    ) -> Result<Reservation> {
        let _admission = self.lease.registry_admission()?;
        self.checked_root()?;
        let slot = endpoint.slot()?;
        provenance.validate()?;
        self.capacity(&slot)?;
        #[cfg(test)]
        self.lease.admission_after_capacity();
        let path = self.root.join(&slot);
        let previous = match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(fail("E_DAEMON_PATH")),
            Ok(_) => {
                if matches!(endpoint, Endpoint::CodexAppServer { .. }) {
                    return Err(fail("E_CODEX_NO_SUCCESSOR"));
                }
                let h = self.history(&slot)?;
                let r = self.complete(&h)?;
                if team_process::state(&r.process.native()) != ProcessState::Gone {
                    return Err(fail("E_DAEMON_IDENTITY_UNVERIFIED"));
                }
                Some(r.reservation.incarnation.clone())
            }
        };
        let reservation = Reservation {
            schema_version: SCHEMA,
            incarnation: Uuid::new_v4().to_string(),
            previous,
            endpoint,
            spawned_by: Identity::from_native(self.lease.identity()?)?,
            provenance,
            reserved_at_ms: at_ms,
        };
        reservation.validate(&slot)?;
        let dir = self.slot_path(&slot, true)?;
        self.lease.verify_live()?;
        journal::write_private_json_atomic(
            &dir.join(format!("reservation-{}.json", reservation.incarnation)),
            &reservation,
            false,
        )?;
        Ok(reservation)
    }
    pub(crate) fn record(
        &self,
        reservation: &Reservation,
        process: &ProcessIdentity,
        at_ms: u64,
    ) -> Result<Record> {
        let slot = reservation.endpoint.slot()?;
        let dir = self.slot_path(&slot, false)?;
        let h = self.history(&slot)?;
        if h.reservations.get(&reservation.incarnation) != Some(reservation)
            || h.current != reservation.previous
            || reservation.spawned_by != Identity::from_native(self.lease.identity()?)?
        {
            return Err(fail("E_DAEMON_RECORD"));
        }
        if team_process::state(process) != ProcessState::Same {
            return Err(fail("E_DAEMON_IDENTITY_UNVERIFIED"));
        }
        let r = Record {
            schema_version: SCHEMA,
            reservation: reservation.clone(),
            process: Identity::from_native(process)?,
            spawned_at_ms: at_ms,
        };
        r.validate(&slot)?;
        self.lease.verify_live()?;
        journal::write_private_json_atomic(
            &dir.join(format!("{}.json", reservation.incarnation)),
            &r,
            false,
        )?;
        Ok(r)
    }
    pub(crate) fn publish_current(&self, record: &Record) -> Result<()> {
        let slot = record.reservation.endpoint.slot()?;
        let dir = self.slot_path(&slot, false)?;
        let h = self.history(&slot)?;
        let id = &record.reservation.incarnation;
        if h.records.get(id) != Some(record)
            || h.reservations.get(id) != Some(&record.reservation)
            || h.current != record.reservation.previous
            || record.reservation.spawned_by != Identity::from_native(self.lease.identity()?)?
        {
            return Err(fail("E_DAEMON_RECORD"));
        }
        let candidate = History {
            current: Some(id.clone()),
            ..h
        };
        self.complete(&candidate)?;
        self.lease.verify_live()?;
        journal::write_private_json_atomic(
            &dir.join("current"),
            id,
            record.reservation.previous.is_some(),
        )
    }
}

#[cfg(test)]
#[path = "daemon_registry_tests.rs"]
mod tests;
