//! F2-A metadata only. A valid identity is NOT protocol proof or permission to
//! adopt, spawn, signal, or unlink an endpoint. Incomplete history fails closed.
use crate::team_replacement::{ProcessIdentity, ProcessState};
use crate::{controller::ControllerLock, journal, team_process};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
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
fn valid_name(name: &str) -> bool {
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

pub(crate) struct Registry<'a> {
    lease: &'a ControllerLock,
    root: PathBuf,
}
struct History {
    reservations: BTreeMap<String, Reservation>,
    records: BTreeMap<String, Record>,
    current: Option<String>,
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
    fn checked_root(&self) -> Result<()> {
        self.lease.verify_live()?;
        let root = journal::validate_component_path(self.lease.run_dir()?, "daemons", false)
            .map_err(|_| fail("E_DAEMON_PATH"))?;
        journal::ensure_private_dir(&root).map_err(|_| fail("E_DAEMON_PATH"))
    }
    fn slot_path(&self, slot: &str, create: bool) -> Result<PathBuf> {
        self.checked_root()?;
        if !valid_slot(slot) {
            return Err(fail("E_DAEMON_PATH"));
        }
        let path = journal::validate_component_path(&self.root, slot, create)
            .map_err(|_| fail("E_DAEMON_PATH"))?;
        journal::ensure_private_dir(&path).map_err(|_| fail("E_DAEMON_PATH"))?;
        Ok(path)
    }
    fn history(&self, slot: &str) -> Result<History> {
        let path = self.slot_path(slot, false)?;
        let mut h = History {
            reservations: BTreeMap::new(),
            records: BTreeMap::new(),
            current: None,
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
            let file = journal::validate_component_path(&path, &name, false)
                .map_err(|_| fail("E_DAEMON_PATH"))?;
            if name == "current" {
                let id: String = read(&file)?;
                if !uuid(&id) {
                    return Err(fail("E_DAEMON_RECORD"));
                }
                h.current = Some(id);
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
    /// Account for both immutable facts and initial current BEFORE reserve or
    /// spawn. No history pruning/UUID rollover is a way around this limit.
    pub(crate) fn capacity(&self, slot: &str) -> Result<()> {
        self.inspect()?;
        let slots = fs::read_dir(&self.root)
            .map_err(|_| fail("E_DAEMON_PATH"))?
            .count();
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
        self.checked_root()?;
        let slot = endpoint.slot()?;
        provenance.validate()?;
        self.capacity(&slot)?;
        let path = self.root.join(&slot);
        let previous = match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(fail("E_DAEMON_PATH")),
            Ok(_) => {
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
