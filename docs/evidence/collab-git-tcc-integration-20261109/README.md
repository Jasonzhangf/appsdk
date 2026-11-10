# Launchd Git child TCC block: live receipts

## Incident

Live daemon `0.2.0448` (launchd, PID `63957`,
`/Users/fanzhang/.local/bin/collab serve --service`) blocks integration-path
requests such as `collab recv` / `collab task integrated`.

## Raw receipts

- `live-git-child.txt`: daemon and its stuck git children, plus the child cwd.
- `raw-stacks.txt`: git child stack and the daemon child-wait frames.

Key facts captured read-only:

- Git child PID `36286`, PPID `63957`, elapsed over 20 minutes.
- Executable: `/Applications/Xcode.app/Contents/Developer/usr/bin/git`.
- Args: explicit
  `--git-dir=/Users/fanzhang/Documents/github/appsdk/.git` and
  `--work-tree=/Users/fanzhang/Documents/github/appsdk`, running
  `rev-parse --verify refs/heads/main^{commit}`.
- `lsof` confirms child cwd is `/`.
- Child stack (3s sample, 2412 frames all at top):
  `cmd_rev_parse -> setup_git_directory_gently -> setup_explicit_git_dir ->
  is_git_directory -> git_open_with_retry -> open -> __open`.
- Daemon stack shows `posix_spawn`/`__posix_spawn` and `__wait4` frames.

## Root cause

`git_dir_for_root` runs inside the daemon process before the spawn. Because
the git child exists, those in-process `.git` marker reads (stat, and marker
file read for worktrees) completed without blocking. The block occurs only in
the git child:

1. The child binary is Xcode `git`, a different executable from the daemon.
2. It does not hold the daemon's TCC/FDA grant.
3. Git still opens the protected Documents `.git` path even with cwd `/` and
   explicit `--git-dir`; that `open()` does not return.

This is why `current_dir("/")` plus explicit git-dir/work-tree cannot fix the
block. The daemon's own in-process filesystem reads on the same cwd succeed
(`collab context` returns immediately from
`/Users/fanzhang/Documents/github/appsdk`), which is the property the gix fix
relies on: read the repo in-process inside the authorized daemon instead of
forking an unauthorized `git` child.

## Fix status

- Commit `4712967c`: `fix(collab): resolve integration git queries in-process
  via gix`.
- PR `#139` head `4712967c`, CI green (`AppSDK full`, `Windows AppSDK`,
  packaging, docs).
- Live activation is UNVERIFIED: global install and daemon restart were
  forbidden in this task and require separate user approval.
