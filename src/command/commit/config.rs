//! Typed Git-config defaults for the commit editor template.

use crate::internal::config::{
    LocalIdentityTarget, parse_git_config_bool, read_cascaded_config_value_strict,
};

const COMMIT_STATUS_KEY: &str = "commit.status";

#[derive(Debug, thiserror::Error)]
pub enum CommitDisplayConfigError {
    #[error("failed to read config '{key}': {detail}")]
    Read { key: &'static str, detail: String },
    #[error("bad config value '{value}' for '{key}' (expected a Git boolean)")]
    Invalid { key: &'static str, value: String },
}

/// Resolve whether the editor template should include status information.
///
/// Explicit CLI toggles bypass the config read. Otherwise `commit.status` is
/// read through the strict local -> global -> system cascade and defaults to
/// `true`, matching Git.
pub(super) async fn status_in_editor_template(
    status: bool,
    no_status: bool,
) -> Result<bool, CommitDisplayConfigError> {
    if status {
        return Ok(true);
    }
    if no_status {
        return Ok(false);
    }

    let value =
        read_cascaded_config_value_strict(LocalIdentityTarget::CurrentRepo, COMMIT_STATUS_KEY)
            .await
            .map_err(|error| CommitDisplayConfigError::Read {
                key: COMMIT_STATUS_KEY,
                detail: format!("{error:#}"),
            })?;
    match value {
        Some(value) => parse_git_config_bool(&value).ok_or(CommitDisplayConfigError::Invalid {
            key: COMMIT_STATUS_KEY,
            value,
        }),
        None => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_error_display_is_actionable() {
        let error = CommitDisplayConfigError::Invalid {
            key: COMMIT_STATUS_KEY,
            value: "sometimes".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "bad config value 'sometimes' for 'commit.status' (expected a Git boolean)"
        );
    }
}
