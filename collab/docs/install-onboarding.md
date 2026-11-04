# Collab first install: permissions, Git, tmux, and QQBot

This page is for a new machine or a first install of the Collab CLI and global
daemon. It records the issues that appeared during the first `jenny` install on
2026-10-30 and the smallest setup that made the real Codex TUI work.

## Install and activation

From the AppSDK checkout:

```sh
scripts/install-global-collab.sh
```

The installer installs `collab`, `collab-mcp`, the `collab-desktop-mcp` helper
and its `.mjs`, and refreshes the Collab Skill. It does not load or start the
daemon, does not call `launchctl`, and does not create the daemon socket.

Daemon activation belongs to `collab up`. Run it from the project root that
Collab should bind:

```sh
~/.cargo/bin/collab up
```

`collab up` writes the service record for the current project first, then
installs and loads the per-user LaunchAgent as a side capability. The
LaunchAgent label is `com.appsdk.collab` and it uses `RunAtLoad` plus
`KeepAlive.SuccessfulExit=false`. If autostart fails while a daemon was already
serving, `up` still reports `ok` and `started:false` instead of disturbing the
running daemon. If no daemon is serving, a real launchd or plist failure exits
non-zero.

Verify the installed binary, service, and socket:

```sh
~/.cargo/bin/collab --version
test -S ~/.collab/server.sock && echo socket-ready
launchctl print "gui/$(id -u)/com.appsdk.collab" | grep -E 'state =|pid ='
```

The installer does not start or restart a running daemon. If a daemon was
running before the install, it keeps the old binary until the next user-managed
maintenance window.
Do not use `launchctl kickstart -k` for Collab as a normal install step.

## Install target must be on the system data volume

On macOS the installer refuses to write the Collab binaries or the Skill bundle
onto an external volume. It resolves each install target to its real path
through any symlinks and compares the target's device with the writable system
data volume `/System/Volumes/Data`. Do not point `CARGO_HOME`, the Skill target,
or a symlink in their path at a volume under `/Volumes/<external>`; the install
fails before writing. The check is macOS-only: on Linux Collab reports its
support scope and does not guess at physical devices.

## Permissions: YOLO is not required

Do not use `--dangerously-bypass-approvals-and-sandbox`, do not select
`--sandbox danger-full-access`, and do not add a `yolo` rule. Collab needs only
two controlled grants:

- write access to the Collab state root `~/.collab`
- permission to connect to the Collab daemon Unix socket
  `~/.collab/server.sock`

Put one named profile in `~/.codex/config.toml` and make it the default.
Replace `/Users/<you>` with the absolute home directory shown by `echo $HOME`.

```toml
default_permissions = "collab"

[permissions.collab]
extends = ":workspace"

[permissions.collab.filesystem]
"/Users/<you>/.collab" = "write"

[permissions.collab.network]
enabled = true

[permissions.collab.network.unix_sockets]
"/Users/<you>/.collab/server.sock" = "allow"
```

The profile on the `jenny` Air machine used `collab-air` as its name with the
same three grants. The exact name is not special; the grants are. A project
does not need a separate permission profile. `collab init` writes the project
MCP entry and removes the deprecated `sandbox_mode` and `approval_policy` keys,
but it does not write a global `~/.codex` profile. Add the profile once during
first install.

## Git is not a Collab requirement

Older Collab builds called a Git subprocess from `collab context`. That could
fail or hang outside a Git repository and made first registration appear to
require Git. Current Collab does not read the Git repository to resolve a
project or an identity. `collab context` uses the real process cwd and the
host-provided Codex runtime facts.

Do not create a Git repository just to make Collab register. A non-Git
directory is valid. A minimal check is:

```sh
cd "$(mktemp -d)"
collab context
```

The response must not contain `COLLAB_CONTEXT_UNRESOLVED` or a Git failure. It
may still report the normal missing Codex identity fields when the command is
run outside a Codex TUI.

## tmux is placement, not identity

Older builds could return `TMUX_OBSERVATION_UNAVAILABLE` when tmux placement
metadata was absent or malformed. Current Collab tolerates missing or malformed
placement metadata. tmux session, pane, socket, and server PID are only used
to locate delivery and observation. They are never the stable work identity.

For the first registration, run `collab context` inside the real Codex TUI.
Do not run it from a plain SSH shell or another terminal and expect the same
runtime facts. The Codex TUI supplies the real session and thread facts that
registration needs. Do not restart tmux, change `TMUX`, or inject a socket
path. Collab does not own the tmux server lifecycle.

## QQBot receiver and first bind

The receiver is the single gateway process at
`~/.local/share/qqbot-channel/receiver.mjs`. The status command is:

```sh
node ~/.local/share/qqbot-channel/qqbot.mjs status
```

It must show `credentials=present`, `target=bound`, and `connection=ready`.
Credentials and AppSecret values must never be printed in a chat or in these
docs.

The first target binding needs one private C2C QQ message to the bot. The
receiver opens a ten-minute binding window when it starts. Send the message,
then check:

```sh
cat ~/.local/share/qqbot-channel/state/target.json
cat ~/.local/share/qqbot-channel/state/receiver-status.json
```

If the window expired, reopen it with the LaunchAgent only:

```sh
launchctl kickstart -k "gui/$(id -u)/com.appsdk.qqbot-channel"
```

Send the private message again within ten minutes. Do not start a second
receiver, do not start a second gateway, and do not use the Codex window as
the QQ reply channel. A QQBot reply goes through:

```sh
node ~/.local/share/qqbot-channel/qqbot.mjs send --file /tmp/reply.txt
```

The first line of the reply file must carry the sender prefix, for example
`[CEO]` or `[<project>.<name>]`.
