# Zone Transition Matrix

| From | To | Rule | Owner |
|---|---|---|---|
| Playground | Active | Only through mainline merge, review PASS, compile, and publish | promotion/compiler adapters |
| Playground | Protected | Only as audited experiment/history archive | VCS adapter |
| Active | Protected | Only through version lock with source/API/artifact evidence | freeze adapters |
| Protected | Playground | New change workspace/snapshot only; never in-place edit | workspace adapter |
| Generated | Active | Only verified compiler publish; generated files are not source | compiler/publish adapter |
| Playground | Runtime | Forbidden | architecture gate |
| Protected | Runtime | Forbidden | architecture gate |
| Generated | Runtime | Only verified Active library/artifact consumption | runtime adapter |

Protected source is historical input, not runtime input. Active library is the current consumption surface.

The machine source is `contracts/transitions/zone-transition-manifest.json`. It must enumerate all 16 ordered pairs across the four zones, including forbidden pairs. Every allowed edge declares requirements, owner adapter, runtime policy, artifact requirement, and record types. Forbidden runtime edges cannot be overridden by a project record.
