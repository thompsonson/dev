# Workflow Service — design doc

Status: draft, pre-decision.
Audience: decision-makers across `dev`, `atomicguard`, `chops`.
Outcome the doc seeks: **an agreement on which repo owns a small service that brokers between voice/text utterances and registered workflows** — and a concrete four-capability API that lets us start building regardless.

## Context

The AtomicGuard PoC on pop-mini (documented in `thompsonson/atomicguard` `docs/masters_report/chapters/08_poc.tex`) is live:

- Chops captures voice, transcribes via whisper-rs, publishes to MQTT `voice/transcriptions`.
- Agent-core (a Python service) runs an embedding classifier, a parameter extractor, and a `WorkflowEffector` over a static catalogue of 23 workflows, publishes events to `agent/workflow/events` and escalations to `agent/workflow/escalation`.
- "Something's wrong" → 3.7 s → actionable diagnostic on the phone.

Today the workflow catalogue is **baked into agent-core at startup**. Adding a new workflow requires editing agent-core. This locks workflow authorship to one repo and one language, and prevents `dev`, a monitoring-ops service, a future `pi.dev` daemon, or any other domain-owner from contributing workflows they naturally own.

The question this doc answers: what is the minimal service that lets any domain-owner register workflows and have them reachable from the voice/text front-end?

## Scope

A service with **four capabilities, nothing more**:

1. **Workflow Registration** — accept and catalogue workflow manifests with their intent examples.
2. **Intent → Workflow Matching** — take an utterance, return a workflow name plus extracted parameters.
3. **Workflow Dispatch** — invoke a named workflow with parameters.
4. **Workflow Response Handling** — stream events, final results, and escalations back; accept feedback for in-flight escalations.

Workflows are opaque to the service. What they contain (action pairs, generators, guards, the specifics of execution) is none of the service's business. This is intentional — the service is plumbing, not a workflow runtime. It calls the workflow runtime (`WorkflowOrchestrator` in atomicguard) as a library.

### Explicit non-goals

| Out of scope | Why |
|---|---|
| The internals of workflows (APs, guards, retries, backtracking) | Workflow runtime's job |
| A new workflow definition language | Manifests are opaque bytes; today workflow.json + ap_context.json, tomorrow DS-PDDL |
| Transport-specific effector code | Effectors are part of a workflow's manifest, not the service's API |
| Authentication / authorisation beyond filesystem permissions | v1 is single-user on pop-mini; multi-tenant comes later |
| Persistence of artifact DAGs | The workflow runtime owns R; the service only tracks dispatches |

## The meta-workflow lens — apply before choosing a repo

Before deciding where this service lives, notice what's actually in it. The PoC's meta-workflow (per `docs/design/plans/system-view-intent-parsing.md`) is already three action pairs:

- **A_intent** ⟨embedding classifier, IntentGuard⟩
- **A_decompose** ⟨LLM param extraction, schema guard⟩
- **A_dispatch** ⟨WorkflowEffector, WorkflowStatusGuard⟩

That is a valid atomicguard workflow. Registering a child workflow = adding to Π_task, which `WorkflowEffector` reads. **The orchestration half of "Layer 2" is already expressible in atomicguard's own vocabulary.** The service doesn't need to reimplement it — it runs Π_meta per utterance, same way any other workflow runs.

Strip the meta-workflow out and see what's actually left as service-specific code:

| Capability | Where it lives |
|---|---|
| Intent → workflow matching (classifier + guard) | Inside **A_intent** AP |
| Parameter extraction (LLM + schema validation) | Inside **A_decompose** AP |
| Dispatch (invoke child workflow) | Inside **A_dispatch** AP via `WorkflowEffector` |
| MQTT subscribe/publish | **Residual plumbing** |
| HTTP `/workflows` endpoints | **Residual plumbing** |
| TTL + heartbeat on registrations | **Residual plumbing** |
| Classifier index maintenance (centroid rebuild) | **Residual plumbing** |
| Event loop feeding utterances into meta-workflow runs | **Residual plumbing** |

The residual is **~500 LoC of glue**. Everything else lives inside Π_meta, shipped as a JSON pair in the catalogue — same file format and runtime as any workflow in Π_task.

**Implication for the repo-placement question below:** it isn't "where does the workflow service live" — it's "where do ~500 LoC of MQTT + HTTP + catalogue glue live, given atomicguard does the actual orchestration work?" A much smaller question. Apply this lens first and the answer falls out cleanly.

## Architecture

```
                    ┌─────────────────────────────────────────┐
                    │  AtomicGuard Workflow Service           │
                    │                                         │
    utterances ────▶│  ① Match                                │
    (chops/MQTT)    │     utterance → (workflow, params)      │
                    │                                         │
    manifests  ────▶│  ② Register                             │
    (registrants)   │     workflow catalogue + classifier     │
                    │     index (TTL + heartbeat)             │
                    │                                         │
                    │  ③ Dispatch                             │
                    │     invoke WorkflowOrchestrator         │
                    │     with manifest + params              │
                    │                                         │
                    │  ④ Respond                              │
                    │     events/result/escalations out,      │
                    │     feedback in                         │
                    │                                         │
                    └─────────────────────────────────────────┘
                      ▲                │                ▲
                      │                │                │
       register       │       events   │                │ dispatch_id +
       POST /workflows│       via MQTT │                │ feedback in
                      │                ▼                │
         ┌────────────┴──┐    ┌─────────┴───┐    ┌──────┴────────┐
         │  registrants  │    │  callers    │    │ humans        │
         │               │    │             │    │ (via chops)   │
         │  dev          │    │  chops      │    │               │
         │  monitoring   │    │  cron/other │    │               │
         │  pi.dev?      │    │  CLI        │    │               │
         └───────────────┘    └─────────────┘    └───────────────┘
```

Two populations interact with the service:

- **Registrants** bring workflows in. A registrant owns its domain — `dev` owns tmux operations, monitoring-ops owns the observability stack, chops owns voice-input workflows, etc. Each pushes its manifests at startup and heartbeats to keep them alive.
- **Callers** push utterances / explicit dispatch requests in. Chops is today's only caller; anything with an MQTT or HTTP client can be one tomorrow.

## Capability detail

### 1. Workflow Registration

```
POST   /workflows               register or replace
GET    /workflows               list
GET    /workflows/:name         detail
POST   /workflows/:name/refresh heartbeat (extends TTL)
DELETE /workflows/:name         retire
```

**Register payload:**

```json
{
  "name": "restart_service",
  "manifest": { "...": "workflow.json + ap_context.json (opaque)" },
  "spec_schema": {
    "service": { "type": "string", "required": true }
  },
  "intent_examples": [
    "restart {spec.service}",
    "bounce {spec.service}",
    "kick {spec.service} please"
  ],
  "owner": "dev-daemon@pop-mini",
  "ttl_seconds": 300
}
```

The `manifest` is data the service stores verbatim and hands to the workflow runtime at dispatch time. The service does not inspect its internals.

On register:
1. Validate the envelope (name, manifest present, intent_examples non-empty).
2. Store in the registry.
3. Trigger classifier re-index (Section 2) — may be debounced.
4. Respond 201 with the stored record.

On retire or TTL expiry:
1. Remove from registry.
2. Re-index classifier.
3. In-flight dispatches keep their manifest (snapshot at dispatch time).

### 2. Intent → Workflow Matching

```
POST /match
body:   { "utterance": "restart chops web" }
return:
  {
    "workflow":   "restart_service",
    "confidence": 0.967,
    "params":     { "service": "chops-web" },
    "candidates": [
      { "workflow": "service_status",   "confidence": 0.812 },
      { "workflow": "service_recovery", "confidence": 0.734 }
    ]
  }
```

Implementation mirrors the existing embedding classifier in `examples/sysadmin/intent_embedding.py`:

- Centroid vector per workflow, computed from `intent_examples`.
- Cosine similarity at query time.
- Optional LLM fallback when top-1 confidence is below a threshold (Tier 3).
- LLM parameter extraction when the matched workflow has a non-trivial `spec_schema` and the utterance contains slot values (Tier 2).

Re-index triggers:
- Register → add centroid, amortise into the matrix on debounce.
- Retire / TTL expiry → remove centroid.
- No request-path rebuilds.

### 3. Workflow Dispatch

```
POST /dispatch
body:
  {
    "workflow":        "restart_service",
    "params":          { "service": "chops-web" },
    "conversation_id": "uuid",
    "reply_channel":   "mqtt://agent/workflow/events"  // optional override
  }
return:
  { "dispatch_id": "uuid", "status": "running" }
```

Convenience composition endpoint:

```
POST /run
body:   { "utterance": "...", "conversation_id": "..." }
# = /match then /dispatch. Returns dispatch_id + matched workflow + confidence.
```

Under the hood: load the manifest from the registry, instantiate a `WorkflowOrchestrator`, run it in a worker, stream events to the reply channel.

### 4. Workflow Response Handling

Two directions. Events/results/escalations flow **out**; escalation feedback flows **in**.

**Out** — the service publishes to MQTT topics (chops-compatible):

- `agent/workflow/events` — `StepStarted`, `StepSatisfied`, `StepFailed`, `EffectorExecuted`, `Backtracking`, `WorkflowComplete`, `WorkflowFailed`. QoS 0.
- `agent/workflow/escalation` — `EscalationRequired` messages carrying `dispatch_id`, `conversation_id`, step context, and the prompt for the human. QoS 1+.

Every message carries `dispatch_id` and `conversation_id` so a caller can correlate.

HTTP equivalents for non-MQTT callers:

```
GET  /dispatches/:id             snapshot: status + final result (if complete)
GET  /dispatches/:id/events      SSE stream or paged list
```

**In** — escalation feedback:

```
POST /dispatches/:id/feedback
body:
  {
    "specification": "run stack tdd",
    "feedback":      "the previous attempt missed the edge case",
    "approve":       true
  }
```

Or MQTT equivalent on `agent/workflow/feedback` with `dispatch_id` correlation — matches the topology chops already uses.

When feedback arrives, the service resumes the paused workflow by injecting the feedback/specification into the `WorkflowOrchestrator`'s escalation hook and the orchestrator continues from where it paused.

## State the service holds

```
┌─ Registry ─────────────────────────────────────┐
│ name → {                                       │
│   manifest, spec_schema, intent_examples,      │
│   owner, ttl_expires, centroid_vector,         │
│   content_hash  (for change detection)         │
│ }                                              │
└────────────────────────────────────────────────┘

┌─ Classifier index ─────────────────────────────┐
│ Matrix of centroid vectors keyed by name.      │
│ Rebuilt incrementally on register/retire.      │
└────────────────────────────────────────────────┘

┌─ Dispatches ───────────────────────────────────┐
│ dispatch_id → {                                │
│   workflow_name, manifest_snapshot, params,    │
│   conversation_id, reply_channel,              │
│   status  (running|awaiting_feedback|done|failed), │
│   events[], result?, pending_escalation?       │
│ }                                              │
└────────────────────────────────────────────────┘
```

Registry and classifier index can be reconstructed from the registered manifests (which registrants re-POST on startup). Dispatch state is in-memory only; the workflow runtime's Repository is the durable artifact record.

## Sequence: voice utterance to completion

```
chops         service        runtime          registrant (e.g. dev)
 │              │               │                      │
 │              │◀──────────────┼── POST /workflows   (at daemon startup)
 │              │               │                      │
 │ voice →      │               │                      │
 │ MQTT utter   │               │                      │
 │──────────▶   │               │                      │
 │              │ ① match       │                      │
 │              │ ② dispatch    │                      │
 │              │──────────────▶│ run workflow X       │
 │              │               │ (APs run; effectors  │
 │              │               │  inside the workflow │
 │              │               │  do their thing,     │
 │              │               │  which may call back │
 │              │               │  to the registrant's │
 │              │               │  endpoint — not the  │
 │              │               │  service's concern)  │
 │◀─────────────│◀──────────────│ MQTT events          │
 │              │               │                      │
 │◀─────────────│◀──────────────│ MQTT escalation      │
 │ human        │               │                      │
 │ types spec   │               │                      │
 │──────────────▶ POST /feedback │                      │
 │              │──────────────▶│ resume               │
 │◀─────────────│◀──────────────│ MQTT final result    │
 │              │               │                      │
```

The service never knows how the workflow talks to its effectors. That's the runtime + manifest's problem.

## Where should this service live? — the decision this doc is asking for

Five options. Ranked roughly by my read; adjust to taste.

### Option A: `thompsonson/atomicguard` (Python, main repo)

**Pro** — The workflow runtime already lives here; `mqtt_workflow_service.py` is a direct precursor; the embedding classifier exists in `examples/sysadmin/intent_embedding.py`. Adding the registry + `/match` + `/dispatch` endpoints is incremental — you'd refactor `mqtt_workflow_service.py` into a more generic service that reads from a dynamic registry instead of a static catalogue.

**Pro** — Single release cadence. When atomicguard's workflow runtime evolves, the service evolves with it.

**Con** — Mixes "the library / framework" with "a running service daemon." Users who want atomicguard purely as a library for their own orchestrators will see service code in the same repo.

**Con** — Python is the language; if you later want a tiny static Rust binary for deployment, you're doing a port.

**Verdict:** Natural home for v1. Lowest time-to-working.

### Option B: `thompsonson/atomicguard-rs` (Rust, the new repo)

**Pro** — Clean service boundary, small binary, nice deploy story (matches `dev` daemon's existing ergonomics). Single-threaded HTTP-over-UDS is a pattern we already know works.

**Pro** — Separates service from library.

**Con** — atomicguard-rs doesn't yet have the workflow runtime. Missing `CommandTemplateGenerator`, effector `undo`, `WorkflowEffector`, DS-PDDL parser. Building this here today means either reimplementing the runtime (months of work), or calling into Python atomicguard as a subprocess (fragile).

**Con** — Embedding classifier would need a Rust path (Ollama HTTP client is fine, but that's one more thing).

**Verdict:** Target state, not v1. Revisit when atomicguard-rs has caught up on the runtime primitives (~5 tracked issues).

### Option C: `thompsonson/dev`

**Pro** — `dev` already runs a single-threaded HTTP-over-UDS daemon. Same service shape.

**Con** — Wrong scope. `dev` is tmux session management. Putting workflow routing here conflates two concerns and confuses readers of both.

**Con** — `dev` deployment targets are anywhere tmux runs. Workflow service needs the atomicguard runtime (Python today). That's a heavier dep.

**Verdict:** No. Kept in the list for completeness.

### Option D: `thompsonson/chops`

**Pro** — Chops is already the MQTT bus endpoint.

**Con** — Chops is a Tauri app (client). Not a server.

**Con** — Chops should consume the service, not be the service.

**Verdict:** No.

### Option E: New repo, e.g. `thompsonson/atomicguard-service` (or `workflow-bridge`)

**Pro** — Clear single-purpose repo. Independent cadence from the framework.

**Pro** — Lets atomicguard stay pure-library.

**Pro** — When atomicguard-rs catches up, this repo's Python implementation gets a Rust sibling; the API shape stays constant.

**Con** — One more repo to maintain. Cross-repo CI/release coordination.

**Con** — Initial import of workflow runtime code from atomicguard (dep), classifier (dep). Nothing novel lives here except the four capabilities.

**Verdict:** The right end-state. Premature for v1 — grow it inside atomicguard first, extract once the API stabilises.

### Recommendation

**Option A (inside atomicguard) for v1, combined with the meta-workflow lens from the earlier section. Option E (new repo) later when operational pressure justifies it. Option B (atomicguard-rs) when the Rust port has caught up on runtime primitives.**

Concretely, in order:

1. **Apply the meta-workflow lens first.** Ship Π_meta as `workflows/meta/workflow.json` + `workflows/meta/ap_context.json` inside atomicguard's catalogue, same pattern as the sysadmin workflows. The embedding classifier, parameter extractor, and dispatcher become APs — not service code. This is mostly a rearrangement of code that already exists in `examples/sysadmin/`. Π_meta is just another workflow after this.
2. **Build the residual plumbing as `atomicguard.application.service`** — about 500 LoC covering MQTT bridge, HTTP registry, TTL/heartbeat, classifier index maintenance. Lives inside atomicguard because that matches the current shape: the repo already ships `examples/sysadmin/mqtt_*.py` and `src/atomicguard/web/` (FastAPI dashboard). Option A formalises what's already there.
3. **Mirror as `ag-service` crate in atomicguard-rs** once the runtime primitives for Π_meta exist (tracked in `thompsonson/atomicguard-rs#1, #2`, plus `CommandTemplateGenerator` and `WorkflowEffector` ports). Natural fit for the existing 7-crate Cargo workspace; same four-capability API, different implementation language.
4. **Extract to a standalone repo (Option E) when operational concerns justify it** — multi-host deployment, authn/authz, rate limiting, observability features that don't belong in the framework repo. Today that pressure doesn't exist; extracting now doubles coordination cost for zero user-visible gain.

**Why not Option E today** (i.e., "keep atomicguard pure-library"): atomicguard is already not a pure library. It ships `examples/sysadmin/mqtt_workflow_service.py`, `mqtt_intent_listener.py`, and a FastAPI web UI. Committing to Option E now forces either (a) extracting those existing services too — which breaks the PoC's single-install distribution story — or (b) living with the inconsistency of two service repos. The current shape is "framework + reference system"; the workflow service formalises the reference-system role as a first-class subpackage rather than a pile of scripts under `examples/`.

## Minimum viable build

Order in which to land pieces, each independently reviewable:

0. **Write Π_meta as `workflows/meta/workflow.json` + `ap_context.json`.** Three APs: A_intent (embedding classifier + IntentGuard), A_decompose (LLM param extraction + schema guard), A_dispatch (WorkflowEffector + WorkflowStatusGuard). This is the meta-workflow lens realised as data. Everything from step 1 onward runs this workflow per utterance instead of reimplementing it.
1. **`POST /workflows` + `GET /workflows` + in-memory registry.** No classifier integration yet; a hand-wired workflow loader so the existing sysadmin catalogue works.
2. **Classifier re-indexing on register/retire.** Reuse the centroid compute from `examples/sysadmin/intent_embedding.py`. The re-index updates the A_intent generator's lookup data; Π_meta itself is unchanged.
3. **`POST /match`.** Shortcut endpoint that runs only A_intent + A_decompose (truncated Π_meta); useful for front-ends that want to preview the route before dispatching.
4. **`POST /dispatch` + `POST /run`.** `/run` executes the full Π_meta (which dispatches to Π_task via WorkflowEffector); `/dispatch` runs a named Π_task directly.
5. **MQTT out.** Events + escalations flow on the chops-compatible topics.
6. **`POST /feedback` + MQTT feedback.** Escalation loop closes.
7. **TTL + heartbeat + persistence.** Registrations survive service restart.

After step 4, you've replaced `mqtt_workflow_service.py`. After step 6, you have the full conversation/escalation surface chops needs. Step 7 is polish.

## Consumers and registrants at v1

| Role | Who | What they register / call |
|---|---|---|
| Registrant | `dev` daemon | tmux workflows: `open_claude_session`, `list_sessions`, `kill_session`, `run_and_capture` (internal) |
| Registrant | atomicguard (self-registers its built-in sysadmin catalogue at startup) | health_check, disk_check, triage, restart_service, 23 workflows total |
| Registrant | monitoring-ops (future) | otelcol_reload, prom_reload, monitoring_triage |
| Caller | chops | `/run` via MQTT utterances |
| Caller | CLI tools, cron, webhooks (future) | `/dispatch` directly |

`dev` as first external registrant is the cleanest way to validate the API: its workflows are small, well-bounded, and its UDS surface is already stable.

## Relationship to existing code

What this service would replace / subsume:

- `atomicguard/examples/sysadmin/mqtt_workflow_service.py` — becomes a thin wrapper around the new service's MQTT listener, or is deleted outright once the service handles the full topology.
- `atomicguard/examples/sysadmin/mqtt_intent_listener.py` — deleted; `/match` covers it.
- The static workflow catalogue loaded in agent-core — becomes a set of self-registrations atomicguard does at its own startup.

What stays exactly the same:

- Chops, whisper-rs transcription, the MQTT voice topic.
- `WorkflowOrchestrator`, `DualStateAgent`, all AP/guard/effector types.
- The embedding classifier's algorithm (centroid + cosine).

## Open questions beyond the repo-placement decision

1. **Registration transport** — HTTP (assumed here) vs adding a `/workflows/register` MQTT topic. HTTP is standard, easier for request/response; MQTT would reuse the existing bus. Recommend HTTP; keep MQTT for events only.
2. **Classifier re-index cost** — 79 examples currently take 2.8s one-time. Each registration adds maybe 5–10 examples. Incremental add = milliseconds. Full rebuild only needed when an example corpus changes, not on every registration.
3. **Manifest versioning** — if `dev` re-registers with a changed manifest, is that a new workflow or an update? Recommend content-hash-based: same name + different hash = update; centroid recomputed if `intent_examples` changed.
4. **Dispatch state persistence** — in-memory is fine for v1 (reboot loses in-flight dispatches). If sessions need to outlive restarts, add SQLite. Not required before steps 1–6.
5. **Authn/authz** — v1 runs on pop-mini single-user; UDS permissions enforce it. If the service binds TCP, add a token. Not critical for v1.
6. **Who self-registers the existing sysadmin catalogue?** — atomicguard's startup script? A one-shot tool? A `bootstrap` endpoint? Recommend: atomicguard's own service startup posts its built-in manifests, same API as any other registrant.

## What this doc deliberately doesn't cover

- Workflow JSON / DS-PDDL format — that's the workflow runtime's concern.
- Effector mechanics — inside workflows, not the service's concern.
- RL / training loop / workflow discovery — orthogonal.
- Multi-host federation — single pop-mini for v1.
- Rate limiting, quotas, audit log — deferrable.

## Next steps

1. Decide repo placement (the question above).
2. Draft OpenAPI for the six endpoints (`POST /workflows`, `GET /workflows`, `POST /match`, `POST /dispatch`, `POST /run`, `POST /feedback` + heartbeat).
3. Draft the MQTT topic map with message schemas.
4. Stub steps 1–2 of the MVP (registry + classifier re-indexing) against the sysadmin catalogue to prove the shape works before wiring dispatch.
5. Bring `dev` on as the first external registrant; prove the loop from chops voice to dev-pane execution end-to-end.

## References

- `thompsonson/atomicguard` `docs/masters_report/chapters/08_poc.tex` — the PoC that validates the meta-workflow pattern.
- `thompsonson/atomicguard` `docs/design/plans/system-view-intent-parsing.md` — current full-system view.
- `thompsonson/atomicguard` `docs/design/examples/sysadmin-workflows.md` — the 20-workflow catalogue.
- `thompsonson/atomicguard` `docs/design/examples/llm-intent-parsing.md` — the single-AP intent parsing design (input to capability 2).
- `thompsonson/atomicguard` `examples/sysadmin/intent_embedding.py` — the 29ms, 93% classifier.
- `thompsonson/atomicguard` `examples/sysadmin/mqtt_workflow_service.py` — the precursor this service generalises.
- `thompsonson/dev` README — the UDS daemon pattern a future atomicguard-rs port would follow.
- `thompsonson/atomicguard-rs` — Rust port target (issues #1, #2 track blocking gaps).
