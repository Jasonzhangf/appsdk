**The mixed snapshot is possible inside `handle_context`, but the inspected public routed dispatch path serializes the proposed `Context`/`MasterClear` interleaving.** I did not run a public reproduction. This is a source diagnosis, with no verdict or implementation.

The local split is clear in [part_09.rs](/Volumes/Intel/playground/appsdk/collab-master-authority-fix-20261007/collab/src/server/mod_parts/part_09.rs:1051):

- Line 1080 captures `current_role_brief`. `identity.role` and top-level `authority` derive from that captured value.
- Lines 1158–1163 capture peer roles.
- Line 1178 releases the state mutex.
- Line 1220 calls `worker_presence_with_view`. DSH presence calls the real gateway `agent-facts` endpoint.
- Lines 1261–1271 acquire the state mutex again and project the **then-current** master grant.

If an authority mutation could commit during those probes, the first inconsistent read would be `current_master_grant` at line 1263. `master_authority_view` at line 1271 would then serialize the changed grant beside the old role fields. Clear could yield `identity.role == "master"` with `master == null`. Replacement could yield the old holder’s master role beside a different holder.

However, another owner prevents that public interleaving.

In [part_04.rs](/Volumes/Intel/playground/appsdk/collab-master-authority-fix-20261007/collab/src/server/mod_parts/part_04.rs:1078), `ProjectRuntimeManager::dispatch_sync` selects an existing runtime with:

```rust
if let Some((runtime, _)) = self.routes.lock().unwrap().get(&key).and_then(...) {
    // validation
    // dispatch_with_route_context(...)
    // finalize_registration(...)
    // return
}
```

The temporary `MutexGuard` in that `if let` scrutinee remains alive through the successful branch. Cloning the runtime does not release it. The branch calls `handle_context` while holding the manager’s `routes` mutex. A concurrent public `MasterClear` must pass through the same runtime lookup, so it cannot reach its handler until context finishes that branch.

The routed connection code in [part_12.rs](/Volumes/Intel/playground/appsdk/collab-master-authority-fix-20261007/collab/src/server/mod_parts/part_12.rs:205) starts independent blocking dispatch tasks. That permits concurrent connections, but does not remove the manager mutex. The separate non-routed `dispatch_wire` gate excludes `Context`; that fact alone does **not** establish concurrency on the fixture’s routed daemon path.

**No source fix is justified for the proposed public clear/replace race on this path.** This does not establish the full B15 invariant. It only disproves this particular interleaving by source inference. If the manager lock is later shortened, or another authority writer bypasses it, the local split will need attention.

For that conditional case, the smallest change is to capture the existing `Option<MasterGrant>` and serialize its authority projection under the initial state lock, using `master_authority_view`. After probes, update only the existing `endpoint_live` observation for that captured holder. Do not reselect authority under the later lock. Keep the deliberate later scheduling reads separate. This needs no epoch, schema change, fallback, or new serializer.

A concrete public barrier experiment can confirm the serialization:

1. Use the existing DSH fixture setup. Register A and B through the public socket. Save their returned binding contexts. Promote A with an approved public `MasterPromote`. Finish setup before arming the barrier.

2. Send this NDJSON request on a dedicated daemon connection:

   ```json
   {
     "op": "Context",
     "worker_id": "workerA",
     "token": "tokenA",
     "project_context": "<ctx_a object from binding_context(reg_a, root)>"
   }
   ```

   Replace the placeholder with the actual object. Do not send it as a string.

3. In the gateway callback, accept an `agent-facts` request for `params.agentId == "agentA"`. Retain that stream and signal a channel barrier before writing its reply. Release it through an explicit channel message. Return the existing successful response shape, echoing the request’s `nonce`, `runtimeId`, and `agentId`, with `sessionId: "agentA"`, the fixture root, and `status: "running"`.

4. Once the callback is held, send this request on a **second** daemon connection:

   ```json
   {
     "op": "MasterClear",
     "worker_id": "workerA",
     "token": "tokenA",
     "approval": "fixture approved concurrent clear",
     "project_context": "<same ctx_a object>"
   }
   ```

   `handle_master_clear` itself does not consult liveness. The obstacle is the outer manager lock.

5. Observe receipt ordering, then release the callback. The source predicts that clear cannot complete while context owns that branch. Context should return the pre-clear authority projection; clear should complete afterward. A fresh public context should return the post-clear projection.

Do **not** wait indefinitely for clear success before releasing the callback. Under the inspected code, that creates a test deadlock. A bounded timeout can protect the experiment, but timeout alone does not prove lock ownership. Likewise, two coherent replies alone do not prove all concurrency cases.

The callback has an attribution hazard. The existing gateway serves connections serially, and `agent-facts` carries runtime, agent, and nonce fields—not the originating Collab operation. Registration and other presence calls can also generate facts requests. The daemon also starts a background timer scheduler. I did not inspect its probe behavior. A test must establish that the held call belongs to the target context; it must not assume “the next facts request” is sufficient. A held serial callback can also stall unrelated gateway work.

**Evidence status:** The split reads, manager lock scope, public request shapes, and clear’s lack of liveness probes are source facts. The resulting public serialization is source inference. Callback attribution, receipt ordering, and the full B15 behavior remain untested here. No files or runtime state were changed.