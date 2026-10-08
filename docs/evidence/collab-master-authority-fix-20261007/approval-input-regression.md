# Clear approval input regression

Clock receipt: 2026-10-08 08:11:26 UTC. Base: 7350fbf6b020b1464d531337c7b2f6b8fa5de6f2 plus task candidate. Accepted contract section 2 already rejects control characters in approval; promote enforces this through existing MasterGrant validation.

The parent created one isolated CLI consumer root `/tmp/cma-approval.Y1q1EY`, host state `h`, own tmux socket `t.sock`, and observed tmux server PID18306/pane%0. Actual CLI context registered its own peer. No shared daemon or existing project authority was changed.

After `master promote --approval 'user approved isolated input regression'`, the public command `master clear --approval $'approved\nforged'` incorrectly returned exit0 and cleared the holder. The raw successful response is `approval-control-red.json`.

The parent added the missing `char::is_control` rejection to handle_master_clear, rebuilt the bins with actual exit0, and used isolated `collab down`/`collab up` to load that binary. After promoting the same isolated peer, the identical clear request returned actual exit1 and the explicit error in `approval-control-green.stderr`. `approval-green-status.json` proves the original holder and approval remained. A subsequent clear with ordinary nonempty approval returned exit0.

The final isolated down succeeded; the owned tmux socket received kill-server. The socket was released, PID18306 was absent, and only this owned fixture was removed. `test ! -e /tmp/cma-approval.Y1q1EY` returned exit0. Permanent DSH public approval coverage belongs to the GCM consumer worker. This incremental regression is not full candidate, installed runtime, review or main acceptance.
