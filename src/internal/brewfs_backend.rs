//! BrewFS worktree backend boundary.
//!
//! BrewFS is a persistent distributed volume, not a Mega revision projection.
//! Libra therefore supplies Git checkout/import semantics while an SDK runtime
//! owns the BrewFS VFS, FUSE session, metadata connection, and object storage.
//!
//! BrewFS 0.1.2 exposes its filesystem client SDK, but the complete mount
//! assembly used by the binary is not yet a stable public constructor. This
//! module keeps that limitation explicit: a caller must inject a
//! [`BrewFsRuntime`] implemented against a suitable BrewFS SDK release.

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::internal::worktree_backend::{
    BackendCapabilities, BackendDescriptor, BackendHealth, BackendKind, BackendMountRequest,
    BackendMountSession, BackendMountSource, WorktreeBackendDriver, WorktreeBackendError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrewFsBackendConfig {
    pub volume: String,
    pub mount_root: PathBuf,
    pub metadata_profile: String,
    pub data_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
}

impl BrewFsBackendConfig {
    pub fn validate(&self) -> Result<(), WorktreeBackendError> {
        for (name, value) in [
            ("volume", self.volume.as_str()),
            ("metadata_profile", self.metadata_profile.as_str()),
            ("data_profile", self.data_profile.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(WorktreeBackendError::InvalidRequest(format!(
                    "BrewFS {name} cannot be empty"
                )));
            }
        }
        if !self.mount_root.is_absolute() {
            return Err(WorktreeBackendError::InvalidRequest(
                "BrewFS mount_root must be absolute".to_string(),
            ));
        }
        validate_subpath(self.subpath.as_deref())?;
        Ok(())
    }
}

/// Runtime implemented with the BrewFS Rust SDK.
///
/// The runtime retains SDK clients and FUSE handles internally. Libra only
/// persists the returned opaque session data and never stores credentials in
/// backend records.
#[async_trait]
pub trait BrewFsRuntime: Send + Sync {
    async fn mount_volume(
        &self,
        config: &BrewFsBackendConfig,
        request: &BackendMountRequest,
        mountpoint: &PathBuf,
    ) -> Result<BackendMountSession, WorktreeBackendError>;

    async fn health(
        &self,
        session: &BackendMountSession,
    ) -> Result<BackendHealth, WorktreeBackendError>;

    async fn flush(
        &self,
        session: &BackendMountSession,
    ) -> Result<(), WorktreeBackendError>;

    async fn unmount(
        &self,
        session: &BackendMountSession,
    ) -> Result<(), WorktreeBackendError>;
}

pub struct BrewFsDriver {
    config: BrewFsBackendConfig,
    runtime: Arc<dyn BrewFsRuntime>,
}

impl BrewFsDriver {
    pub fn new(
        config: BrewFsBackendConfig,
        runtime: Arc<dyn BrewFsRuntime>,
    ) -> Result<Self, WorktreeBackendError> {
        config.validate()?;
        Ok(Self { config, runtime })
    }
}

#[async_trait]
impl WorktreeBackendDriver for BrewFsDriver {
    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: BackendKind::BrewFs,
            display_name: "BrewFS persistent volume",
            protocol_version:
                crate::internal::worktree_backend::BACKEND_CONTROL_PROTOCOL_VERSION,
            capabilities: BackendCapabilities::brewfs(),
            available: true,
            unavailable_reason: None,
        }
    }

    async fn mount(
        &self,
        request: &BackendMountRequest,
    ) -> Result<BackendMountSession, WorktreeBackendError> {
        let (volume, subpath) = match &request.source {
            BackendMountSource::PersistentVolume { volume, subpath } => (volume, subpath),
            BackendMountSource::LocalDirectory { .. } => {
                return Err(WorktreeBackendError::UnsupportedSource {
                    backend: BackendKind::BrewFs,
                    source: "local_directory",
                });
            }
            BackendMountSource::RemoteProjection { .. } => {
                return Err(WorktreeBackendError::UnsupportedSource {
                    backend: BackendKind::BrewFs,
                    source: "remote_projection",
                });
            }
        };
        if volume != &self.config.volume {
            return Err(WorktreeBackendError::InvalidRequest(format!(
                "BrewFS driver is configured for volume '{}', not '{volume}'",
                self.config.volume
            )));
        }
        if let Some(subpath) = subpath.as_deref() {
            validate_subpath(Some(subpath))?;
        }
        let mountpoint = request
            .mountpoint_hint
            .clone()
            .unwrap_or_else(|| self.config.mount_root.join(&request.instance_id));
        self.runtime
            .mount_volume(&self.config, request, &mountpoint)
            .await
    }

    async fn health(
        &self,
        session: &BackendMountSession,
    ) -> Result<BackendHealth, WorktreeBackendError> {
        self.runtime.health(session).await
    }

    async fn flush(
        &self,
        session: &BackendMountSession,
    ) -> Result<(), WorktreeBackendError> {
        self.runtime.flush(session).await
    }

    async fn unmount(
        &self,
        session: &BackendMountSession,
    ) -> Result<(), WorktreeBackendError> {
        self.runtime.unmount(session).await
    }
}

fn validate_subpath(subpath: Option<&str>) -> Result<(), WorktreeBackendError> {
    let Some(subpath) = subpath else {
        return Ok(());
    };
    if subpath.starts_with('/')
        || subpath.contains('\0')
        || subpath
            .split('/')
            .any(|component| matches!(component, "" | "." | ".."))
    {
        return Err(WorktreeBackendError::InvalidRequest(
            "BrewFS subpath must be a normalized relative path".to_string(),
        ));
    }
    Ok(())
}
