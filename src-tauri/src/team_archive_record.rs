//! Strict epic-bound reconciliation data. BEADS metadata is NOT authenticated
//! authority and a record/ref/hash never proves acceptance or metric truth.
//! Only the later native GLaDOS approval seam may authorize a verified digest.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub(crate) const TASK_CAP: usize = 256;
const RECORD_CAP: usize = 512 * 1024;
const OUTPUT_CAP: usize = 8 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordError {
    Missing,
    Shape,
    Limit,
    Binding,
    Reference,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveRecord {
    pub schema_version: u32,
    pub team: String,
    pub generation: u64,
    pub epic_id: String,
    pub items: Vec<RecordDisposition>,
    pub reviews: Vec<RecordReview>,
    pub metrics: Vec<RecordMetric>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RecordDisposition {
    Completed {
        task_id: String,
        evidence_refs: Vec<String>,
    },
    Cancelled {
        task_id: String,
        approval_ref: String,
    },
    Transferred {
        task_id: String,
        owner: String,
        task: String,
        approval_ref: String,
        acceptance_ref: String,
        history_ref: String,
    },
}
impl RecordDisposition {
    pub(crate) fn task_id(&self) -> &str {
        match self {
            Self::Completed { task_id, .. }
            | Self::Cancelled { task_id, .. }
            | Self::Transferred { task_id, .. } => task_id,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecordReview {
    pub task_id: String,
    pub reviewer: String,
    pub verdict: ReviewVerdict,
    pub at: String,
    pub evidence_ref: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewVerdict {
    Pass,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecordMetric {
    pub metric: String,
    pub observed_at: String,
    pub evidence_ref: String,
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 96
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn text(s: &str, cap: usize, sentinels: &[String]) -> bool {
    !s.is_empty()
        && s.len() <= cap
        && s.trim() == s
        && !s
            .chars()
            .any(|c| c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))
        && !s.contains("://")
        && !s.contains('@')
        && !s.contains('=')
        && !sentinels
            .iter()
            .filter(|v| !v.is_empty())
            .any(|v| s.contains(v))
}
fn reference(s: &str, sentinels: &[String]) -> bool {
    text(s, 256, sentinels)
        && s.split_once(':').is_some_and(|(kind, value)| {
            matches!(kind, "beads" | "artifact" | "git")
                && !value.is_empty()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._/-#".contains(&b))
                && !value.starts_with('/')
                && !value.split('/').any(|p| p == ".." || p.is_empty())
        })
}
fn time(s: &str, now: i64) -> bool {
    s.len() <= 40
        && chrono::DateTime::parse_from_rfc3339(s)
            .is_ok_and(|t| t.timestamp_millis() >= 0 && t.timestamp_millis() <= now)
}
impl ArchiveRecord {
    pub(crate) fn validate(
        &self,
        team: &str,
        generation: u64,
        epic: &str,
        now: i64,
        sentinels: &[String],
    ) -> Result<(), RecordError> {
        if self.schema_version != 1
            || self.team != team
            || self.generation != generation
            || self.epic_id != epic
            || generation == 0
            || !id(epic)
            || !crate::agent_loader::is_valid_seat_name(team)
            || team.len() > 16
        {
            return Err(RecordError::Binding);
        }
        if self.items.is_empty()
            || self.items.len() > TASK_CAP
            || self.reviews.is_empty()
            || self.reviews.len() > 64
            || self.metrics.is_empty()
            || self.metrics.len() > 64
        {
            return Err(RecordError::Limit);
        }
        let mut seen = HashSet::new();
        for item in &self.items {
            if !id(item.task_id()) || !seen.insert(item.task_id()) {
                return Err(RecordError::Shape);
            }
            let okay = match item {
                RecordDisposition::Completed { evidence_refs, .. } => {
                    !evidence_refs.is_empty()
                        && evidence_refs.len() <= 16
                        && evidence_refs.iter().all(|v| reference(v, sentinels))
                        && evidence_refs.iter().collect::<HashSet<_>>().len() == evidence_refs.len()
                }
                RecordDisposition::Cancelled { approval_ref, .. } => {
                    reference(approval_ref, sentinels)
                }
                RecordDisposition::Transferred {
                    owner,
                    task,
                    approval_ref,
                    acceptance_ref,
                    history_ref,
                    ..
                } => {
                    crate::agent_loader::is_valid_seat_name(owner)
                        && id(task)
                        && [approval_ref, acceptance_ref, history_ref]
                            .iter()
                            .all(|v| reference(v, sentinels))
                }
            };
            if !okay {
                return Err(RecordError::Reference);
            }
        }
        let mut reviews = HashSet::new();
        for r in &self.reviews {
            if !id(&r.task_id)
                || !reviews.insert(&r.task_id)
                || !crate::agent_loader::is_valid_seat_name(&r.reviewer)
                || !time(&r.at, now)
                || !reference(&r.evidence_ref, sentinels)
            {
                return Err(RecordError::Reference);
            }
        }
        let mut metrics = HashSet::new();
        for m in &self.metrics {
            if !text(&m.metric, 256, sentinels)
                || !metrics.insert(&m.metric)
                || !time(&m.observed_at, now)
                || !reference(&m.evidence_ref, sentinels)
            {
                return Err(RecordError::Reference);
            }
        }
        Ok(())
    }
    pub(crate) fn sha256(&self) -> Result<String, RecordError> {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(self).map_err(|_| RecordError::Shape)?;
        if bytes.len() > RECORD_CAP {
            return Err(RecordError::Limit);
        }
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}
/// Native `bd show --json` only. It is not a command DTO and creates no fact.
/// Never parse notes/prose or fall back to a record belonging to another epic.
pub(crate) fn parse_epic_record(
    raw: &[u8],
    team: &str,
    g: u64,
    epic: &str,
    now: i64,
    sentinels: &[String],
) -> Result<ArchiveRecord, RecordError> {
    if raw.len() > OUTPUT_CAP {
        return Err(RecordError::Limit);
    }
    let json: serde_json::Value = serde_json::from_slice(raw).map_err(|_| RecordError::Shape)?;
    let rows = json.as_array().ok_or(RecordError::Shape)?;
    if rows.len() != 1 || rows[0].get("id").and_then(|v| v.as_str()) != Some(epic) {
        return Err(RecordError::Binding);
    }
    let value = rows[0]
        .get("metadata")
        .and_then(|v| v.get("aperture_archive_v1"))
        .ok_or(RecordError::Missing)?;
    if serde_json::to_vec(value)
        .map_err(|_| RecordError::Shape)?
        .len()
        > RECORD_CAP
    {
        return Err(RecordError::Limit);
    }
    let record: ArchiveRecord =
        serde_json::from_value(value.clone()).map_err(|_| RecordError::Shape)?;
    record.validate(team, g, epic, now, sentinels)?;
    Ok(record)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> serde_json::Value {
        serde_json::json!([{"id":"aperture-epic","metadata":{"aperture_archive_v1":{
            "schema_version":1,"team":"t1","generation":1,"epic_id":"aperture-epic",
            "items":[{"kind":"completed","task_id":"aperture-work","evidence_refs":["artifact:receipt-sha"]}],
            "reviews":[{"task_id":"aperture-review","reviewer":"izzy","verdict":"pass","at":"2026-09-20T12:00:00Z","evidence_ref":"beads:aperture-review"}],
            "metrics":[{"metric":"mission outcome","observed_at":"2026-09-20T12:00:00Z","evidence_ref":"artifact:metric-sha"}]
        }}}])
    }
    fn parse(v: &serde_json::Value) -> Result<ArchiveRecord, RecordError> {
        parse_epic_record(
            &serde_json::to_vec(v).unwrap(),
            "t1",
            1,
            "aperture-epic",
            i64::MAX,
            &[],
        )
    }
    #[test]
    fn exact_record_is_deterministic_data_not_authority() {
        let a = parse(&fixture()).unwrap();
        let b = parse(&fixture()).unwrap();
        assert_eq!(a.sha256().unwrap(), b.sha256().unwrap());
        assert_eq!(a.items.len(), 1); // No approval, archive effect or metric truth is produced.
    }
    #[test]
    fn caller_boolean_unknown_field_or_prose_never_becomes_evidence() {
        for field in [
            "approved",
            "complete_observation",
            "acceptance_met",
            "actor",
        ] {
            let mut v = fixture();
            v[0]["metadata"]["aperture_archive_v1"][field] = true.into();
            assert!(parse(&v).is_err());
        }
        let mut v = fixture();
        v[0]["notes"] = v[0]["metadata"]["aperture_archive_v1"].to_string().into();
        v[0]["metadata"] = serde_json::json!({});
        assert!(matches!(parse(&v), Err(RecordError::Missing)));
    }
    #[test]
    fn wrong_epic_generation_duplicate_missing_and_future_fields_fail() {
        for mode in 0..5 {
            let mut v = fixture();
            let r = &mut v[0]["metadata"]["aperture_archive_v1"];
            match mode {
                0 => r["generation"] = 2.into(),
                1 => r["epic_id"] = "aperture-other".into(),
                2 => {
                    let x = r["items"][0].clone();
                    r["items"].as_array_mut().unwrap().push(x);
                }
                3 => r["reviews"] = serde_json::json!([]),
                _ => r["metrics"][0]["observed_at"] = "not-time".into(),
            };
            assert!(parse(&v).is_err());
        }
        assert!(parse_epic_record(
            &serde_json::to_vec(&fixture()).unwrap(),
            "t1",
            1,
            "aperture-epic",
            1,
            &[]
        )
        .is_err());
    }
    #[test]
    fn paths_urls_controls_and_sentinels_are_rejected_without_echo() {
        for value in [
            "artifact:../private",
            "artifact:/absolute",
            "https://host/token",
            "artifact:abc\nsecret",
            "artifact:user@host",
        ] {
            let mut v = fixture();
            v[0]["metadata"]["aperture_archive_v1"]["items"][0]["evidence_refs"] =
                serde_json::json!([value]);
            assert!(parse(&v).is_err());
        }
        assert!(parse_epic_record(
            &serde_json::to_vec(&fixture()).unwrap(),
            "t1",
            1,
            "aperture-epic",
            i64::MAX,
            &["mission outcome".into()]
        )
        .is_err());
    }
}
