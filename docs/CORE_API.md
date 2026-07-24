# Core API — versioning contract

The Core API is the versioned, module-facing surface (G-3, tech spec §2). Every
user-facing module — Ping, Chat, Drop, and every future third-party module —
consumes **only** this surface (`crate::api`, exposed to Dart under
`lib/src/rust/api/`). The engine, store, identity, and crypto stay behind it.

`api_version()` returns the current contract version; this document is its
changelog.

## The rule

- **Breaking changes bump the version.** Removing or renaming a function/field,
  changing a signature, or changing an event's meaning is breaking — bump
  `api_version()` and add a changelog entry.
- **Additive changes don't.** New functions, new `CoreEvent` variants, and new
  optional fields are backward-compatible within a version. (Dart consumers may
  need to extend a `switch` when a variant is added — that's normal evolution,
  not an API break.)
- **The P3 guard is enforced in CI.** `test/p3_api_boundary_test.dart` (an AST
  pass) fails the build if app code imports anything under `lib/src/rust/` other
  than the API surface — only `lib/main.dart` may import `frb_generated.dart`
  for `RustLib.init`.

## The six areas

| Area | Functions | Types / events |
|---|---|---|
| core | `core_init`, `api_version`, `core_version` | `PersonaDto` |
| identity | `current_persona`, `update_persona` | `PersonaDto` |
| discovery | `set_discovery`, `nearby_devices` | `NearbyDevice`, `DiscoveryUpdated` |
| sessions (messaging) | `ping`, `send_chat`, `thread_messages`, `thread_for_device`, `list_threads` | `ChatMessageDto`, `ThreadSummary`, `Pinged`, `MessageReceived` |
| transfers | `offer_drop` | `TransferProgress`, `TransferCompleted` |
| events | `core_event_stream` | `CoreEvent` |

## Known additive points (no version bump when added)

- **Incoming transfers** — a `TransferOffered` event + a `respond_to_drop`
  function arrive with T17 (Drop UI) / T16 (transfer engine). Additive.
- **Contacts** — `list_contacts` / contact editing as the UI needs them.

## Changelog

### v0 — 2026-07-24 (T07)

Initial surface. Backed by the real identity + store and a **fake network**
(scripted peers, canned transfers) standing in for the radio plane (T08–T10);
the fakes will be replaced behind this same surface without callers changing.
