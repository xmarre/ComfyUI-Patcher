use crate::errors::{AppError, AppResult};
use crate::git::{
    canonicalize_remote, ls_remote_default_branch_remote, ls_remote_head, ls_remote_head_remote,
    ls_remote_tag, ls_remote_tag_remote,
};
use crate::models::{ResolvedTarget, TargetKind};
use crate::util::slugify;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

#[derive(Clone)]
pub struct GithubClient {
    client: reqwest::Client,
    _token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PullHeadRepo {
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct PullHead {
    sha: String,
    #[serde(rename = "ref")]
    ref_name: String,
    repo: PullHeadRepo,
}

#[derive(Debug, Deserialize)]
struct PullBaseRepo {
    clone_url: String,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct PullBase {
    #[serde(rename = "ref")]
    ref_name: String,
    repo: PullBaseRepo,
}

#[derive(Debug, Deserialize)]
struct PullResponse {
    number: u64,
    head: PullHead,
    base: PullBase,
}

#[derive(Debug, Deserialize)]
struct RepoResponse {
    default_branch: String,
    clone_url: String,
}

impl GithubClient {
    pub fn new(token: Option<String>) -> AppResult<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("comfyui-patcher"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        if let Some(token) = &token {
            headers.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| AppError::Github(e.to_string()))?,
            );
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .default_headers(headers)
            .build()?;
        Ok(Self {
            client,
            _token: token,
        })
    }

    async fn get_repo(&self, owner: &str, repo: &str) -> AppResult<RepoResponse> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}");
        Ok(self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn get_pr(&self, owner: &str, repo: &str, number: u64) -> AppResult<PullResponse> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{number}");
        Ok(self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn resolve_target(
        &self,
        input: &str,
        current_repo_remote: Option<&str>,
        current_repo_path: Option<&Path>,
    ) -> AppResult<ResolvedTarget> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("target input is empty".to_string()));
        }

        if let Some(pr) = parse_pr_url(trimmed) {
            let pr_data = self.get_pr(&pr.owner, &pr.repo, pr.number).await?;
            let canonical_repo_url =
                canonicalize_remote(&pr_data.base.repo.html_url).ok_or_else(|| {
                    AppError::Github("could not canonicalize PR base repo".to_string())
                })?;
            let fetch_url = pr_data.base.repo.clone_url;
            let head_url = canonicalize_remote(&pr_data.head.repo.html_url).ok_or_else(|| {
                AppError::Github("could not canonicalize PR head repo".to_string())
            })?;
            return Ok(ResolvedTarget {
                source_input: trimmed.to_string(),
                target_kind: TargetKind::Pr,
                canonical_repo_url: canonical_repo_url.clone(),
                fetch_url,
                checkout_ref: format!("patcher/pr-{}", pr.number),
                resolved_sha: Some(pr_data.head.sha.clone()),
                pr_number: Some(pr.number),
                pr_base_repo_url: Some(canonical_repo_url),
                pr_base_ref: Some(pr_data.base.ref_name.clone()),
                pr_head_repo_url: Some(head_url),
                pr_head_ref: Some(pr_data.head.ref_name.clone()),
                summary_label: format!("PR #{} @ {}", pr_data.number, short_sha(&pr_data.head.sha)),
                suggested_local_dir_name: slugify(&pr.repo),
            });
        }

        if let Some(tree) = parse_branch_url(trimmed) {
            let canonical_repo_url = format!("https://github.com/{}/{}", tree.owner, tree.repo);
            let fetch_url = format!("https://github.com/{}/{}.git", tree.owner, tree.repo);
            let (target_kind, checkout_ref) = resolve_tree_ref(&fetch_url, &tree.branch).await?;
            return Ok(ResolvedTarget {
                source_input: trimmed.to_string(),
                target_kind: target_kind.clone(),
                canonical_repo_url: canonical_repo_url.clone(),
                fetch_url,
                checkout_ref: checkout_ref.clone(),
                resolved_sha: None,
                pr_number: None,
                pr_base_repo_url: None,
                pr_base_ref: None,
                pr_head_repo_url: None,
                pr_head_ref: None,
                summary_label: match target_kind {
                    TargetKind::Branch => {
                        format!("branch {} @ {}/{}", checkout_ref, tree.owner, tree.repo)
                    }
                    TargetKind::Tag => {
                        format!("tag {} @ {}/{}", checkout_ref, tree.owner, tree.repo)
                    }
                    _ => format!("ref {} @ {}/{}", checkout_ref, tree.owner, tree.repo),
                },
                suggested_local_dir_name: slugify(&tree.repo),
            });
        }

        if let Some(commit) = parse_commit_url(trimmed) {
            let canonical_repo_url = format!("https://github.com/{}/{}", commit.owner, commit.repo);
            return Ok(ResolvedTarget {
                source_input: trimmed.to_string(),
                target_kind: TargetKind::Commit,
                canonical_repo_url: canonical_repo_url.clone(),
                fetch_url: format!("https://github.com/{}/{}.git", commit.owner, commit.repo),
                checkout_ref: commit.sha.clone(),
                resolved_sha: Some(commit.sha.clone()),
                pr_number: None,
                pr_base_repo_url: None,
                pr_base_ref: None,
                pr_head_repo_url: None,
                pr_head_ref: None,
                summary_label: format!("commit {}", short_sha(&commit.sha)),
                suggested_local_dir_name: slugify(&commit.repo),
            });
        }

        if let Some(repo) = parse_repo_url(trimmed) {
            let canonical_repo_url = format!("https://github.com/{}/{}", repo.owner, repo.repo);
            let canonical_repo_url = canonicalize_remote(&canonical_repo_url).ok_or_else(|| {
                AppError::Github("could not canonicalize repository URL".to_string())
            })?;
            let fetch_url = format!("{canonical_repo_url}.git");

            match self.get_repo(&repo.owner, &repo.repo).await {
                Ok(metadata) => {
                    let canonical_repo_url = canonicalize_remote(&metadata.clone_url)
                        .ok_or_else(|| {
                            AppError::Github("could not canonicalize repository URL".to_string())
                        })?;
                    let fetch_url = metadata.clone_url;
                    let default_branch = metadata.default_branch;
                    return Ok(ResolvedTarget {
                        source_input: trimmed.to_string(),
                        target_kind: TargetKind::DefaultBranch,
                        canonical_repo_url,
                        fetch_url,
                        checkout_ref: default_branch.clone(),
                        resolved_sha: None,
                        pr_number: None,
                        pr_base_repo_url: None,
                        pr_base_ref: None,
                        pr_head_repo_url: None,
                        pr_head_ref: None,
                        summary_label: format!("default branch {}", default_branch),
                        suggested_local_dir_name: slugify(&repo.repo),
                    });
                }
                Err(api_err) => {
                    let default_branch = ls_remote_default_branch_remote(&fetch_url)
                        .await
                        .map_err(|git_err| {
                            AppError::Github(format!(
                                "could not resolve repository default branch via GitHub API ({api_err}) or git ls-remote HEAD ({git_err})"
                            ))
                        })?
                        .ok_or_else(|| {
                            AppError::Github(format!(
                                "could not resolve repository default branch via GitHub API ({api_err}) or git ls-remote HEAD"
                            ))
                        })?;
                    return Ok(ResolvedTarget {
                        source_input: trimmed.to_string(),
                        target_kind: TargetKind::DefaultBranch,
                        canonical_repo_url,
                        fetch_url,
                        checkout_ref: default_branch.clone(),
                        resolved_sha: None,
                        pr_number: None,
                        pr_base_repo_url: None,
                        pr_base_ref: None,
                        pr_head_repo_url: None,
                        pr_head_ref: None,
                        summary_label: format!("default branch {}", default_branch),
                        suggested_local_dir_name: slugify(&repo.repo),
                    });
                }
            }
        }

        if let Some(repo_path) = current_repo_path {
            let kind = if ls_remote_head(repo_path, trimmed).await? {
                Some(TargetKind::Branch)
            } else if ls_remote_tag(repo_path, trimmed).await? {
                Some(TargetKind::Tag)
            } else {
                None
            };

            if let Some(kind) = kind {
                let canonical_repo_url = current_repo_remote.ok_or_else(|| {
                    AppError::InvalidInput("missing current repository remote".to_string())
                })?;
                return Ok(ResolvedTarget {
                    source_input: trimmed.to_string(),
                    target_kind: kind.clone(),
                    canonical_repo_url: canonical_repo_url.to_string(),
                    fetch_url: format!("{canonical_repo_url}.git"),
                    checkout_ref: trimmed.to_string(),
                    resolved_sha: None,
                    pr_number: None,
                    pr_base_repo_url: None,
                    pr_base_ref: None,
                    pr_head_repo_url: None,
                    pr_head_ref: None,
                    summary_label: match kind {
                        TargetKind::Branch => format!("branch {trimmed}"),
                        TargetKind::Tag => format!("tag {trimmed}"),
                        _ => format!("ref {trimmed}"),
                    },
                    suggested_local_dir_name: canonical_repo_url
                        .rsplit('/')
                        .next()
                        .unwrap_or("repo")
                        .to_string(),
                });
            }

            if is_probable_sha(trimmed) {
                let canonical_repo_url = current_repo_remote.ok_or_else(|| {
                    AppError::InvalidInput(
                        "raw commit SHA requires an existing repository context".to_string(),
                    )
                })?;
                return Ok(ResolvedTarget {
                    source_input: trimmed.to_string(),
                    target_kind: TargetKind::Commit,
                    canonical_repo_url: canonical_repo_url.to_string(),
                    fetch_url: format!("{canonical_repo_url}.git"),
                    checkout_ref: trimmed.to_string(),
                    resolved_sha: Some(trimmed.to_string()),
                    pr_number: None,
                    pr_base_repo_url: None,
                    pr_base_ref: None,
                    pr_head_repo_url: None,
                    pr_head_ref: None,
                    summary_label: format!("commit {}", short_sha(trimmed)),
                    suggested_local_dir_name: canonical_repo_url
                        .rsplit('/')
                        .next()
                        .unwrap_or("repo")
                        .to_string(),
                });
            }

            return Err(AppError::InvalidInput(format!(
                "could not resolve branch, tag, or commit '{trimmed}' against origin"
            )));
        }

        if is_probable_sha(trimmed) {
            let canonical_repo_url = current_repo_remote.ok_or_else(|| {
                AppError::InvalidInput(
                    "raw commit SHA requires an existing repository context".to_string(),
                )
            })?;
            return Ok(ResolvedTarget {
                source_input: trimmed.to_string(),
                target_kind: TargetKind::Commit,
                canonical_repo_url: canonical_repo_url.to_string(),
                fetch_url: format!("{canonical_repo_url}.git"),
                checkout_ref: trimmed.to_string(),
                resolved_sha: Some(trimmed.to_string()),
                pr_number: None,
                pr_base_repo_url: None,
                pr_base_ref: None,
                pr_head_repo_url: None,
                pr_head_ref: None,
                summary_label: format!("commit {}", short_sha(trimmed)),
                suggested_local_dir_name: canonical_repo_url
                    .rsplit('/')
                    .next()
                    .unwrap_or("repo")
                    .to_string(),
            });
        }

        Err(AppError::InvalidInput(
            "raw branch or tag names require an existing repository context; use a GitHub URL for new custom nodes"
                .to_string(),
        ))
    }
}

#[derive(Debug)]
struct RepoUrlParts {
    owner: String,
    repo: String,
}

#[derive(Debug)]
struct BranchUrlParts {
    owner: String,
    repo: String,
    branch: String,
}

#[derive(Debug)]
struct CommitUrlParts {
    owner: String,
    repo: String,
    sha: String,
}

#[derive(Debug)]
struct PrUrlParts {
    owner: String,
    repo: String,
    number: u64,
}

fn parse_repo_url(input: &str) -> Option<RepoUrlParts> {
    let url = url::Url::parse(input).ok()?;
    if !url.host_str()?.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() == 2 {
        Some(RepoUrlParts {
            owner: segments[0].to_string(),
            repo: segments[1].trim_end_matches(".git").to_string(),
        })
    } else {
        None
    }
}

fn parse_branch_url(input: &str) -> Option<BranchUrlParts> {
    let url = url::Url::parse(input).ok()?;
    if !url.host_str()?.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() >= 4 && segments[2] == "tree" {
        Some(BranchUrlParts {
            owner: segments[0].to_string(),
            repo: segments[1].trim_end_matches(".git").to_string(),
            branch: segments[3..].join("/"),
        })
    } else {
        None
    }
}

fn parse_commit_url(input: &str) -> Option<CommitUrlParts> {
    let url = url::Url::parse(input).ok()?;
    if !url.host_str()?.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() == 4 && segments[2] == "commit" {
        Some(CommitUrlParts {
            owner: segments[0].to_string(),
            repo: segments[1].trim_end_matches(".git").to_string(),
            sha: segments[3].to_string(),
        })
    } else {
        None
    }
}

fn parse_pr_url(input: &str) -> Option<PrUrlParts> {
    let url = url::Url::parse(input).ok()?;
    if !url.host_str()?.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() == 4 && segments[2] == "pull" {
        Some(PrUrlParts {
            owner: segments[0].to_string(),
            repo: segments[1].trim_end_matches(".git").to_string(),
            number: segments[3].parse().ok()?,
        })
    } else {
        None
    }
}

async fn resolve_tree_ref(fetch_url: &str, tail: &str) -> AppResult<(TargetKind, String)> {
    let parts: Vec<&str> = tail
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(AppError::InvalidInput(
            "GitHub tree URL is missing the target branch or tag".to_string(),
        ));
    }

    for end in (1..=parts.len()).rev() {
        let candidate = parts[..end].join("/");
        if ls_remote_head_remote(fetch_url, &candidate).await? {
            return Ok((TargetKind::Branch, candidate));
        }
        if ls_remote_tag_remote(fetch_url, &candidate).await? {
            return Ok((TargetKind::Tag, candidate));
        }
    }

    Err(AppError::InvalidInput(format!(
        "could not resolve tree URL target '{tail}' against remote refs"
    )))
}

fn is_probable_sha(input: &str) -> bool {
    Regex::new(r"^[0-9a-fA-F]{7,40}$").unwrap().is_match(input)
}

fn short_sha(input: &str) -> String {
    input.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tree_urls_with_slashes() {
        let parsed =
            parse_branch_url("https://github.com/Comfy-Org/ComfyUI/tree/feature/a/b").unwrap();
        assert_eq!(parsed.owner, "Comfy-Org");
        assert_eq!(parsed.repo, "ComfyUI");
        assert_eq!(parsed.branch, "feature/a/b");
    }

    #[test]
    fn parses_tree_urls_with_paths_after_branch() {
        let parsed =
            parse_branch_url("https://github.com/Comfy-Org/ComfyUI/tree/main/src/components")
                .unwrap();
        assert_eq!(parsed.branch, "main/src/components");
    }

    #[test]
    fn parses_pr_urls() {
        let parsed = parse_pr_url("https://github.com/Comfy-Org/ComfyUI/pull/12936").unwrap();
        assert_eq!(parsed.number, 12936);
    }

    #[test]
    fn detects_sha() {
        assert!(is_probable_sha("abcdef1234567890"));
        assert!(!is_probable_sha("feature/something"));
    }
}
