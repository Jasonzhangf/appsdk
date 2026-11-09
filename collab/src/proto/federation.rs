//! 联邦 wire 共享合同（rex-federation-wire-20261108）。
//!
//! 本模块只定义联邦地址、请求、响应与凭据信封的类型合同，不包含
//! daemon 逻辑、路由决策或持久化。所有 serde 标签统一使用 `kind` /
//! `value`，字段使用 snake_case；联邦公开编码使用 `collab-ref:` 前缀
//! 加上 MessageReference 的紧凑 JSON，不引入 hash/base64 新依赖，
//! 不放业务 metadata。

use serde::{Deserialize, Serialize};

use super::{DirectoryTarget, ReplyPolicy};
use crate::identity::{AppServerId, MessageId, OperationId};
use crate::scope::ProjectScopeId;

/// 联邦机器名。轻量别名到 String，不包装，避免无必要的包装与转换；
/// 机器名地址/配置的合法性由对应的 address/config owner 校验。
pub type MachineName = String;

/// 本机名址：项目与该项目下的短名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalNameAddress {
    pub project: String,
    pub name: String,
}

/// 限定名址：机器 + 本机名址。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualifiedNameAddress {
    pub machine: MachineName,
    pub local: LocalNameAddress,
}

/// 地址输入：旧字符串地址或限定名地址，统一 `kind`/`value` 标签。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AddressInput {
    Legacy(String),
    Qualified(QualifiedNameAddress),
}

/// 联邦主体：地址、项目作用域、应用作用域与稳定名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationPrincipal {
    pub address: QualifiedNameAddress,
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub stable_name: String,
}

/// 源消息引用：来源机器、项目作用域、应用作用域与 operation_id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginMessageRef {
    pub machine: MachineName,
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub operation_id: OperationId,
}

/// 消息引用：来源机器、项目作用域、应用作用域与 msg_id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRef {
    pub machine: MachineName,
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub msg_id: MessageId,
}

/// 消息引用的两种形式：原消息引用或消息引用，统一 `kind`/`value`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MessageReference {
    Origin(OriginMessageRef),
    Message(MessageRef),
}

/// 联邦请求判别式，统一 `kind`/`value` 标签。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FederationReq {
    Hello,
    Directory {
        project: Option<String>,
    },
    Put(FederationPut),
    Lookup {
        origin: OriginMessageRef,
        requester: FederationActor,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compare: Option<FederationPut>,
    },
    Receipt {
        reference: MessageRef,
        requester: FederationActor,
    },
}

/// 联邦信道引用：信道所属机器、项目作用域、应用作用域与入口标识。
/// 暂用必要类型，不含 secret、reply_route 或正文控制镜像。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedChannelRef {
    pub owner_machine: MachineName,
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub inbound_id: String,
}

impl FederatedChannelRef {
    pub fn reply_address(&self) -> String {
        format!(
            "collab-channel:{}",
            serde_json::to_string(self).expect("typed channel reference serializes")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FederationActor {
    Peer(FederationPrincipal),
    ExternalChannel(FederatedChannelRef),
}

impl FederationActor {
    pub fn machine(&self) -> &str {
        match self {
            Self::Peer(principal) => &principal.address.machine,
            Self::ExternalChannel(channel) => &channel.owner_machine,
        }
    }

    pub fn project_scope(&self) -> &ProjectScopeId {
        match self {
            Self::Peer(principal) => &principal.project_scope,
            Self::ExternalChannel(channel) => &channel.project_scope,
        }
    }

    pub fn app_scope_id(&self) -> &AppServerId {
        match self {
            Self::Peer(principal) => &principal.app_scope_id,
            Self::ExternalChannel(channel) => &channel.app_scope_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FederationDestination {
    Named(LocalNameAddress),
    ChannelOwner(FederatedChannelRef),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FederationEndpoint {
    Named(QualifiedNameAddress),
    ChannelOwner(FederatedChannelRef),
}

impl FederationEndpoint {
    pub fn machine(&self) -> &str {
        match self {
            Self::Named(address) => &address.machine,
            Self::ChannelOwner(channel) => &channel.owner_machine,
        }
    }

    pub fn destination(&self) -> FederationDestination {
        match self {
            Self::Named(address) => FederationDestination::Named(address.local.clone()),
            Self::ChannelOwner(channel) => FederationDestination::ChannelOwner(channel.clone()),
        }
    }
}

/// 联邦发送：channel 暂用必要类型，不含 secret、reply_route 或正文控制镜像。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationPut {
    pub origin: OriginMessageRef,
    pub sender: FederationActor,
    pub target: FederationDestination,
    pub mtype: String,
    pub subject: String,
    pub body: String,
    pub delivery: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<MessageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_origin: Option<OriginMessageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_policy: Option<ReplyPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<FederatedChannelRef>,
}

/// 联邦信任凭据与请求的信封。只 derive Serialize/Deserialize，不 derive
/// Debug/Display，避免凭据进入诊断日志；请求本身由 FederationReq 负责 Debug。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationEnvelope {
    pub target: MachineName,
    pub credential: String,
    pub request: FederationReq,
}

/// Current notification evidence projected by the target notification owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationNotificationStatus {
    NotAttempted,
    Accepted,
    KnownNotDelivered,
    Unknown,
}

/// A receipt is read from the machine that owns the mailbox message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationMessageReceipt {
    pub origin: OriginMessageRef,
    pub reference: MessageRef,
    pub sender: FederationActor,
    pub recipient: FederationEndpoint,
    pub notification: FederationNotificationStatus,
    pub consumed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_delivery: Option<crate::server::channel_delivery::ChannelDeliveryResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_delivery_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_reference: Option<MessageRef>,
}

/// Each variant has the fields required by its operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FederationResponse {
    Hello {
        machine: MachineName,
        protocol: String,
    },
    Directory {
        targets: Vec<DirectoryTarget>,
    },
    Message {
        receipt: FederationMessageReceipt,
    },
    NotSeen,
    Expired {
        reference: MessageRef,
    },
    Error {
        code: String,
        detail: String,
    },
}

/// 联邦公开编码前缀。
pub const COLLAB_REF_PREFIX: &str = "collab-ref:";
pub const FEDERATION_PROTOCOL: &str = "collab-federation/v1";

/// 将 MessageReference 编码为 `collab-ref:` 加紧凑 JSON 的公开引用字符串。
pub fn format_collab_ref(reference: &MessageReference) -> Result<String, serde_json::Error> {
    serde_json::to_string(reference).map(|value| format!("{COLLAB_REF_PREFIX}{value}"))
}

/// 将 `collab-ref:` 引用字符串解析为 MessageReference；缺少前缀或非法 JSON 返回错误。
pub fn parse_collab_ref(s: &str) -> Result<MessageReference, String> {
    let stripped = s
        .strip_prefix(COLLAB_REF_PREFIX)
        .ok_or_else(|| format!("COLLAB_REF_INVALID: missing prefix {COLLAB_REF_PREFIX}"))?;
    serde_json::from_str(stripped).map_err(|error| format!("COLLAB_REF_INVALID: {error}"))
}

#[cfg(test)]
mod federation_wire_tests {
    use super::*;

    #[test]
    fn put_roundtrip_preserves_origin_target_and_body() {
        let value = serde_json::json!({
            "kind": "put",
            "value": {
                "origin": {
                    "machine": "studio",
                    "project_scope": "/projects/alpha",
                    "app_scope_id": "appserver-alpha",
                    "operation_id": "op-1"
                },
                "sender": {"kind": "peer", "value": {
                    "address": {"machine": "studio", "local": {"project": "alpha", "name": "Alice"}},
                    "project_scope": "/projects/alpha",
                    "app_scope_id": "appserver-alpha",
                    "stable_name": "Alice"
                }},
                "target": {"kind": "named", "value": {"project": "alpha", "name": "Bob"}},
                "mtype": "message",
                "subject": "联邦消息",
                "body": "第一行\nsecond line",
                "delivery": "immediate"
            }
        });
        let request: FederationReq = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), value);
    }

    fn origin_ref() -> OriginMessageRef {
        OriginMessageRef {
            machine: "machine-a".to_owned(),
            project_scope: ProjectScopeId::new("/projects/alpha".to_owned()).unwrap(),
            app_scope_id: AppServerId::new("appserver-alpha".to_owned()).unwrap(),
            operation_id: OperationId::new("op-1".to_owned()).unwrap(),
        }
    }

    fn message_ref() -> MessageRef {
        MessageRef {
            machine: "machine-a".to_owned(),
            project_scope: ProjectScopeId::new("/projects/alpha".to_owned()).unwrap(),
            app_scope_id: AppServerId::new("appserver-alpha".to_owned()).unwrap(),
            msg_id: MessageId::new("msg-1".to_owned()).unwrap(),
        }
    }

    #[test]
    fn collab_ref_roundtrip_origin() {
        let reference = MessageReference::Origin(origin_ref());
        let encoded = format_collab_ref(&reference).unwrap();
        assert!(encoded.starts_with(COLLAB_REF_PREFIX));
        let decoded = parse_collab_ref(&encoded).unwrap();
        assert_eq!(decoded, reference);
    }

    #[test]
    fn collab_ref_roundtrip_message() {
        let reference = MessageReference::Message(message_ref());
        let encoded = format_collab_ref(&reference).unwrap();
        let decoded = parse_collab_ref(&encoded).unwrap();
        assert_eq!(decoded, reference);
    }

    #[test]
    fn collab_ref_rejects_missing_prefix() {
        assert!(parse_collab_ref("no-prefix:{}").is_err());
        assert!(parse_collab_ref("collab-ref:").is_err());
    }

    #[test]
    fn collab_ref_rejects_illegal_json() {
        assert!(parse_collab_ref("collab-ref:{not-json").is_err());
        assert!(parse_collab_ref(&format!("{COLLAB_REF_PREFIX}{{}}")).is_err());
    }

    #[test]
    fn federation_envelope_compiles_without_debug() {
        let _envelope = FederationEnvelope {
            target: "machine-a".to_owned(),
            credential: "credential".to_owned(),
            request: FederationReq::Hello,
        };
        // FederationReq 本身可 Debug；FederationEnvelope 不 derive Debug/Display，
        // 凭据字段不会通过诊断格式暴露。
        let _ = format!("{:?}", _envelope.request);
    }
}
