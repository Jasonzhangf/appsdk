# Proven legacy SDK bundle transition

This note records the source identity used by the `0.1.5-to-0.1.6` migration
validator for legacy records whose lock no longer retains the authoring bundle.
The table is a Rust constant, not a bundle resource, so the SDK bundle digest
and `0.1.0015` version remain unchanged.

## Bundle algorithm

`sdk_bundle_digest()` hashes the bundle manifest bytes and each manifest
resource with its path and class. The digests below were recomputed from SDK
source history with that algorithm. No consumer record or lock digest was used
as an input.

## Transition

- Step: `0.1.5-to-0.1.6`
- Authoring SDK commit: `99ca3788b4fe3ca4008600f14378d418864513bd`
- Legacy record bundle:
  `sha256:4813363da77e9eec678bb7e4bbc8664beec920479aa61677c5991d4e45f82018`
- Canonical target SDK commit:
  `916cf9d88e32304f52aa1cb3ecbfa86b222b1805`
- Target bundle:
  `sha256:b3bd7c8ef3b44790f6cf50169ef7a42d61a20b66747e1e5c71b9a348f945ccb4`

## Canonical target tuple

| Governance map | SHA-256 |
|---|---|
| `resource-map.json` | `sha256:373f7121a351f87c7126c7b163784190a0c5393edc9ddccb5c776a9c1224246b` |
| `function-map.json` | `sha256:c0fbf20f6e697e3ea71d424f9d550eb22821eefeee2d7568447f2689a215c0d6` |
| `mainline-call-map.json` | `sha256:d8964f67c3d7e51e5131a0a5fffc011462198231328f7e5644de5740095ca8f6` |
| `verification-map.json` | `sha256:f873ccad5a2590a1cec2a516a1ef9ebda36edab4023f18e4d69e211fe3ce7668` |

An independently retained downstream lock at commit
`5354e90d144c05b7c224768673642b4312646f97` preserves
`previous_bundle_digest=4813363d...` and target bundle `b3bd7c8e...`.
Its legacy migration record and four snapshots are byte-identical to the
accepted migration input. This supports the historical transition but is not
itself the authorization source.

## Trust boundary

The validator grants an unanchored historical record authority only when all
four maps together match the proven transition: null canonical bindings, the
declared source digest, and the proven target digest for each map. A mixture of
historical and current targets, or an explicit custom binding, is not the proven
tuple and is rejected with a typed failure. The same read-only validation runs
before any consumer write.
