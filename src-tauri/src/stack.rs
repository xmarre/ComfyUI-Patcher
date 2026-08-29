use crate::errors::{AppError, AppResult};
use crate::git::canonicalize_remote;
use crate::models::{TrackedPrOverlay, TrackedRepoState};

fn remotes_match(left: &str, right: &str) -> bool {
    match (canonicalize_remote(left), canonicalize_remote(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn overlay_head_repo(overlay: &TrackedPrOverlay) -> Option<String> {
    overlay
        .pr_head_repo_url
        .as_deref()
        .and_then(canonicalize_remote)
}

fn overlay_provides_declared_base(parent: &TrackedPrOverlay, child: &TrackedPrOverlay) -> bool {
    if parent.pr_head_ref.as_deref() != Some(child.pr_base_ref.as_str()) {
        return false;
    }
    let Some(parent_head_repo) = overlay_head_repo(parent) else {
        return false;
    };
    remotes_match(&parent_head_repo, &child.pr_base_repo_url)
}

pub fn overlay_dependency_index(
    tracked_state: &TrackedRepoState,
    overlay_index: usize,
) -> AppResult<Option<usize>> {
    let overlay = tracked_state.overlays.get(overlay_index).ok_or_else(|| {
        AppError::InvalidInput(format!("overlay index {overlay_index} is out of range"))
    })?;
    if !overlay.enabled {
        return Ok(None);
    }

    if overlay.pr_base_ref == tracked_state.base.checkout_ref {
        if remotes_match(
            &overlay.pr_base_repo_url,
            &tracked_state.base.canonical_repo_url,
        ) {
            return Ok(None);
        }
        return Err(AppError::Conflict(format!(
            "PR #{} targets branch '{}' in a different repository than the tracked stack base",
            overlay.pr_number, overlay.pr_base_ref
        )));
    }

    let providers = tracked_state
        .overlays
        .iter()
        .enumerate()
        .filter(|(index, candidate)| {
            *index != overlay_index && overlay_provides_declared_base(candidate, overlay)
        })
        .collect::<Vec<_>>();

    let enabled_predecessors = providers
        .iter()
        .filter(|(index, candidate)| *index < overlay_index && candidate.enabled)
        .map(|(index, candidate)| (*index, *candidate))
        .collect::<Vec<_>>();

    if enabled_predecessors.len() == 1 {
        return Ok(Some(enabled_predecessors[0].0));
    }
    if enabled_predecessors.len() > 1 {
        let parents = enabled_predecessors
            .iter()
            .map(|(_, candidate)| format!("#{}", candidate.pr_number))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(AppError::Conflict(format!(
            "PR #{} targets branch '{}', but multiple earlier enabled overlays provide that branch: {}",
            overlay.pr_number, overlay.pr_base_ref, parents
        )));
    }

    if let Some((_, candidate)) = providers
        .iter()
        .find(|(index, candidate)| *index < overlay_index && !candidate.enabled)
    {
        return Err(AppError::Conflict(format!(
            "PR #{} depends on PR #{} via base branch '{}', but PR #{} is disabled",
            overlay.pr_number,
            candidate.pr_number,
            overlay.pr_base_ref,
            candidate.pr_number
        )));
    }

    if let Some((_, candidate)) = providers.iter().find(|(index, _)| *index > overlay_index) {
        return Err(AppError::Conflict(format!(
            "PR #{} depends on PR #{} via base branch '{}' and must be ordered after it",
            overlay.pr_number, candidate.pr_number, overlay.pr_base_ref
        )));
    }

    Err(AppError::Conflict(format!(
        "PR #{} targets base branch '{}', which is neither the tracked stack base '{}' nor the head branch of an earlier enabled overlay",
        overlay.pr_number, overlay.pr_base_ref, tracked_state.base.checkout_ref
    )))
}

pub fn validate_overlay_stack(tracked_state: &TrackedRepoState) -> AppResult<()> {
    for (index, overlay) in tracked_state.overlays.iter().enumerate() {
        if overlay.enabled {
            overlay_dependency_index(tracked_state, index)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{OverlayApplyStatus, TargetKind, TrackedBaseTarget};

    const REPO: &str = "https://github.com/example/repo";

    fn overlay(
        pr_number: u64,
        base_ref: &str,
        head_ref: Option<&str>,
        enabled: bool,
    ) -> TrackedPrOverlay {
        TrackedPrOverlay {
            id: format!("pr-{pr_number}"),
            source_input: format!("{REPO}/pull/{pr_number}"),
            canonical_repo_url: REPO.to_string(),
            pr_number,
            pr_base_repo_url: REPO.to_string(),
            pr_base_ref: base_ref.to_string(),
            pr_head_repo_url: Some(REPO.to_string()),
            pr_head_ref: head_ref.map(ToOwned::to_owned),
            resolved_sha: Some(format!("{pr_number:040x}")),
            summary_label: format!("PR #{pr_number}"),
            position: 0,
            enabled,
            last_apply_status: Some(if enabled {
                OverlayApplyStatus::Pending
            } else {
                OverlayApplyStatus::Disabled
            }),
            last_error: None,
        }
    }

    fn state(mut overlays: Vec<TrackedPrOverlay>) -> TrackedRepoState {
        for (index, overlay) in overlays.iter_mut().enumerate() {
            overlay.position = index;
        }
        TrackedRepoState {
            version: 1,
            base: TrackedBaseTarget {
                source_input: "main".to_string(),
                target_kind: TargetKind::Branch,
                canonical_repo_url: REPO.to_string(),
                checkout_ref: "main".to_string(),
                resolved_sha: None,
                summary_label: "branch main".to_string(),
            },
            overlays,
            materialized_branch: Some("patcher/stack".to_string()),
        }
    }

    #[test]
    fn accepts_independent_prs_targeting_stack_base() {
        let tracked = state(vec![
            overlay(10, "main", Some("feature/a"), true),
            overlay(11, "main", Some("feature/b"), true),
        ]);
        assert!(validate_overlay_stack(&tracked).is_ok());
        assert_eq!(overlay_dependency_index(&tracked, 0).unwrap(), None);
        assert_eq!(overlay_dependency_index(&tracked, 1).unwrap(), None);
    }

    #[test]
    fn accepts_linear_stacked_pr_chain() {
        let tracked = state(vec![
            overlay(86, "main", Some("feature/seeds-2-3-support"), true),
            overlay(
                87,
                "feature/seeds-2-3-support",
                Some("feature/sa-solver-support"),
                true,
            ),
            overlay(
                88,
                "feature/sa-solver-support",
                Some("feature/next-change"),
                true,
            ),
        ]);
        assert!(validate_overlay_stack(&tracked).is_ok());
        assert_eq!(overlay_dependency_index(&tracked, 1).unwrap(), Some(0));
        assert_eq!(overlay_dependency_index(&tracked, 2).unwrap(), Some(1));
    }

    #[test]
    fn rejects_dependent_pr_before_its_parent() {
        let tracked = state(vec![
            overlay(
                87,
                "feature/seeds-2-3-support",
                Some("feature/sa-solver-support"),
                true,
            ),
            overlay(86, "main", Some("feature/seeds-2-3-support"), true),
        ]);
        let error = validate_overlay_stack(&tracked).unwrap_err().to_string();
        assert!(error.contains("must be ordered after"));
        assert!(error.contains("PR #86"));
    }

    #[test]
    fn rejects_enabled_child_when_parent_is_disabled() {
        let tracked = state(vec![
            overlay(86, "main", Some("feature/seeds-2-3-support"), false),
            overlay(
                87,
                "feature/seeds-2-3-support",
                Some("feature/sa-solver-support"),
                true,
            ),
        ]);
        let error = validate_overlay_stack(&tracked).unwrap_err().to_string();
        assert!(error.contains("PR #86 is disabled"));
    }

    #[test]
    fn permits_disabled_dependency_chain() {
        let tracked = state(vec![
            overlay(86, "main", Some("feature/seeds-2-3-support"), false),
            overlay(
                87,
                "feature/seeds-2-3-support",
                Some("feature/sa-solver-support"),
                false,
            ),
        ]);
        assert!(validate_overlay_stack(&tracked).is_ok());
    }

    #[test]
    fn rejects_unrepresented_intermediate_base() {
        let tracked = state(vec![
            overlay(86, "main", Some("feature/seeds-2-3-support"), true),
            overlay(87, "feature/other", Some("feature/sa-solver-support"), true),
        ]);
        let error = validate_overlay_stack(&tracked).unwrap_err().to_string();
        assert!(error.contains("neither the tracked stack base"));
    }

    #[test]
    fn missing_head_repo_metadata_fails_closed_for_dependency_matching() {
        let mut parent = overlay(86, "main", Some("feature/seeds-2-3-support"), true);
        parent.pr_head_repo_url = None;
        let tracked = state(vec![
            parent,
            overlay(
                87,
                "feature/seeds-2-3-support",
                Some("feature/sa-solver-support"),
                true,
            ),
        ]);
        assert!(validate_overlay_stack(&tracked).is_err());
    }

    #[test]
    fn does_not_match_same_branch_name_from_different_head_repo() {
        let mut parent = overlay(86, "main", Some("feature/seeds-2-3-support"), true);
        parent.pr_head_repo_url = Some("https://github.com/other/fork".to_string());
        let tracked = state(vec![
            parent,
            overlay(
                87,
                "feature/seeds-2-3-support",
                Some("feature/sa-solver-support"),
                true,
            ),
        ]);
        assert!(validate_overlay_stack(&tracked).is_err());
    }
}
