# AppSDK and Collab Command Surface

Use these commands instead of guessing paths or running broad help exploration.
This list is the decision path for normal setup, quality, delivery, and Collab
work.

## AppSDK commands

```text
appsdk prepare                         create or print .appsdk-prepare.json
appsdk init .                          scaffold/refresh governance and register project
appsdk init . --fresh --discard-legacy reset old AppSDK control plane and rebuild
appsdk reset-governance . --discard-legacy
                                       lower-level AppSDK control-plane reset
appsdk new <dir>                       create an empty new governed project
appsdk verify .                        verify current governance contract/baseline
appsdk compile .                       compile project modules
appsdk compile-module . --module <id>  compile one module
appsdk pin-lock . --binary <path>      pin project to a binary and write sdk.lock
appsdk guide status                    read compiled guidance status
appsdk guide compile                   compile declared guidance after contract binding
appsdk guide init --task <id> --mode <mode> --module <module-id>
                                       create a task-specific guide plan
appsdk goal subscribe --goal <file.md> --interval <interval>
                                       master-only long-horizon registration
appsdk goal status --json              verify goal subscription state
appsdk longhorizon show --json         read long-horizon role/task state
appsdk bug list -q <kw> --json         query bug backlog
appsdk bug new -t <title> -m <body> -l <labels>
                                       create a bug record
appsdk bug new --upstream ...          report an AppSDK defect upstream
appsdk bug show <id> --json            read a bug record
appsdk bug close <id> -m <solution> --receipt-id <receipt>
                                       close a bug with solution evidence
appsdk subagent start --id <id>        start a managed subagent
appsdk subagent status                 inspect managed subagents
appsdk subagent send <id> --subject <topic> "<assignment>"
                                       dispatch a managed subagent task
appsdk subagent close <id>             close a managed subagent
appsdk subworker <action>              compatibility entry for Collab subagent flow
appsdk communication reset-runtime-registry --discard-legacy --approval "<text>"
                                       archive legacy ~/.appsdk/runtimes.jsonl and rebuild current baseline
project-memory entry                   append memory entry and regenerate projections
project-memory reentry [project] --run <run-id>
                                       resume a memory run after interruption
```

## Collab commands

```text
collab init                            register this peer once through AppSDK or standalone
collab context                         read current peer/role contract
collab sendmessage --to <peer> --subject <topic> "<body>"
                                       send one durable ordinary message
collab recv                            consume delivered notifications
collab inbox                           list unread messages (read-only)
collab msg <id>                        read one message without consuming
collab ack <id> | --all                compatibility ACK for delivered messages
collab master status                   show live master state
collab master promote --approval "<text>"
                                       promote this peer when user approves and no master exists
collab master delegate <peer>          transfer live master authority
collab master send --project <target> --to <target-master> --subject <topic> "<body>"
                                       cross-project master-to-master send
collab subagent dispatch --request-id <id> --subject <topic> "<assignment>"
                                       scheduler-reserved dispatch
collab task accept <task-id>           accept assigned task (assigned -> working)
collab task update --status <state>    update task state
collab task block <id>                 block with concrete cause/owner/unblock condition
collab task close <id> [--force --reason "<reason>"]
                                       close owned task
collab migrate inspect                 read-only migration/retirement inspection
collab migrate plan                    prepare migration/retirement snapshot
collab migrate apply                   freeze admission and persist snapshot
collab migrate verify                  verify migration/retirement continuity
collab reset --discard-legacy --approval "<text>"
                                       retire/rebuild Collab-owned local control plane
collab down                            controlled daemon stop
collab up                              controlled daemon start
collab worker recover                  rebind/recover worker identity after restart
```

The commands below are diagnostic-only. They are not initialization steps and
must not be chained after `collab context` during normal setup. The explicit
stale-daemon recovery procedure in
[`init-prompts.md`](init-prompts.md#stale-daemon-socket-or-lock) is the only
exception: there, preserve the exact failure, then use `collab status --all`
before the controlled `collab up` and final `collab context`.

```text
collab status --all                    server summary and worker/task state
collab who                             registered peers and liveness
collab worker status <peer-id>         one peer's identity/liveness/transport
collab notify status                   own subscriptions
```

## Do not guess

- Do not edit `~/.appsdk`, `~/.collab`, `.appsdk/`, `.appsdk-control/`, or
  `.agent-collab/` by hand.
- Do not delete project directories to clean global truth.
- Do not use `--help` exploration as the setup path; read the referenced
  lifecycle docs when a command's exact flag matters.
- If a command returns an error, preserve the exact error and report it.
  Never claim a route, migration, merge, install, restart, or delivery from
  command output alone.
