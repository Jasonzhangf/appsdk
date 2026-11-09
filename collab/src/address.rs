use crate::proto::{AddressInput, LocalNameAddress, QualifiedNameAddress};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Configuration and current project aliases supplied by the daemon owner.
#[derive(Default)]
pub struct AddressContext<'a> {
    pub local_machine: Option<&'a str>,
    pub peers: &'a [String],
    pub local_projects: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ResolvedAddress {
    LocalLegacy(String),
    LocalName(LocalNameAddress),
    Remote {
        machine: String,
        target: LocalNameAddress,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressError {
    pub code: &'static str,
    pub detail: String,
}

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for AddressError {}

pub fn resolve_address(
    input: &AddressInput,
    context: &AddressContext<'_>,
) -> Result<ResolvedAddress, AddressError> {
    let raw = match input {
        AddressInput::Qualified(address) => return resolve_qualified(address.clone(), context),
        AddressInput::Legacy(raw) => raw,
    };
    let parts: Vec<_> = raw.split('.').collect();
    if parts.len() <= 2 {
        if parts.len() == 2 && raw.contains('%') {
            return Ok(ResolvedAddress::LocalName(LocalNameAddress {
                project: decode_segment(parts[0])?,
                name: decode_segment(parts[1])?,
            }));
        }
        return Ok(ResolvedAddress::LocalLegacy(raw.clone()));
    }

    let (legacy_project, name) = raw.rsplit_once('.').unwrap();
    let legacy_alias = context
        .local_projects
        .iter()
        .find(|project| project.eq_ignore_ascii_case(legacy_project));
    let known_machine = context
        .local_machine
        .unwrap_or("local")
        .eq_ignore_ascii_case(parts[0])
        || context
            .peers
            .iter()
            .any(|peer| peer.eq_ignore_ascii_case(parts[0]));
    if let Some(project) = legacy_alias {
        if parts.len() != 3 || !known_machine {
            return Ok(ResolvedAddress::LocalName(LocalNameAddress {
                project: project.clone(),
                name: name.to_owned(),
            }));
        }
        if !raw.contains('%') {
            let local = QualifiedNameAddress {
                machine: context.local_machine.unwrap_or("local").to_owned(),
                local: LocalNameAddress {
                    project: project.clone(),
                    name: name.to_owned(),
                },
            };
            let remote = QualifiedNameAddress {
                machine: parts[0].to_owned(),
                local: LocalNameAddress {
                    project: parts[1].to_owned(),
                    name: name.to_owned(),
                },
            };
            return Err(AddressError {
                code: "ADDRESS_AMBIGUOUS",
                detail: format!(
                    "use {} or {}",
                    format_qualified_address(&local, context)?,
                    format_qualified_address(&remote, context)?
                ),
            });
        }
    }
    if parts.len() != 3 {
        return Err(invalid("qualified addresses require machine.project.name"));
    }
    resolve_qualified(
        QualifiedNameAddress {
            machine: parts[0].to_owned(),
            local: LocalNameAddress {
                project: decode_segment(parts[1])?,
                name: decode_segment(parts[2])?,
            },
        },
        context,
    )
}

pub fn format_local_address(address: &LocalNameAddress) -> Result<String, AddressError> {
    validate_segment(&address.project)?;
    validate_segment(&address.name)?;
    Ok(format!(
        "{}.{}",
        encode_segment(&address.project),
        encode_segment(&address.name)
    ))
}

pub fn format_qualified_address(
    address: &QualifiedNameAddress,
    context: &AddressContext<'_>,
) -> Result<String, AddressError> {
    let (machine, _) = canonical_machine(&address.machine, context)?;
    validate_segment(&address.local.project)?;
    validate_segment(&address.local.name)?;
    let mut project = encode_segment(&address.local.project);
    let name = encode_segment(&address.local.name);
    let legacy_project = format!("{}.{}", machine, project);
    let conflicts = context
        .local_projects
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(&legacy_project));
    if conflicts && !project.contains('%') && !name.contains('%') {
        let first = project.chars().next().unwrap();
        let encoded_first = first
            .to_string()
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>();
        project = format!("{}{}", encoded_first, &project[first.len_utf8()..]);
    }
    Ok(format!("{machine}.{project}.{name}"))
}

pub fn validate_machine_name(machine: &str) -> Result<(), AddressError> {
    validate_segment(machine)?;
    if machine.contains(['.', '%']) {
        return Err(invalid(
            "machine names cannot contain dots or percent signs",
        ));
    }
    Ok(())
}

fn resolve_qualified(
    address: QualifiedNameAddress,
    context: &AddressContext<'_>,
) -> Result<ResolvedAddress, AddressError> {
    validate_segment(&address.local.project)?;
    validate_segment(&address.local.name)?;
    let (machine, is_local) = canonical_machine(&address.machine, context)?;
    if is_local {
        Ok(ResolvedAddress::LocalName(address.local))
    } else {
        Ok(ResolvedAddress::Remote {
            machine,
            target: address.local,
        })
    }
}

fn canonical_machine(
    machine: &str,
    context: &AddressContext<'_>,
) -> Result<(String, bool), AddressError> {
    validate_machine_name(machine)?;
    let local = context.local_machine.unwrap_or("local");
    if local.eq_ignore_ascii_case(machine) {
        return Ok((local.to_owned(), true));
    }
    if context.local_machine.is_none() {
        return Err(AddressError {
            code: "MACHINE_NOT_CONFIGURED",
            detail: "configure this daemon's machine name before remote communication".into(),
        });
    }
    let mut peers = context
        .peers
        .iter()
        .filter(|peer| peer.eq_ignore_ascii_case(machine));
    let Some(peer) = peers.next() else {
        return Err(AddressError {
            code: "MACHINE_UNKNOWN",
            detail: format!("machine {machine} is not declared"),
        });
    };
    if peers.next().is_some() {
        return Err(AddressError {
            code: "ADDRESS_AMBIGUOUS",
            detail: format!("machine {machine} has multiple declarations"),
        });
    }
    Ok((peer.clone(), false))
}

fn validate_segment(segment: &str) -> Result<(), AddressError> {
    if segment.is_empty()
        || segment
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(invalid(
            "address segments must be nonempty and contain no whitespace or control characters",
        ));
    }
    Ok(())
}

fn encode_segment(segment: &str) -> String {
    segment.replace('%', "%25").replace('.', "%2E")
}

fn decode_segment(segment: &str) -> Result<String, AddressError> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = bytes
                .get(index + 1..index + 3)
                .ok_or_else(|| invalid("incomplete percent escape"))?;
            let digits =
                std::str::from_utf8(digits).map_err(|_| invalid("invalid percent escape"))?;
            decoded.push(
                u8::from_str_radix(digits, 16).map_err(|_| invalid("invalid percent escape"))?,
            );
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| invalid("address is not valid UTF-8"))?;
    validate_segment(&decoded)?;
    Ok(decoded)
}

fn invalid(detail: &str) -> AddressError {
    AddressError {
        code: "ADDRESS_INVALID",
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
#[path = "address/tests.rs"]
mod tests;
