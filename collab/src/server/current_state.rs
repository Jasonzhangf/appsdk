//! The current business state. No mutation history is stored or read.
use super::notification_contract::MutationError;
use super::State;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentFile {
    schema_version: u32,
    state: State,
}

pub(crate) fn load(path: &Path, new_namespace: bool) -> Result<State, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && new_namespace => {
            return Ok(State::default())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "CURRENT_STATE_MIGRATION_REQUIRED: {}",
                path.display()
            ))
        }
        Err(error) => {
            return Err(format!(
                "CURRENT_STATE_READ_FAILED: {}: {error}",
                path.display()
            ))
        }
    };
    let current: CurrentFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("CURRENT_STATE_DECODE_FAILED: {}: {error}", path.display()))?;
    if current.schema_version != 1 {
        return Err(format!(
            "CURRENT_STATE_VERSION_UNSUPPORTED: {}",
            current.schema_version
        ));
    }
    current
        .state
        .global
        .validate()
        .map_err(|error| format!("CURRENT_STATE_INVALID: {error}"))?;
    Ok(current.state)
}

/// False means replacement landed but directory durability is unknown.
/// Callers publish that same current value in memory and return the typed error.
pub(crate) fn save(path: &Path, state: &State) -> Result<bool, MutationError> {
    #[derive(Serialize)]
    struct CurrentRef<'a> {
        schema_version: u32,
        state: &'a State,
    }
    let body = serde_json::to_vec(&CurrentRef {
        schema_version: 1,
        state,
    })
    .map_err(|error| MutationError::Persistence(error.to_string()))?;
    let parent = path
        .parent()
        .ok_or_else(|| MutationError::Persistence("current state has no parent".into()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| MutationError::Persistence(error.to_string()))?;
    let temp = parent.join(format!(".state-{:032x}.tmp", rand::random::<u128>()));
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&body)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(MutationError::Persistence(error.to_string()));
    }
    Ok(std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .is_ok())
}

/// JSON maps cannot encode tuple keys. These are current relationship rows,
/// represented directly as key/value pairs, with no events or history.
pub(crate) mod rows {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;
    pub(crate) fn serialize<K: Serialize, V: Serialize, S: Serializer>(
        map: &BTreeMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(serializer)
    }
    pub(crate) fn deserialize<
        'de,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    >(
        deserializer: D,
    ) -> Result<BTreeMap<K, V>, D::Error> {
        Ok(Vec::<(K, V)>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}
