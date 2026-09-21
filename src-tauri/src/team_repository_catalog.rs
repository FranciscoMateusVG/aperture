//! Runtime offers. Stored team bindings deliberately do not depend on this file.
use super::*;

const MAX_REPOSITORIES: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct RepositoryEntry {
    pub(super) project: String,
    pub(super) repo: String,
    pub(super) display_name: String,
    pub(super) enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Registry {
    schema_version: u32,
    pub(super) repositories: Vec<RepositoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SaveRepositoryInput {
    pub project: String,
    pub repo: String,
    pub display_name: String,
    pub enabled: bool,
    pub expected_sha256: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepositoryRegistryEntryView {
    project: String,
    repo: String,
    display_name: String,
    enabled: bool,
    available: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepositoryRegistryView {
    pub schema_version: u32,
    pub sha256: String,
    pub repositories: Vec<RepositoryRegistryEntryView>,
}

fn invalid() -> TeamError { TeamError::new("E_REPOSITORY_REGISTRY_INVALID", "repository registry is invalid") }

fn root(home: &Path) -> TeamResult<PathBuf> {
    let root = home.join(".aperture");
    ensure_private_dir(&root).map_err(TeamError::from_message)?;
    Ok(root)
}

pub(super) fn lock(home: &Path) -> TeamResult<AdvisoryLock> {
    let root = root(home)?;
    ensure_private_dir(&root.join("run")).map_err(TeamError::from_message)?;
    try_lock(&root.join("run/repository-locks"), "catalog").map_err(TeamError::from_message)
}

fn validate(registry: &mut Registry) -> TeamResult<()> {
    if registry.schema_version != 1 || registry.repositories.is_empty() || registry.repositories.len() > MAX_REPOSITORIES {
        return Err(invalid());
    }
    let mut keys = HashSet::new();
    for entry in &registry.repositories {
        if !repository_binding_is_wellformed(&entry.project, &entry.repo)
            || !keys.insert((&entry.project, &entry.repo)) { return Err(invalid()); }
        validate_text(&entry.display_name, MAX_DISPLAY_SCALARS, MAX_DISPLAY_BYTES, "repository display name")?;
    }
    registry.repositories.sort_by(|a,b| (&a.project, &a.repo).cmp(&(&b.project, &b.repo)));
    Ok(())
}

pub(super) fn read(home: &Path) -> TeamResult<Registry> {
    let path = root(home)?.join("repositories.json");
    // symlink_metadata distinguishes absence from dangling links and errors.
    let mut registry = match fs::symlink_metadata(&path) {
        Ok(_) => read_private_json(&path).map_err(TeamError::from_message)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Registry {
            schema_version: 1,
            repositories: REPOSITORY_SEEDS.iter().map(|(project,repo,display)| RepositoryEntry {
                project: (*project).into(), repo: (*repo).into(), display_name: (*display).into(), enabled: true,
            }).collect(),
        },
        Err(_) => return Err(invalid()),
    };
    validate(&mut registry)?;
    Ok(registry)
}

fn digest(registry: &Registry) -> TeamResult<String> {
    let mut bytes = b"aperture-repository-registry-v1\0".to_vec();
    bytes.extend(serde_json::to_vec(registry).map_err(|_| invalid())?);
    Ok(sha256(&bytes))
}

fn view(home: &Path, registry: Registry) -> TeamResult<RepositoryRegistryView> {
    Ok(RepositoryRegistryView {
        schema_version: 1, sha256: digest(&registry)?,
        repositories: registry.repositories.into_iter().map(|r| RepositoryRegistryEntryView {
            available: repository_is_available(home, &r.repo),
            project: r.project, repo: r.repo, display_name: r.display_name, enabled: r.enabled,
        }).collect(),
    })
}

pub(super) fn resolve_selected(home: &Path, project: &str, repo: &str) -> TeamResult<PathBuf> {
    if repo.is_empty() { return Err(TeamError::new("E_REPO_REQUIRED", "repository selection is required")); }
    if !repository_binding_is_wellformed(project, repo)
        || !read(home)?.repositories.iter().any(|r| r.project == project && r.repo == repo && r.enabled) {
        return Err(TeamError::new("E_REPO_NOT_IN_CATALOG", "repository is not enabled for the selected project"));
    }
    resolve_repository(home, project, repo)
}

impl TeamEngine {
    pub fn list_repositories(&self) -> TeamResult<RepositoryRegistryView> {
        view(&self.paths.home, read(&self.paths.home)?)
    }

    pub fn save_repository(&self, actor: &AuthenticatedActor, input: SaveRepositoryInput) -> TeamResult<RepositoryRegistryView> {
        if !actor.is_glados() { return Err(TeamError::new("E_CONTROL_UNAUTHORIZED", "authenticated glados context required")); }
        actor.revalidate_before_mutation().map_err(TeamError::from_message)?;
        let entry = RepositoryEntry { project: input.project, repo: input.repo, display_name: input.display_name, enabled: input.enabled };
        validate(&mut Registry { schema_version: 1, repositories: vec![entry.clone()] })?;
        if input.expected_sha256.len() != 64 || !input.expected_sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return Err(invalid()); }
        let _lock = lock(&self.paths.home)?;
        let mut registry = read(&self.paths.home)?;
        if digest(&registry)? != input.expected_sha256 {
            return Err(TeamError::new("E_REPOSITORY_CONFLICT", "repository registry changed; reread before editing"));
        }
        if entry.enabled { resolve_repository(&self.paths.home, &entry.project, &entry.repo)?; }
        match registry.repositories.iter_mut().find(|r| r.project == entry.project && r.repo == entry.repo) {
            Some(current) => *current = entry,
            None => registry.repositories.push(entry),
        }
        validate(&mut registry)?;
        actor.revalidate_before_mutation().map_err(TeamError::from_message)?;
        write_private_json_atomic(&self.paths.home.join(".aperture/repositories.json"), &registry, true).map_err(TeamError::from_message)?;
        let actual = read(&self.paths.home)?;
        if actual != registry { return Err(TeamError::state("repository publication could not be confirmed")); }
        view(&self.paths.home, actual)
    }
}
