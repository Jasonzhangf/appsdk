use crate::identity::{AppServerId, OperationId};
use crate::proto::ProjectContext;
use crate::scope::{HostPaths, Scope};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const PROOF_VERSION: u8 = 1;
const PROOF_DIR: &str = "context-operations";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContextOperationProof {
    pub version: u8,
    pub operation_id: String,
    pub query_capability: String,
    pub project_scope: String,
    pub app_scope_id: String,
}

#[derive(Debug)]
pub(crate) struct PreparedContextOperation {
    pub(crate) proof: ContextOperationProof,
}

fn proof_dir(host_paths: &HostPaths) -> PathBuf {
    host_paths.state_root().join(PROOF_DIR)
}

fn proof_path(host_paths: &HostPaths, operation_id: &str) -> PathBuf {
    proof_dir(host_paths).join(format!("{operation_id}.json"))
}

fn validate_operation_id(operation_id: &str) -> anyhow::Result<()> {
    OperationId::new(operation_id.to_owned())
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("IDENTITY_OPERATION_ID_INVALID: {error}"))
}

fn read_proof(path: &Path) -> anyhow::Result<ContextOperationProof> {
    let bytes = std::fs::read(path)?;
    let proof: ContextOperationProof = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow::anyhow!("IDENTITY_OPERATION_PROOF_INVALID: {error}"))?;
    if proof.version != PROOF_VERSION {
        anyhow::bail!(
            "IDENTITY_OPERATION_PROOF_INVALID: unsupported proof version {}",
            proof.version
        );
    }
    validate_operation_id(&proof.operation_id)?;
    if proof.operation_id
        != path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("")
    {
        anyhow::bail!(
            "IDENTITY_OPERATION_PROOF_INVALID: proof filename does not match operation id"
        );
    }
    if proof.query_capability.trim().is_empty()
        || proof.query_capability.chars().any(char::is_control)
    {
        anyhow::bail!("IDENTITY_OPERATION_PROOF_INVALID: query capability is invalid");
    }
    if proof.project_scope.trim().is_empty()
        || proof.app_scope_id.trim().is_empty()
        || proof.project_scope.chars().any(char::is_control)
        || proof.app_scope_id.chars().any(char::is_control)
    {
        anyhow::bail!("IDENTITY_OPERATION_PROOF_INVALID: proof scope is invalid");
    }
    Ok(proof)
}

fn write_proof_atomic(path: &Path, proof: &ContextOperationProof) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    let mut bytes = serde_json::to_vec(proof)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    match std::fs::hard_link(&temporary, path) {
        Ok(()) => {
            std::fs::remove_file(&temporary)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(&temporary)?;
            anyhow::bail!("IDENTITY_OPERATION_PROOF_EXISTS: {}", path.display())
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error.into())
        }
    }
}

fn random_capability() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut encoded = String::with_capacity(7 + 43);
    encoded.push_str("base64url:");
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        encoded.push(ALPHABET[(b0 >> 2) as usize] as char);
        encoded.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(b2 & 0x3f) as usize] as char);
        }
    }
    encoded
}

fn random_operation_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut suffix = String::with_capacity(32);
    for byte in bytes {
        suffix.push_str(&format!("{byte:02x}"));
    }
    format!("ctxop-{suffix}")
}

fn proof_matches(proof: &ContextOperationProof, project_context: &ProjectContext) -> bool {
    proof.project_scope == project_context.project_scope.as_str()
        && proof.app_scope_id == project_context.app_scope_id.as_str()
}

fn load_or_create_proof(
    host_paths: &HostPaths,
    project_context: &ProjectContext,
    operation_id: Option<String>,
) -> anyhow::Result<ContextOperationProof> {
    let operation_id = operation_id.unwrap_or_else(random_operation_id);
    validate_operation_id(&operation_id)?;
    let path = proof_path(host_paths, &operation_id);
    if path.exists() {
        let proof = read_proof(&path)?;
        if !proof_matches(&proof, project_context) {
            anyhow::bail!(
                "IDENTITY_OPERATION_PROOF_SCOPE_MISMATCH: proof scope does not match requested project/app scope"
            );
        }
        return Ok(proof);
    }
    let proof = ContextOperationProof {
        version: PROOF_VERSION,
        operation_id,
        query_capability: random_capability(),
        project_scope: project_context.project_scope.as_str().to_owned(),
        app_scope_id: project_context.app_scope_id.as_str().to_owned(),
    };
    write_proof_atomic(&path, &proof)?;
    Ok(proof)
}

pub(crate) fn prepare_mutating_operation(
    host_paths: &HostPaths,
    project_context: &ProjectContext,
    operation_id: Option<String>,
) -> anyhow::Result<PreparedContextOperation> {
    let proof = load_or_create_proof(host_paths, project_context, operation_id)?;
    Ok(PreparedContextOperation { proof })
}

pub(crate) fn prepare_query_operation(
    host_paths: &HostPaths,
    project_context: &ProjectContext,
    operation_id: String,
) -> anyhow::Result<PreparedContextOperation> {
    validate_operation_id(&operation_id)?;
    let proof = read_proof(&proof_path(host_paths, &operation_id)).map_err(|error| {
        anyhow::anyhow!(
            "IDENTITY_OPERATION_PROOF_MISSING: no local proof exists for {operation_id}: {error}"
        )
    })?;
    if !proof_matches(&proof, project_context) {
        anyhow::bail!(
            "IDENTITY_OPERATION_PROOF_SCOPE_MISMATCH: proof scope does not match requested project/app scope"
        );
    }
    Ok(PreparedContextOperation { proof })
}

#[cfg(test)]
pub(crate) fn prepare_test_proof(
    host_paths: &HostPaths,
    project_context: &ProjectContext,
    operation_id: &str,
) -> anyhow::Result<()> {
    let path = proof_path(host_paths, operation_id);
    let proof = ContextOperationProof {
        version: PROOF_VERSION,
        operation_id: operation_id.to_owned(),
        query_capability: random_capability(),
        project_scope: project_context.project_scope.as_str().to_owned(),
        app_scope_id: project_context.app_scope_id.as_str().to_owned(),
    };
    write_proof_atomic(&path, &proof)
}

pub(crate) fn cli_project_context_for_root(
    root: &Path,
    app_scope_id: &AppServerId,
) -> anyhow::Result<ProjectContext> {
    ProjectContext::for_registered_root_with_app(root, app_scope_id.clone())
}

pub(crate) fn default_project_context(scope: &Scope) -> anyhow::Result<ProjectContext> {
    cli_project_context_for_root(
        &scope.root,
        &AppServerId::new(crate::identity::CLI_APP_SERVER_ID)?,
    )
}
