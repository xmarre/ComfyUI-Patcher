use crate::errors::AppResult;
use crate::git::canonicalize_remote;
use crate::models::{ResolvedTarget, TargetKind};
use crate::util::slugify;
use regex::Regex;
use serde::Deserialize;
use std::{borrow::Cow, collections::HashMap, sync::Arc};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const MANAGER_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/Comfy-Org/ComfyUI-Manager/refs/heads/main/custom-node-list.json";
const REGISTRY_TTL: Duration = Duration::from_secs(60 * 30);

#[derive(Clone)]
pub struct ManagerRegistryClient {
    client: reqwest::Client,
    cache: Arc<Mutex<Option<RegistryCache>>>,
    remote_alias_cache: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

#[derive(Clone)]
struct RegistryCache {
    fetched_at: Instant,
    entries: Vec<ManagerCustomNodeEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManagerRegistryPayload {
    custom_nodes: Vec<ManagerCustomNodeEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ManagerCustomNodeEntry {
    id: Option<String>,
    title: Option<String>,
    author: Option<String>,
    description: Option<String>,
    reference: Option<String>,
    files: Option<Vec<String>>,
    install_type: Option<String>,
}

impl ManagerRegistryClient {
    pub fn new() -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            cache: Arc::new(Mutex::new(None)),
            remote_alias_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn entries(&self) -> AppResult<Vec<ManagerCustomNodeEntry>> {
        let mut cache = self.cache.lock().await;
        if let Some(existing) = cache.as_ref() {
            if existing.fetched_at.elapsed() < REGISTRY_TTL {
                return Ok(existing.entries.clone());
            }
        }

        let payload: ManagerRegistryPayload = self
            .client
            .get(MANAGER_REGISTRY_URL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let entries = payload.custom_nodes;
        *cache = Some(RegistryCache {
            fetched_at: Instant::now(),
            entries: entries.clone(),
        });
        Ok(entries)
    }

    pub async fn search_entries(
        &self,
        query: Option<&str>,
        limit: usize,
    ) -> AppResult<Vec<ManagerCustomNodeEntry>> {
        let mut entries = self.entries().await?;
        if let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) {
            let query = query.to_ascii_lowercase();
            entries.retain(|entry| entry.matches_query(&query));
        }
        entries.sort_by_key(|entry| entry.sort_key().to_ascii_lowercase());
        entries.truncate(limit);
        Ok(entries)
    }

    pub async fn remote_aliases(&self, canonical_remote: &str) -> Vec<String> {
        let canonical_remote = match canonicalize_remote(canonical_remote) {
            Some(value) => value,
            None => return Vec::new(),
        };

        if let Some(cached) = self
            .remote_alias_cache
            .lock()
            .await
            .get(&canonical_remote)
            .cloned()
        {
            return cached;
        }

        let mut aliases = vec![canonical_remote.clone()];
        if let Ok(response) = self.client.get(&canonical_remote).send().await {
            if let Some(final_remote) = canonicalize_remote(response.url().as_str()) {
                if !aliases.iter().any(|value| value == &final_remote) {
                    aliases.push(final_remote);
                }
            }
        }

        self.remote_alias_cache
            .lock()
            .await
            .insert(canonical_remote, aliases.clone());
        aliases
    }

    pub fn expected_dir_names_for_entry(
        &self,
        entry: &ManagerCustomNodeEntry,
    ) -> Vec<String> {
        fn push_candidate(out: &mut Vec<String>, raw: &str) {
            let slug = slugify(raw);
            if slug.is_empty() {
                return;
            }
            if !out.iter().any(|value| value == &slug) {
                out.push(slug.clone());
            }
            for prefix in ["comfyui-", "comfyui_", "comfyui"] {
                if let Some(stripped) = slug.strip_prefix(prefix) {
                    let stripped = stripped.trim_start_matches(['-', '_']);
                    if !stripped.is_empty() && !out.iter().any(|value| value == stripped) {
                        out.push(stripped.to_string());
                    }
                }
            }
        }

        let mut candidates = Vec::new();

        if let Some(id) = entry.id.as_deref() {
            push_candidate(&mut candidates, id);
        }

        if let Some(title) = entry.title.as_deref() {
            push_candidate(&mut candidates, title);
        }

        if let Some(reference) = entry.canonical_git_remote() {
            if let Some(repo_name) = repo_name_from_url(&reference) {
                push_candidate(&mut candidates, &repo_name);
            }
        }

        candidates
    }

    pub async fn preferred_dir_name_for_target(
        &self,
        resolved: &ResolvedTarget,
    ) -> AppResult<Option<String>> {
        let canonical_target = canonicalize_remote(&resolved.canonical_repo_url)
            .unwrap_or_else(|| resolved.canonical_repo_url.clone());
        let entries = self.entries().await?;
        let entry = entries
            .into_iter()
            .find(|entry| entry.matches_remote(&canonical_target));
        let Some(entry) = entry else {
            return Ok(None);
        };

        if let Some(pyproject_name) = self.pyproject_name_for_target(resolved).await? {
            return Ok(Some(pyproject_name));
        }

        if let Some(id) = entry.id.as_deref() {
            let normalized = slugify(id);
            if !normalized.is_empty() {
                return Ok(Some(normalized));
            }
        }

        Ok(repo_name_from_url(&canonical_target).map(|name| slugify(&name)))
    }

    async fn pyproject_name_for_target(&self, resolved: &ResolvedTarget) -> AppResult<Option<String>> {
        let Some((owner, repo, git_ref)) = repo_ref_for_target(resolved) else {
            return Ok(None);
        };
        let url = format!(
            "https://raw.githubusercontent.com/{owner}/{repo}/{git_ref}/pyproject.toml"
        );
        let response = self.client.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let text = response.error_for_status()?.text().await?;
        Ok(parse_pyproject_name(&text)
            .map(|name| slugify(&name))
            .filter(|name| !name.is_empty()))
    }
}

impl ManagerCustomNodeEntry {
    pub(crate) fn registry_id(&self) -> String {
        self.id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| self.title.as_deref().map(slugify).filter(|value| !value.is_empty()))
            .or_else(|| {
                self.canonical_git_remote().and_then(|value| {
                    repo_name_from_url(&value)
                        .map(|name| slugify(&name))
                        .filter(|value| !value.is_empty())
                })
            })
            .unwrap_or_else(|| "custom-node".to_string())
    }

    pub(crate) fn title(&self) -> String {
        self.title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.registry_id())
    }

    pub(crate) fn author(&self) -> Option<String> {
        self.author
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    pub(crate) fn description(&self) -> Option<String> {
        self.description
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    pub(crate) fn install_type_label(&self) -> String {
        self.install_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub(crate) fn source_input(&self) -> Option<String> {
        self.reference
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.files.as_ref().and_then(|files| {
                    files
                        .iter()
                        .map(|value| value.trim())
                        .find(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                })
            })
    }

    pub(crate) fn canonical_git_remote(&self) -> Option<String> {
        if self.install_type.as_deref() != Some("git-clone") {
            return None;
        }
        self.reference
            .iter()
            .chain(self.files.iter().flatten())
            .filter_map(|value| canonicalize_remote(value))
            .next()
    }

    fn matches_remote(&self, canonical_target: &str) -> bool {
        self.canonical_git_remote()
            .as_deref()
            .is_some_and(|value| value == canonical_target)
    }

    fn matches_query(&self, query: &str) -> bool {
        self.search_haystack().contains(query)
    }

    fn search_haystack(&self) -> String {
        [
            self.id.as_deref(),
            self.title.as_deref(),
            self.author.as_deref(),
            self.description.as_deref(),
            self.reference.as_deref(),
        ]
        .into_iter()
        .flatten()
        .chain(
            self.files
                .as_ref()
                .into_iter()
                .flat_map(|values| values.iter().map(String::as_str)),
        )
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase()
    }

    fn sort_key(&self) -> Cow<'_, str> {
        if let Some(title) = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Cow::Borrowed(title)
        } else if let Some(id) = self
            .id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Cow::Borrowed(id)
        } else if let Some(reference) = self.reference.as_deref() {
            Cow::Borrowed(reference)
        } else {
            Cow::Borrowed("zzzz-custom-node")
        }
    }
}

fn repo_ref_for_target(resolved: &ResolvedTarget) -> Option<(String, String, String)> {
    match resolved.target_kind {
        TargetKind::Pr => {
            let repo_url = resolved.pr_head_repo_url.as_deref()?;
            let git_ref = resolved.pr_head_ref.as_deref()?.to_string();
            let (owner, repo) = owner_repo_from_canonical_url(repo_url)?;
            Some((owner, repo, git_ref))
        }
        _ => {
            let git_ref = resolved
                .resolved_sha
                .clone()
                .unwrap_or_else(|| resolved.checkout_ref.clone());
            let (owner, repo) = owner_repo_from_canonical_url(&resolved.canonical_repo_url)?;
            Some((owner, repo, git_ref))
        }
    }
}

fn owner_repo_from_canonical_url(url: &str) -> Option<(String, String)> {
    let canonical = canonicalize_remote(url)?;
    let mut parts = canonical.rsplitn(2, '/');
    let repo = parts.next()?.to_string();
    let owner = parts.next()?.rsplit('/').next()?.to_string();
    Some((owner, repo))
}

fn repo_name_from_url(url: &str) -> Option<String> {
    owner_repo_from_canonical_url(url).map(|(_, repo)| repo)
}

fn parse_pyproject_name(text: &str) -> Option<String> {
    let mut in_project = false;
    let section_re = Regex::new(r"^\s*\[(?P<section>[^\]]+)\]\s*$").unwrap();
    let name_re = Regex::new(r#"^\s*name\s*=\s*[\"'](?P<name>[^\"']+)[\"']\s*$"#).unwrap();

    for line in text.lines() {
        if let Some(caps) = section_re.captures(line) {
            in_project = caps.name("section").map(|m| m.as_str()) == Some("project");
            continue;
        }
        if in_project {
            if let Some(caps) = name_re.captures(line) {
                return caps.name("name").map(|m| m.as_str().trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_name_from_project_section() {
        let text = r#"
[build-system]
requires = ["setuptools"]

[project]
name = "wan22fmlf"
version = "1.0.0"
"#;
        assert_eq!(parse_pyproject_name(text).as_deref(), Some("wan22fmlf"));
    }

    #[test]
    fn prefers_project_section_name_only() {
        let text = r#"
[tool.other]
name = "wrong"

[project]
name = "right-name"
"#;
        assert_eq!(parse_pyproject_name(text).as_deref(), Some("right-name"));
    }

    #[test]
    fn returns_first_git_clone_remote_only_for_git_clone_entries() {
        let entry = ManagerCustomNodeEntry {
            id: Some("wan22fmlf".to_string()),
            title: Some("WAN 2.2".to_string()),
            author: None,
            description: None,
            reference: Some("https://github.com/wallen0322/ComfyUI-Wan22FMLF".to_string()),
            files: None,
            install_type: Some("git-clone".to_string()),
        };
        assert_eq!(
            entry.canonical_git_remote().as_deref(),
            Some("https://github.com/wallen0322/ComfyUI-Wan22FMLF")
        );

        let copy_entry = ManagerCustomNodeEntry {
            install_type: Some("copy".to_string()),
            ..entry
        };
        assert_eq!(copy_entry.canonical_git_remote(), None);
    }
}
